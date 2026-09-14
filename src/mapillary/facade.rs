//! Wall-plane extraction, view selection and colour aggregation.
//!
//! For each building footprint the walls are rebuilt as planes in world-block
//! space, every panorama that can see a wall head-on is scored, and a grid of
//! points on the wall is sampled from up to a handful of views. Taking the
//! median per cell across views is what removes parked cars and pedestrians:
//! they move between captures, the wall does not.

use fnv::FnvHashMap;

use crate::colors::{rgb_to_oklab, RGBTuple};
use crate::mapillary::api::Panorama;
use crate::mapillary::project::{self, CameraPose, Reject};

/// Farthest a camera can be from a wall and still contribute. Beyond this a
/// facade is a handful of pixels wide and mostly atmosphere.
const MAX_VIEW_DISTANCE_M: f64 = 45.0;

/// Closest usable camera. Nearer than this the pano is looking almost straight
/// up the wall and the sample band leaves the building entirely.
const MIN_VIEW_DISTANCE_M: f64 = 4.0;

/// Maximum angle between the wall normal and the camera direction. OpenFACADES
/// keeps 10-130 degrees of angular width; this is the equivalent constraint
/// expressed per wall rather than per building.
const MAX_INCIDENCE_DEG: f64 = 60.0;

/// Views sampled per wall. Three is enough for the median to reject a parked
/// car; more mostly costs downloads.
const MAX_VIEWS_PER_WALL: usize = 4;

/// Fraction of a wall's length skipped at each end, so a sample never lands on
/// the neighbouring building across a party wall.
const EDGE_INSET_FRAC: f64 = 0.12;

/// Walls shorter than this carry too few samples to be worth projecting.
const MIN_WALL_LENGTH_M: f64 = 3.0;

/// Vertical band sampled on each wall, in metres above the building base.
/// The lower bound clears parked cars and ground-floor shopfronts, whose
/// glazing and signage say nothing about the building's colour; the upper
/// bound stays below the roofline of a typical mid-rise.
const BAND_MIN_M: f64 = 2.5;
const BAND_MAX_M: f64 = 12.0;

/// Assumed camera height above the building base. Mapillary's `computed_altitude`
/// is too noisy to use directly and Arnis has no per-building terrain height at
/// this stage, so a typical phone/roof-mount height is assumed instead. An error
/// here tilts the sample band slightly; it does not move it off the wall.
pub const CAMERA_HEIGHT_M: f64 = 2.5;

/// Assumed storey height when a building has `building:levels` but no `height`.
const METRES_PER_LEVEL: f64 = 3.0;

/// Fallback building height when neither tag is present.
const DEFAULT_HEIGHT_M: f64 = 9.0;

/// Cells that must survive filtering before a colour is trusted.
const MIN_ACCEPTED_CELLS: usize = 12;

/// Share of accepted cells the winning colour cluster must hold. Below this the
/// wall is a jumble - usually heavy occlusion - and no colour is emitted.
const MIN_DOMINANT_SHARE: f64 = 0.15;

/// A single wall of a footprint, as a plane in world-block space.
#[derive(Debug, Clone, Copy)]
pub struct WallSegment {
    pub ax: f64,
    pub az: f64,
    pub bx: f64,
    pub bz: f64,
    /// Unit outward normal in XZ.
    pub nx: f64,
    pub nz: f64,
    /// Length in blocks.
    pub length: f64,
}

/// Signed area of a footprint, used to resolve which side of a wall is outside.
fn signed_area(points: &[(f64, f64)]) -> f64 {
    let mut acc = 0.0;
    for i in 0..points.len() {
        let (x0, z0) = points[i];
        let (x1, z1) = points[(i + 1) % points.len()];
        acc += x0 * z1 - x1 * z0;
    }
    acc
}

