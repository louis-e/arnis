//! Mapillary Graph API client: panorama metadata search and image download.
//!
//! The Graph API caps `bbox` searches at 0.01 degrees square, so an area of any
//! useful size has to be walked as a grid of sub-cells. Everything here is
//! blocking and runs in the pre-pass, before any tile thread starts.

use std::time::Duration;

use rayon::prelude::*;
use serde::Deserialize;

use crate::coordinate_system::geographic::LLBBox;
use crate::net::request_permit;

const GRAPH_URL: &str = "https://graph.mapillary.com/images";

/// Mapillary rejects bbox queries of 0.01 degrees or more. Stay under it with
/// margin so floating point never pushes a cell over the edge.
pub(super) const MAX_CELL_DEG: f64 = 0.008;

/// Hard ceiling on search cells per run, so an accidental country-sized bbox
/// cannot fire tens of thousands of requests.
pub(super) const MAX_CELLS: usize = 4096;

/// Fields we ask for. `computed_*` are the SfM-corrected values and are much
/// better than the raw EXIF ones; we fall back to raw only when absent.
const FIELDS: &str = "id,computed_geometry,geometry,computed_compass_angle,compass_angle,\
computed_rotation,camera_type,is_pano,quality_score,width,height,thumb_1024_url";

/// How many times a cell may split when Mapillary says the response is too
/// large. Five levels take the starting cell from ~890 m down to ~28 m, which
/// is what the Python prototype the facade pipeline is verified against uses,
/// so the two see the same images in a dense centre.
///
/// The API sometimes refuses a cell of a few tens of metres, which no amount of
/// subdividing can be the answer to, so past this depth the cell is reported
/// and skipped instead. Only cells that actually get refused ever split, so a
/// sparse area still costs one request.
pub(super) const MAX_SUBDIVISION_DEPTH: u32 = 5;

/// Mapillary's own quality estimate; OpenFACADES discards anything below 0.2.
const MIN_QUALITY_SCORE: f64 = 0.2;

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<RawImage>,
}

#[derive(Debug, Deserialize)]
struct RawImage {
    id: String,
    computed_geometry: Option<GeoJsonPoint>,
    geometry: Option<GeoJsonPoint>,
    computed_compass_angle: Option<f64>,
    compass_angle: Option<f64>,
    computed_rotation: Option<[f64; 3]>,
    camera_type: Option<String>,
    is_pano: Option<bool>,
    quality_score: Option<f64>,
    thumb_1024_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeoJsonPoint {
    /// GeoJSON order: [lon, lat].
    coordinates: [f64; 2],
}

/// One usable panorama: pose plus the URL to fetch its pixels from.
#[derive(Debug, Clone)]
pub struct PanoMeta {
    pub id: String,
    pub lat: f64,
    pub lon: f64,
    /// Degrees clockwise from north, the direction the panorama's horizontal
    /// centre column faces.
    pub compass_angle: f64,
    /// SfM axis-angle orientation (OpenSfM, world-to-camera). Absent on a few
    /// images, which then fall back to a level pose.
    pub rotation: Option<[f64; 3]>,
    pub quality_score: f64,
    pub thumb_url: String,
}

impl RawImage {
    /// Keeps only equirectangular images that carry a usable pose and URL.
    fn into_meta(self) -> Option<PanoMeta> {
        let is_pano = self.is_pano.unwrap_or(false)
            || matches!(
                self.camera_type.as_deref(),
                Some("equirectangular") | Some("spherical")
            );
        if !is_pano {
            return None;
        }

        let quality_score = self.quality_score.unwrap_or(1.0);
        if quality_score < MIN_QUALITY_SCORE {
            return None;
        }

        // SfM-corrected pose first; raw EXIF only as a fallback.
        let point = self.computed_geometry.or(self.geometry)?;
        let compass_angle = self.computed_compass_angle.or(self.compass_angle)?;
        let thumb_url = self.thumb_1024_url?;

        let [lon, lat] = point.coordinates;
        if !lon.is_finite() || !lat.is_finite() || !compass_angle.is_finite() {
            return None;
        }

        Some(PanoMeta {
            id: self.id,
            lat,
            lon,
            compass_angle,
            rotation: self
                .computed_rotation
                .filter(|r| r.iter().all(|c| c.is_finite())),
            quality_score,
            thumb_url,
        })
    }
}

fn client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .user_agent(crate::retrieve_data::OSM_USER_AGENT)
        .build()
        .map_err(|e| format!("Mapillary HTTP client: {e}"))
}