/// Rebuilds a closed footprint's walls with outward normals.
///
/// Winding is resolved once from the signed area rather than per edge, so
/// concave footprints (courtyards, L-shapes) keep consistent normals.
pub fn wall_segments(points: &[(f64, f64)], scale: f64) -> Vec<WallSegment> {
    if points.len() < 3 {
        return Vec::new();
    }
    // A closed ring repeats its first node; drop it so edges are not doubled.
    let ring = if points.first() == points.last() {
        &points[..points.len() - 1]
    } else {
        points
    };
    if ring.len() < 3 {
        return Vec::new();
    }

    let clockwise = signed_area(ring) > 0.0;
    let min_length = MIN_WALL_LENGTH_M * scale;

    let mut walls = Vec::with_capacity(ring.len());
    for i in 0..ring.len() {
        let (ax, az) = ring[i];
        let (bx, bz) = ring[(i + 1) % ring.len()];
        let (dx, dz) = (bx - ax, bz - az);
        let length = (dx * dx + dz * dz).sqrt();
        if length < min_length {
            continue;
        }
        let (tx, tz) = (dx / length, dz / length);
        // Rotating the tangent one way or the other points out of the polygon
        // depending on which way the ring is wound.
        let (nx, nz) = if clockwise { (tz, -tx) } else { (-tz, tx) };

        walls.push(WallSegment {
            ax,
            az,
            bx,
            bz,
            nx,
            nz,
            length,
        });
    }
    walls
}

/// Building height in metres from OSM tags, falling back to a mid-rise default.
pub fn building_height_m(tags: &std::collections::HashMap<String, String>) -> f64 {
    if let Some(height) = tags.get("height").and_then(|h| {
        // "12", "12 m" and "12.5" all appear in the wild.
        h.trim()
            .trim_end_matches(|c: char| c.is_alphabetic() || c.is_whitespace())
            .parse::<f64>()
            .ok()
    }) {
        if height > 1.0 && height < 400.0 {
            return height;
        }
    }
    if let Some(levels) = tags
        .get("building:levels")
        .and_then(|l| l.trim().parse::<f64>().ok())
    {
        if (1.0..150.0).contains(&levels) {
            return levels * METRES_PER_LEVEL;
        }
    }
    DEFAULT_HEIGHT_M
}

/// The vertical band to sample on a wall, in blocks above the building base.
///
/// Returns `None` for a building too low to offer a band clear of both the
/// shopfront and the roofline.
fn sample_band(height_m: f64, scale: f64) -> Option<(f64, f64)> {
    let top = (height_m - 1.0).min(BAND_MAX_M);
    let bottom = BAND_MIN_M.min(top - 1.0);
    if top - bottom < 1.5 {
        return None;
    }
    Some((bottom * scale, top * scale))
}

/// A camera chosen to sample a particular wall.
struct ScoredView<'a> {
    pano: &'a Panorama,
    pose: CameraPose,
    score: f64,
}

/// Picks the panoramas that see `wall` most directly.
///
/// Scoring prefers head-on and close: an oblique view foreshortens the facade
/// into a few pixels, and a distant one loses it to haze and JPEG noise.
fn select_views<'a>(
    wall: &WallSegment,
    cameras: &'a [(CameraPose, &'a Panorama)],
    scale: f64,
) -> Vec<ScoredView<'a>> {
    let (mx, mz) = ((wall.ax + wall.bx) / 2.0, (wall.az + wall.bz) / 2.0);
    let min_dist = MIN_VIEW_DISTANCE_M * scale;
    let max_dist = MAX_VIEW_DISTANCE_M * scale;
    let min_cos = MAX_INCIDENCE_DEG.to_radians().cos();

    let mut scored: Vec<ScoredView<'a>> = cameras
        .iter()
        .filter_map(|(pose, pano)| {
            let (vx, vz) = (pose.x - mx, pose.z - mz);
            let dist = (vx * vx + vz * vz).sqrt();
            if dist < min_dist || dist > max_dist {
                return None;
            }
            // Positive only when the camera is on the outward side of the wall.
            let cos_incidence = (vx * wall.nx + vz * wall.nz) / dist;
            if cos_incidence < min_cos {
                return None;
            }
            Some(ScoredView {
                pano,
                pose: *pose,
                score: cos_incidence / (1.0 + dist / (20.0 * scale)),
            })
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            // Ties would otherwise depend on thread interleaving upstream.
            .then_with(|| a.pano.meta.id.cmp(&b.pano.meta.id))
    });
    scored.truncate(MAX_VIEWS_PER_WALL);
    scored
}

/// A wall's sampled colours laid out as they sit on the facade, for debugging.
pub struct WallGrid {
    pub cols: usize,
    pub rows: usize,
    /// Row-major, top row first. `None` where every view was rejected.
    pub cells: Vec<Option<RGBTuple>>,
}