/// A search rectangle, in the order the Graph API wants it:
/// `(min_lon, min_lat, max_lon, max_lat)`.
pub(super) type Cell = (f64, f64, f64, f64);

/// Splits a cell into four, for the adaptive retry below.
pub(super) fn quadrants(cell: Cell) -> [Cell; 4] {
    let (min_lon, min_lat, max_lon, max_lat) = cell;
    let mid_lon = (min_lon + max_lon) / 2.0;
    let mid_lat = (min_lat + max_lat) / 2.0;
    [
        (min_lon, min_lat, mid_lon, mid_lat),
        (mid_lon, min_lat, max_lon, mid_lat),
        (min_lon, mid_lat, mid_lon, max_lat),
        (mid_lon, mid_lat, max_lon, max_lat),
    ]
}

/// Splits `llbbox` into cells small enough for the Graph API's bbox limit.
///
/// A box needing more than [`MAX_CELLS`] is refused rather than truncated: the
/// caller reports what comes back as coverage for the whole box, and a band
/// across the bottom of it is not that.
pub(super) fn search_cells(llbbox: &LLBBox) -> Result<Vec<Cell>, String> {
    let (min_lat, min_lon) = (llbbox.min().lat(), llbbox.min().lng());
    let (max_lat, max_lon) = (llbbox.max().lat(), llbbox.max().lng());

    let lat_steps = (((max_lat - min_lat) / MAX_CELL_DEG).ceil() as usize).max(1);
    let lon_steps = (((max_lon - min_lon) / MAX_CELL_DEG).ceil() as usize).max(1);

    let lat_span = (max_lat - min_lat) / lat_steps as f64;
    let lon_span = (max_lon - min_lon) / lon_steps as f64;

    if lat_steps.saturating_mul(lon_steps) > MAX_CELLS {
        return Err(format!(
            "this area needs {} imagery search cells and the limit is {MAX_CELLS}. \
             Search a smaller area.",
            lat_steps * lon_steps
        ));
    }

    let mut cells = Vec::with_capacity(lat_steps * lon_steps);
    for i in 0..lat_steps {
        for j in 0..lon_steps {
            let lo_lat = min_lat + lat_span * i as f64;
            let lo_lon = min_lon + lon_span * j as f64;
            cells.push((
                lo_lon,
                lo_lat,
                (lo_lon + lon_span).min(max_lon),
                (lo_lat + lat_span).min(max_lat),
            ));
        }
    }
    Ok(cells)
}

/// Mapillary's error envelope, so a rejected request can say why.
#[derive(Debug, Deserialize)]
struct ApiError {
    error: ApiErrorBody,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    message: String,
    #[serde(default)]
    #[serde(rename = "type")]
    kind: String,
}

/// Turns a non-success response into a message worth showing the user.
///
/// Mapillary answers a bad token with a JSON envelope naming the problem, and
/// that message is far more actionable than the status code alone - "Invalid
/// OAuth access token" versus a bare 401.
pub(super) fn describe_failure(status: reqwest::StatusCode, body: &str) -> String {
    match serde_json::from_str::<ApiError>(body) {
        Ok(parsed) => format!("Mapillary API {status}: {}", parsed.error.message),
        // Not the documented envelope; show a bounded slice of whatever came back.
        Err(_) => {
            let snippet: String = body.chars().take(200).collect();
            if snippet.trim().is_empty() {
                format!("Mapillary API {status}")
            } else {
                format!("Mapillary API {status}: {snippet}")
            }
        }
    }
}

/// Whether the response is the API refusing the credential rather than the
/// request.
///
/// Worth its own question because such a refusal is the one failure that will
/// never come out differently on a retry, and because the shapes it arrives in
/// do not line up with the status codes: a token Mapillary cannot even parse is
/// a 400 carrying an `OAuthException`, not the 401 the status alone would
/// suggest. Retrying either would only make the user wait for the same answer
/// once per attempt and once per search cell.
pub(super) fn is_auth_failure(status: reqwest::StatusCode, body: &str) -> bool {
    if status.as_u16() == 401 {
        return true;
    }
    match serde_json::from_str::<ApiError>(body) {
        Ok(parsed) => {
            parsed.error.kind == "OAuthException"
                || parsed
                    .error
                    .message
                    .to_ascii_lowercase()
                    .contains("access token")
        }
        Err(_) => false,
    }
}

/// Whether Mapillary is asking for a smaller query rather than reporting a
/// real failure.
///
/// A dense city centre overruns the response size limit even inside the
/// documented 0.01-degree bbox, and the API signals that with a 500 whose
/// message asks for less data rather than with a dedicated status code.
pub(super) fn wants_smaller_area(body: &str) -> bool {
    body.to_ascii_lowercase()
        .contains("reduce the amount of data")
}

/// Fetches one cell, splitting it and retrying when the response would be too
/// large. Sparse areas therefore cost a single request while a dense one pays
/// only for the cells that actually needed subdividing.
fn fetch_cell(
    client: &reqwest::blocking::Client,
    token: &str,
    cell: Cell,
    depth: u32,
) -> Result<Vec<PanoMeta>, String> {
    let (min_lon, min_lat, max_lon, max_lat) = cell;
    let bbox = format!("{min_lon},{min_lat},{max_lon},{max_lat}");

    let response = {
        let _permit = request_permit();
        client
            .get(GRAPH_URL)
            .query(&[
                ("access_token", token),
                ("fields", FIELDS),
                ("bbox", bbox.as_str()),
                ("is_pano", "true"),
            ])
            .send()
            // without_url: the URL carries the access token as a query
            // parameter and reqwest's Display would print it.
            .map_err(|e| format!("Mapillary request failed: {}", e.without_url()))?
    };

    let status = response.status();
    let body = response
        .text()
        .map_err(|e| format!("Mapillary response unreadable: {e}"))?;

    if !status.is_success() {
        if !wants_smaller_area(&body) {
            return Err(describe_failure(status, &body));
        }
        if depth >= MAX_SUBDIVISION_DEPTH {
            return Err(format!(
                "Mapillary refused a {min_lon},{min_lat},{max_lon},{max_lat} cell even after {depth} subdivisions"
            ));
        }
        // Four smaller queries, each of which may split again.
        let parts: Vec<Result<Vec<PanoMeta>, String>> = quadrants(cell)
            .par_iter()
            .map(|&quadrant| fetch_cell(client, token, quadrant, depth + 1))
            .collect();
        let mut merged = Vec::new();
        for part in parts {
            merged.extend(part?);
        }
        return Ok(merged);
    }

    let parsed: SearchResponse = serde_json::from_str(&body)
        .map_err(|e| format!("Mapillary response was not the expected JSON: {e}"))?;
    Ok(parsed
        .data
        .into_iter()
        .filter_map(RawImage::into_meta)
        .collect())
}

/// Fetches every panorama Mapillary lists inside `llbbox`.
///
/// Cells are queried in parallel. A cell that fails is a hole in coverage as
/// long as others succeeded, but if every cell fails the error is returned
/// rather than reported as an empty area: a bad token and genuinely uncovered
/// ground otherwise look identical, which is the most confusing way this can
/// go wrong.
pub fn fetch_panoramas(
    llbbox: &LLBBox,
    token: &str,
    timeout: Duration,
) -> Result<Vec<PanoMeta>, String> {
    let client = client(timeout)?;
    let cells = search_cells(llbbox)?;

    let results: Vec<Result<Vec<PanoMeta>, String>> = cells
        .par_iter()
        .map(|&cell| fetch_cell(&client, token, cell, 0))
        .collect();

    let total = results.len();
    let mut all = Vec::new();
    let mut first_error = None;
    let mut failed = 0usize;
    for result in results {
        match result {
            Ok(metas) => all.extend(metas),
            Err(e) => {
                failed += 1;
                first_error.get_or_insert(e);
            }
        }
    }

    if let Some(error) = first_error {
        if failed == total {
            return Err(error);
        }
        eprintln!("Warning: {failed} of {total} Mapillary search cells failed ({error})");
    }

    // Neighbouring cells share images on their shared edge, and a subdivided
    // cell can return the same image from two quadrants.
    all.sort_by(|a, b| a.id.cmp(&b.id));
    all.dedup_by(|a, b| a.id == b.id);
    Ok(all)
}

/// A decoded equirectangular panorama.
pub struct Panorama {
    pub meta: PanoMeta,
    pub pixels: image::RgbImage,
}