/// Counts of why samples were discarded, for the probe report.
#[derive(Debug, Default, Clone, Copy)]
pub struct RejectStats {
    pub too_dark: usize,
    pub too_bright: usize,
    pub vegetation: usize,
    pub sky: usize,
    pub no_view: usize,
}

impl RejectStats {
    fn record(&mut self, reject: Reject) {
        match reject {
            Reject::TooDark => self.too_dark += 1,
            Reject::TooBright => self.too_bright += 1,
            Reject::Vegetation => self.vegetation += 1,
            Reject::Sky => self.sky += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.too_dark + self.too_bright + self.vegetation + self.sky + self.no_view
    }
}

/// Samples one wall across its selected views.
///
/// Each cell takes the median of whatever survived filtering in each view, so a
/// cell is only lost when every view is occluded at that spot.
fn sample_wall(
    wall: &WallSegment,
    cameras: &[(CameraPose, &Panorama)],
    band: (f64, f64),
    scale: f64,
    stats: &mut RejectStats,
    keep_grid: bool,
) -> (Vec<RGBTuple>, Option<WallGrid>) {
    let views = select_views(wall, cameras, scale);
    if views.is_empty() {
        return (Vec::new(), None);
    }

    // One sample per block, which is the resolution the output is consumed at.
    let inset = wall.length * EDGE_INSET_FRAC;
    let usable = wall.length - 2.0 * inset;
    let cols = (usable.round() as usize).clamp(1, 64);
    let rows = ((band.1 - band.0).round() as usize).clamp(1, 32);

    let (tx, tz) = (
        (wall.bx - wall.ax) / wall.length,
        (wall.bz - wall.az) / wall.length,
    );
    // Lift the sample plane a hair off the wall so it cannot land behind it.
    let epsilon = 0.05 * scale;

    let mut accepted = Vec::with_capacity(cols * rows);
    let mut grid_cells = keep_grid.then(|| Vec::with_capacity(cols * rows));

    for row in 0..rows {
        // Top row first, so a dumped grid reads the same way up as the facade.
        let t = (row as f64 + 0.5) / rows as f64;
        let y = band.1 - t * (band.1 - band.0);

        for col in 0..cols {
            let along = inset + (col as f64 + 0.5) * usable / cols as f64;
            let x = wall.ax + tx * along + wall.nx * epsilon;
            let z = wall.az + tz * along + wall.nz * epsilon;

            let mut per_view: Vec<RGBTuple> = Vec::with_capacity(views.len());
            for view in &views {
                let (u, v) = project::project(&view.pose, x, y, z);
                let Some(raw) = project::sample_pixel(&view.pano.pixels, u, v) else {
                    stats.no_view += 1;
                    continue;
                };
                match project::classify(raw) {
                    Ok(color) => per_view.push(color),
                    Err(reject) => stats.record(reject),
                }
            }

            let cell = project::median_color(&mut per_view);
            if let Some(color) = cell {
                accepted.push(color);
            }
            if let Some(cells) = grid_cells.as_mut() {
                cells.push(cell);
            }
        }
    }

    let grid = grid_cells.map(|cells| WallGrid { cols, rows, cells });
    (accepted, grid)
}

/// Bin counts for the dominant-colour histogram. Facades cluster tightly in
/// chroma, so the a/b axes get a narrow range and finer bins than a general
/// image would need.
const L_BINS: usize = 10;
const AB_BINS: usize = 12;
const AB_RANGE: f32 = 0.2;

/// The most common colour among the accepted cells.
///
/// A mean would slide toward the midpoint between wall and windows, which on a
/// half-glazed facade is a colour that appears nowhere on the building. Binning
/// in Oklab and averaging only the heaviest bin returns an actual wall colour.
pub fn dominant_color(cells: &[RGBTuple]) -> Option<(RGBTuple, f64)> {
    if cells.len() < MIN_ACCEPTED_CELLS {
        return None;
    }

    let mut bins: FnvHashMap<(usize, usize, usize), Vec<RGBTuple>> = FnvHashMap::default();
    for &cell in cells {
        let (l, a, b) = rgb_to_oklab(cell.0, cell.1, cell.2);
        let li = ((l.clamp(0.0, 1.0) * L_BINS as f32) as usize).min(L_BINS - 1);
        let quantize_ab = |v: f32| {
            let normalized = (v.clamp(-AB_RANGE, AB_RANGE) + AB_RANGE) / (2.0 * AB_RANGE);
            ((normalized * AB_BINS as f32) as usize).min(AB_BINS - 1)
        };
        bins.entry((li, quantize_ab(a), quantize_ab(b)))
            .or_default()
            .push(cell);
    }

    // Ties break on the bin key so the result never depends on hash order.
    let (_, members) = bins
        .iter()
        .max_by(|(ka, va), (kb, vb)| va.len().cmp(&vb.len()).then_with(|| kb.cmp(ka)))?;

    let share = members.len() as f64 / cells.len() as f64;
    if share < MIN_DOMINANT_SHARE {
        return None;
    }

    let n = members.len() as u32;
    let sum = members.iter().fold((0u32, 0u32, 0u32), |acc, c| {
        (acc.0 + c.0 as u32, acc.1 + c.1 as u32, acc.2 + c.2 as u32)
    });
    Some((
        ((sum.0 / n) as u8, (sum.1 / n) as u8, (sum.2 / n) as u8),
        share,
    ))
}

/// Resolution of the debug preview, in samples per block.
pub const PREVIEW_PX_PER_BLOCK: u32 = 8;

/// Renders one wall from its single best view, at a resolution far above the
/// block grid and over the wall's whole height.
///
/// This exists purely to make the projection auditable. The block grid is too
/// coarse to eyeball, but the same projection run at 8x with no filtering and
/// no median produces a recognisable ortho-rectified photograph of the facade -
/// windows, floor bands, roofline. If that looks like the building, the pose,
/// the winding and the heading convention are all right.
pub fn render_wall_preview(
    wall: &WallSegment,
    cameras: &[(CameraPose, &Panorama)],
    height_m: f64,
    scale: f64,
) -> Option<(image::RgbImage, (u32, u32), String)> {
    let views = select_views(wall, cameras, scale);
    let view = views.first()?;

    let inset = wall.length * EDGE_INSET_FRAC;
    let usable = wall.length - 2.0 * inset;
    // The full wall, plus a little sky, rather than just the sampled band.
    let top = (height_m + 1.0) * scale;

    let cols = ((usable.round() as u32) * PREVIEW_PX_PER_BLOCK).clamp(1, 2048);
    let rows = ((top.round() as u32) * PREVIEW_PX_PER_BLOCK).clamp(1, 1024);

    let (tx, tz) = (
        (wall.bx - wall.ax) / wall.length,
        (wall.bz - wall.az) / wall.length,
    );
    let epsilon = 0.05 * scale;

    let mut img = image::RgbImage::new(cols, rows);
    for row in 0..rows {
        let y = top * (1.0 - (row as f64 + 0.5) / rows as f64);
        for col in 0..cols {
            let along = inset + (col as f64 + 0.5) * usable / cols as f64;
            let x = wall.ax + tx * along + wall.nx * epsilon;
            let z = wall.az + tz * along + wall.nz * epsilon;

            let (u, v) = project::project(&view.pose, x, y, z);
            // Unfiltered on purpose: the point is to show what is really there.
            let px = project::sample_pixel(&view.pano.pixels, u, v).unwrap_or((0, 0, 0));
            img.put_pixel(col, row, image::Rgb([px.0, px.1, px.2]));
        }
    }

    // Where the sampled band sits in this image, so the two can be compared.
    let band = sample_band(height_m, scale)?;
    let to_row = |world_y: f64| ((1.0 - world_y / top) * rows as f64).round() as u32;
    Some((
        img,
        (to_row(band.1), to_row(band.0)),
        view.pano.meta.id.clone(),
    ))
}

/// Everything Tier 1 learned about one building.
pub struct BuildingSample {
    pub way_id: u64,
    pub color: RGBTuple,
    /// Share of accepted cells backing the winning colour, 0.0..1.0.
    pub confidence: f64,
    pub accepted_cells: usize,
    pub walls_sampled: usize,
    pub rejects: RejectStats,
    /// Per-wall colour grids, only populated when debug output was requested.
    pub grids: Vec<WallGrid>,
    /// High-resolution render of the best-covered wall, with the row range the
    /// block grid was taken from and the id of the panorama it came from.
    /// Debug output only.
    pub preview: Option<(image::RgbImage, (u32, u32), String)>,
    /// Footprint centroid in world blocks, so a sampled building can be found
    /// in the generated world.
    pub world_xz: (i32, i32),
}

/// Runs the whole per-building pass: walls, views, samples, dominant colour.
///
/// Returns `None` when the building has no wall a panorama can see well enough,
/// which is the common case outside well-covered streets.
pub fn sample_building(
    way_id: u64,
    footprint: &[(f64, f64)],
    height_m: f64,
    cameras: &[(CameraPose, &Panorama)],
    scale: f64,
    keep_grids: bool,
) -> Option<BuildingSample> {
    let band = sample_band(height_m, scale)?;
    let walls = wall_segments(footprint, scale);
    if walls.is_empty() {
        return None;
    }

    let mut all_cells = Vec::new();
    let mut rejects = RejectStats::default();
    let mut grids = Vec::new();
    let mut walls_sampled = 0;
    // The wall that contributed most is the one worth previewing.
    let mut best: Option<(usize, &WallSegment)> = None;

    for wall in &walls {
        let (cells, grid) = sample_wall(wall, cameras, band, scale, &mut rejects, keep_grids);
        if cells.is_empty() {
            continue;
        }
        walls_sampled += 1;
        if best.is_none_or(|(count, _)| cells.len() > count) {
            best = Some((cells.len(), wall));
        }
        all_cells.extend(cells);
        if let Some(grid) = grid {
            grids.push(grid);
        }
    }

    let (color, confidence) = dominant_color(&all_cells)?;
    let preview = keep_grids
        .then(|| best.and_then(|(_, wall)| render_wall_preview(wall, cameras, height_m, scale)))
        .flatten();

    let n = footprint.len() as f64;
    let centroid = footprint
        .iter()
        .fold((0.0, 0.0), |acc, (x, z)| (acc.0 + x, acc.1 + z));

    Some(BuildingSample {
        way_id,
        color,
        confidence,
        accepted_cells: all_cells.len(),
        walls_sampled,
        rejects,
        grids,
        preview,
        world_xz: ((centroid.0 / n) as i32, (centroid.1 / n) as i32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// 10x10 square, wound clockwise in world space (+X east, +Z south).
    fn square() -> Vec<(f64, f64)> {
        vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]
    }

    /// 20x10 footprint, wide enough that its long walls carry a full sample row.
    fn square_20x10() -> Vec<(f64, f64)> {
        vec![(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)]
    }

    #[test]
    fn normals_point_away_from_the_footprint() {
        let walls = wall_segments(&square(), 1.0);
        assert_eq!(walls.len(), 4);
        let centre = (5.0, 5.0);
        for wall in walls {
            let (mx, mz) = ((wall.ax + wall.bx) / 2.0, (wall.az + wall.bz) / 2.0);
            // Stepping along the normal must increase distance from the centre.
            let before = (mx - centre.0).hypot(mz - centre.1);
            let after = (mx + wall.nx - centre.0).hypot(mz + wall.nz - centre.1);
            assert!(after > before, "normal points inward on wall at {mx},{mz}");
        }
    }

    #[test]
    fn winding_direction_does_not_change_the_normals() {
        let mut reversed = square();
        reversed.reverse();
        let walls = wall_segments(&reversed, 1.0);
        let centre = (5.0, 5.0);
        for wall in walls {
            let (mx, mz) = ((wall.ax + wall.bx) / 2.0, (wall.az + wall.bz) / 2.0);
            let before = (mx - centre.0).hypot(mz - centre.1);
            let after = (mx + wall.nx - centre.0).hypot(mz + wall.nz - centre.1);
            assert!(after > before, "reversed winding flipped a normal");
        }
    }

    #[test]
    fn closed_rings_do_not_produce_a_duplicate_wall() {
        let mut ring = square();
        ring.push(ring[0]);
        assert_eq!(wall_segments(&ring, 1.0).len(), 4);
    }

    #[test]
    fn short_walls_are_skipped() {
        // A 2 m nub between two long walls.
        let footprint = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 2.0), (0.0, 2.0)];
        let walls = wall_segments(&footprint, 1.0);
        assert_eq!(walls.len(), 2, "the 2 m ends should drop out");
    }

    #[test]
    fn height_parses_tags_and_falls_back() {
        let mut tags = HashMap::new();
        assert_eq!(building_height_m(&tags), DEFAULT_HEIGHT_M);

        tags.insert("building:levels".into(), "4".into());
        assert_eq!(building_height_m(&tags), 12.0);

        // An explicit height wins over the levels estimate.
        tags.insert("height".into(), "17.5".into());
        assert_eq!(building_height_m(&tags), 17.5);

        tags.insert("height".into(), "20 m".into());
        assert_eq!(building_height_m(&tags), 20.0);

        // Nonsense falls back rather than producing a negative band.
        tags.insert("height".into(), "0".into());
        assert_eq!(building_height_m(&tags), 12.0);
    }

    #[test]
    fn band_clears_the_shopfront_and_the_roof() {
        let (bottom, top) = sample_band(20.0, 1.0).unwrap();
        assert_eq!(bottom, BAND_MIN_M);
        assert_eq!(top, BAND_MAX_M);

        // A two-storey building samples below the default band top.
        let (_, low_top) = sample_band(7.0, 1.0).unwrap();
        assert_eq!(low_top, 6.0);

        // A single-storey shed offers no usable band.
        assert!(sample_band(3.0, 1.0).is_none());
    }

    #[test]
    fn band_follows_world_scale() {
        let (bottom, top) = sample_band(20.0, 2.0).unwrap();
        assert_eq!(bottom, BAND_MIN_M * 2.0);
        assert_eq!(top, BAND_MAX_M * 2.0);
    }

    #[test]
    fn dominant_colour_ignores_the_window_cluster() {
        // Two thirds plaster, one third dark glazing: the mean would land on a
        // muddy mid-tone that is on neither.
        let mut cells = vec![(200, 185, 160); 40];
        cells.extend(vec![(35, 40, 48); 20]);
        let (color, share) = dominant_color(&cells).unwrap();
        assert!(color.0 > 150, "picked the windows: {color:?}");
        assert!(share > 0.5, "share={share}");
    }

    #[test]
    fn too_few_cells_yields_nothing() {
        assert!(dominant_color(&[(200, 185, 160); 4]).is_none());
    }

    /// Paints a synthetic panorama by forward ray-tracing a wall.
    ///
    /// This deliberately derives the camera ray the opposite way round from
    /// `project`: pixel to direction to plane intersection, rather than world
    /// point to pixel. A sign error or a swapped axis in either one makes the
    /// painted region and the sampled region disagree, which is the whole point
    /// of rendering the fixture instead of asserting against `project` itself.
    fn render_wall_pano(
        cam: &CameraPose,
        wall_z: f64,
        wall_x: (f64, f64),
        wall_y: (f64, f64),
        wall_color: RGBTuple,
        sky: RGBTuple,
    ) -> image::RgbImage {
        let (w, h) = (1024u32, 512u32);
        let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([sky.0, sky.1, sky.2]));

        for py in 0..h {
            for px in 0..w {
                let u = (px as f64 + 0.5) / w as f64;
                let v = (py as f64 + 0.5) / h as f64;

                let bearing = (cam.compass_angle + (u - 0.5) * 360.0).to_radians();
                let pitch = ((0.5 - v) * 180.0).to_radians();

                // +X east, +Y up, north is -Z.
                let dir = (
                    bearing.sin() * pitch.cos(),
                    pitch.sin(),
                    -bearing.cos() * pitch.cos(),
                );
                if dir.2.abs() < 1e-9 {
                    continue;
                }
                let t = (wall_z - cam.z) / dir.2;
                if t <= 0.0 {
                    continue;
                }

                let hit_x = cam.x + t * dir.0;
                let hit_y = cam.y + t * dir.1;
                if (wall_x.0..=wall_x.1).contains(&hit_x) && (wall_y.0..=wall_y.1).contains(&hit_y)
                {
                    img.put_pixel(
                        px,
                        py,
                        image::Rgb([wall_color.0, wall_color.1, wall_color.2]),
                    );
                }
            }
        }
        img
    }

    #[test]
    fn a_synthetic_facade_round_trips_through_the_whole_pass() {
        // The south wall runs (20,10) -> (0,10), so its outward normal is +Z
        // and a camera has to stand south of it.
        let footprint = square_20x10();

        // Deliberately off to one side: a camera square-on to the wall would
        // still line up if the bearing sign were flipped.
        let cam = CameraPose::level(-10.0, CAMERA_HEIGHT_M, 25.0, 0.0);

        let plaster = (198, 180, 156);
        // Bright and blue, so `classify` rejects everything off the wall and
        // only genuine facade hits can reach the histogram.
        let sky = (150, 190, 240);

        let pano = Panorama {
            meta: crate::mapillary::api::PanoMeta {
                id: "synthetic".into(),
                lat: 0.0,
                lon: 0.0,
                compass_angle: 0.0,
                rotation: None,
                quality_score: 1.0,
                thumb_url: String::new(),
            },
            pixels: render_wall_pano(&cam, 10.0, (0.0, 20.0), (0.0, 9.0), plaster, sky),
        };

        let cameras = vec![(cam, &pano)];
        let sample = sample_building(1, &footprint, DEFAULT_HEIGHT_M, &cameras, 1.0, true)
            .expect("the wall should have been sampled");

        assert_eq!(sample.color, plaster, "recovered the wrong wall colour");
        assert_eq!(sample.walls_sampled, 1, "only the south wall is visible");
        assert!(
            sample.confidence > 0.9,
            "a uniform wall should be unambiguous, got {}",
            sample.confidence
        );
        assert_eq!(sample.rejects.sky, 0, "no sample should have hit sky");
    }