/// Downloads and decodes the given panoramas, dropping any that fail.
///
/// Sorted by id on return so the caller sees a stable order regardless of how
/// the thread pool interleaved the downloads.
pub fn download_panoramas(metas: &[PanoMeta], timeout: Duration) -> Result<Vec<Panorama>, String> {
    let client = client(timeout)?;

    let mut panos: Vec<Panorama> = metas
        .par_iter()
        .filter_map(|meta| {
            let bytes = {
                let _permit = request_permit();
                client.get(&meta.thumb_url).send().ok()?.bytes().ok()?
            };
            let pixels = image::load_from_memory(&bytes).ok()?.to_rgb8();
            // A pano is 2:1; anything else means we were handed a crop.
            if pixels.width() < 2 * pixels.height() {
                return None;
            }
            Some(Panorama {
                meta: meta.clone(),
                pixels,
            })
        })
        .collect();

    panos.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
    Ok(panos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_stay_under_the_api_bbox_limit() {
        // ~2.2 km square, comfortably over one cell.
        let bbox = LLBBox::new(52.50, 13.40, 52.52, 13.42).unwrap();
        let cells = search_cells(&bbox).unwrap();
        assert!(cells.len() > 1);
        for (min_lon, min_lat, max_lon, max_lat) in cells {
            assert!(max_lon - min_lon < 0.01, "cell too wide");
            assert!(max_lat - min_lat < 0.01, "cell too tall");
        }
    }

    #[test]
    fn cells_cover_the_whole_bbox() {
        let bbox = LLBBox::new(52.50, 13.40, 52.53, 13.44).unwrap();
        let cells = search_cells(&bbox).unwrap();
        let max_lon = cells.iter().map(|c| c.2).fold(f64::MIN, f64::max);
        let max_lat = cells.iter().map(|c| c.3).fold(f64::MIN, f64::max);
        assert!((max_lon - 13.44).abs() < 1e-9);
        assert!((max_lat - 52.53).abs() < 1e-9);
    }

    #[test]
    fn tiny_bbox_yields_one_cell() {
        let bbox = LLBBox::new(52.5000, 13.4000, 52.5005, 13.4005).unwrap();
        assert_eq!(search_cells(&bbox).unwrap().len(), 1);
    }

    /// A box past the ceiling is refused, not answered with a slice of itself.
    #[test]
    fn an_oversized_search_area_is_refused() {
        let bbox = LLBBox::new(40.0, -5.0, 48.0, 5.0).unwrap();
        let err = search_cells(&bbox).unwrap_err();
        assert!(err.contains("smaller area"), "{err}");
    }

    /// Builds an otherwise-valid API record so each case can vary one field.
    fn raw_image(camera_type: &str, is_pano: bool, quality: f64) -> RawImage {
        RawImage {
            id: "1".into(),
            computed_geometry: Some(GeoJsonPoint {
                coordinates: [13.4, 52.5],
            }),
            geometry: None,
            computed_compass_angle: Some(90.0),
            compass_angle: None,
            camera_type: Some(camera_type.into()),
            is_pano: Some(is_pano),
            computed_rotation: None,
            quality_score: Some(quality),
            thumb_1024_url: Some("https://example.invalid/p.jpg".into()),
        }
    }

    #[test]
    fn perspective_images_are_rejected() {
        assert!(raw_image("perspective", false, 0.9).into_meta().is_none());
    }

    #[test]
    fn equirectangular_images_are_kept() {
        let meta = raw_image("equirectangular", true, 0.9).into_meta().unwrap();
        assert_eq!(meta.lat, 52.5);
        assert_eq!(meta.lon, 13.4);
        assert_eq!(meta.compass_angle, 90.0);
    }

    #[test]
    fn camera_type_alone_is_enough_when_is_pano_is_missing() {
        let mut raw = raw_image("spherical", false, 0.9);
        raw.is_pano = None;
        assert!(raw.into_meta().is_some());
    }

    #[test]
    fn low_quality_images_are_rejected() {
        assert!(raw_image("equirectangular", true, 0.05)
            .into_meta()
            .is_none());
    }

    #[test]
    fn images_without_a_pose_are_rejected() {
        let mut raw = raw_image("equirectangular", true, 0.9);
        raw.computed_compass_angle = None;
        assert!(raw.into_meta().is_none());
    }
}