    /// A placeholder panorama for cases that only exercise view geometry.
    fn dummy_pano() -> Panorama {
        Panorama {
            meta: crate::mapillary::api::PanoMeta {
                id: "dummy".into(),
                lat: 0.0,
                lon: 0.0,
                compass_angle: 0.0,
                rotation: None,
                quality_score: 1.0,
                thumb_url: String::new(),
            },
            pixels: image::RgbImage::new(2, 1),
        }
    }

    fn pose_at(x: f64, z: f64) -> CameraPose {
        CameraPose::level(x, CAMERA_HEIGHT_M, z, 0.0)
    }

    #[test]
    fn view_selection_enforces_side_distance_and_incidence() {
        // South wall of the 20x10 square: runs (20,10) -> (0,10), normal +Z.
        let walls = wall_segments(&square_20x10(), 1.0);
        let south = walls
            .iter()
            .find(|w| w.nz > 0.5)
            .expect("the square should have a +Z wall");

        let pano = dummy_pano();
        let case = |x: f64, z: f64| {
            let cameras = vec![(pose_at(x, z), &pano)];
            !select_views(south, &cameras, 1.0).is_empty()
        };

        assert!(case(10.0, 25.0), "square-on at 15 m should be selected");
        assert!(
            !case(10.0, -15.0),
            "a camera behind the wall must never be selected"
        );
        assert!(
            !case(10.0, 200.0),
            "a camera past the distance ceiling must be dropped"
        );
        assert!(
            !case(10.0, 11.0),
            "a camera almost touching the wall must be dropped"
        );
        // 70 degrees off the normal, past the incidence limit.
        assert!(!case(10.0 + 41.0, 25.0), "a grazing view must be dropped");
    }

    #[test]
    fn a_camera_inside_the_footprint_sees_no_wall() {
        // Every wall's normal points away from an interior camera, so the
        // outward-side check should leave the building uncoloured.
        let pano = dummy_pano();
        let cameras = vec![(pose_at(10.0, 5.0), &pano)];
        assert!(
            sample_building(1, &square_20x10(), DEFAULT_HEIGHT_M, &cameras, 1.0, false).is_none()
        );
    }

    #[test]
    fn a_jumble_of_colours_yields_nothing() {
        // An even sweep across lightness: the cells share a hue but no lightness
        // cluster is big enough to call a wall colour, which is what a heavily
        // occluded facade looks like once the green and sky samples are gone.
        let cells: Vec<RGBTuple> = (0..60u8).map(|i| (i * 4, i * 4, i * 4)).collect();
        assert!(
            dominant_color(&cells).is_none(),
            "an even spread should not produce a confident colour"
        );
    }

    #[test]
    fn a_clear_majority_survives_some_spread() {
        // Two thirds of the cells on one plaster tone, the rest scattered: the
        // guard must not reject a facade just because it has some noise.
        let mut cells = vec![(198, 180, 156); 40];
        cells.extend((0..20u8).map(|i| (i * 8, i * 7, i * 6)));
        let (color, _) = dominant_color(&cells).expect("clear majority rejected");
        assert_eq!(color, (198, 180, 156));
    }
}
