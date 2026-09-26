pub mod web_mercator;

pub use web_mercator::WebMercatorProjection;

use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::coordinate_system::transformation::CoordTransformer;
use std::fmt;
use std::str::FromStr;

/// Trait for converting between WGS84 geographic coordinates and a projected
/// coordinate system used in Minecraft world generation.
#[allow(dead_code)]
pub trait Projection {
    /// Convert WGS84 latitude/longitude (degrees) to projected (x, z) in meters
    /// (or blocks, depending on scale).
    fn forward(&self, lat: f64, lon: f64) -> (f64, f64);

    /// Convert projected (x, z) back to WGS84 latitude/longitude (degrees).
    fn inverse(&self, x: f64, z: f64) -> (f64, f64);
}

/// Available map projection variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionKind {
    /// Web Mercator (EPSG:3857-like) projection with a local origin offset.
    WebMercator,
    /// Simple local coordinate system (no geographic projection).
    Local,
}

impl fmt::Display for ProjectionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectionKind::WebMercator => write!(f, "web_mercator"),
            ProjectionKind::Local => write!(f, "local"),
        }
    }
}

impl FromStr for ProjectionKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "web_mercator" | "webmercator" | "mercator" => Ok(ProjectionKind::WebMercator),
            "local" => Ok(ProjectionKind::Local),
            other => Err(format!("unknown projection kind: '{other}'")),
        }
    }
}

/// How one run maps lat/lon to block coordinates. Every consumer goes
/// through this so they cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectionSpec {
    pub kind: ProjectionKind,
    /// Web Mercator origin; `None` is the bbox centre.
    pub origin: Option<(f64, f64)>,
    pub scale: f64,
    /// Blocks of geometry kept past the world edge when clipping, so elements
    /// crossing it are built whole.
    pub clip_pad: i32,
}

impl ProjectionSpec {
    #[cfg(test)]
    pub fn local(scale: f64) -> Self {
        Self {
            kind: ProjectionKind::Local,
            origin: None,
            scale,
            clip_pad: 0,
        }
    }

    pub fn from_args(args: &crate::args::Args) -> Self {
        match &args.one_world_run {
            Some(run) => Self {
                kind: ProjectionKind::WebMercator,
                origin: Some((run.origin_lat, run.origin_lon)),
                scale: args.scale,
                clip_pad: crate::one_world::CLIP_PAD_BLOCKS,
            },
            None => Self {
                kind: args.projection,
                origin: None,
                scale: args.scale,
                clip_pad: 0,
            },
        }
    }

    pub fn mercator(&self, llbbox: &LLBBox) -> WebMercatorProjection {
        let (lat, lon) = self.origin.unwrap_or_else(|| {
            (
                (llbbox.min().lat() + llbbox.max().lat()) / 2.0,
                (llbbox.min().lng() + llbbox.max().lng()) / 2.0,
            )
        });
        WebMercatorProjection::new(lat, lon, self.scale)
    }

    pub fn transformer(&self, llbbox: &LLBBox) -> Result<(CoordTransformer, XZBBox), String> {
        match self.kind {
            ProjectionKind::Local => CoordTransformer::llbbox_to_xzbbox(llbbox, self.scale),
            ProjectionKind::WebMercator => {
                CoordTransformer::with_projection(llbbox, self.scale, self.mercator(llbbox))
            }
        }
    }

    pub fn clip_bbox(&self, xzbbox: &XZBBox) -> XZBBox {
        if self.clip_pad <= 0 {
            return xzbbox.clone();
        }
        XZBBox::rect_from_min_max(
            xzbbox.min_x().saturating_sub(self.clip_pad),
            xzbbox.min_z().saturating_sub(self.clip_pad),
            xzbbox.max_x().saturating_add(self.clip_pad),
            xzbbox.max_z().saturating_add(self.clip_pad),
        )
        .unwrap_or_else(|_| xzbbox.clone())
    }
}

pub const CHUNK_BLOCKS: i32 = 16;

pub const WORLD_BORDER_BLOCKS: f64 = 29_999_984.0;

/// Checked before the edges are cast to integers.
pub(crate) fn check_projected_edges(edges: [f64; 4]) -> Result<(), String> {
    if edges.iter().any(|v| !v.is_finite()) {
        return Err("bounding box is outside the Web Mercator domain".to_string());
    }
    if edges.iter().any(|v| v.abs() > WORLD_BORDER_BLOCKS) {
        return Err(
            "this area lies past the Minecraft world border in this world's frame".to_string(),
        );
    }
    Ok(())
}

/// Rounds a projected edge to the block grid. Within 1e-6 of an integer it is
/// that integer, which absorbs the float noise of an inverse-projected edge.
pub(crate) fn snap_edge(v: f64, ceil: bool) -> i32 {
    let r = v.round();
    if (v - r).abs() < 1e-6 {
        return r as i32;
    }
    if ceil {
        v.ceil() as i32
    } else {
        v.floor() as i32
    }
}

/// Snaps a bbox outward to whole chunks in a Web Mercator frame. Returns the
/// inclusive block rectangle and the lat/lon bbox covering exactly that
/// rectangle (the axes are separable, so the two describe the same area).
pub fn snap_bbox_to_chunks(
    proj: &WebMercatorProjection,
    llbbox: &LLBBox,
) -> Result<(XZBBox, LLBBox), String> {
    let x_w = proj.x_for_lon(llbbox.min().lng());
    let x_e = proj.x_for_lon(llbbox.max().lng());
    let z_n = proj.z_for_lat(llbbox.max().lat());
    let z_s = proj.z_for_lat(llbbox.min().lat());
    check_projected_edges([x_w, x_e, z_n, z_s])?;

    let floor_chunk = |v: f64| (snap_edge(v, false) as f64 / CHUNK_BLOCKS as f64).floor() as i32;
    let ceil_chunk = |v: f64| (snap_edge(v, true) as f64 / CHUNK_BLOCKS as f64).ceil() as i32;

    let cx0 = floor_chunk(x_w);
    let mut cx1 = ceil_chunk(x_e);
    let cz0 = floor_chunk(z_n);
    let mut cz1 = ceil_chunk(z_s);
    if cx1 <= cx0 {
        cx1 = cx0 + 1;
    }
    if cz1 <= cz0 {
        cz1 = cz0 + 1;
    }

    let min_x = cx0 * CHUNK_BLOCKS;
    let max_x_excl = cx1 * CHUNK_BLOCKS;
    let min_z = cz0 * CHUNK_BLOCKS;
    let max_z_excl = cz1 * CHUNK_BLOCKS;

    let xzbbox = XZBBox::rect_from_min_max(min_x, min_z, max_x_excl - 1, max_z_excl - 1)?;
    let effective = LLBBox::new(
        proj.lat_for_z(max_z_excl as f64),
        proj.lon_for_x(min_x as f64),
        proj.lat_for_z(min_z as f64),
        proj.lon_for_x(max_x_excl as f64),
    )?;
    Ok((xzbbox, effective))
}

/// The lat/lon bbox covering a block rectangle, outer edges included.
pub fn llbbox_for_rect(proj: &WebMercatorProjection, xzbbox: &XZBBox) -> Result<LLBBox, String> {
    LLBBox::new(
        proj.lat_for_z(xzbbox.max_z() as f64 + 1.0),
        proj.lon_for_x(xzbbox.min_x() as f64),
        proj.lat_for_z(xzbbox.min_z() as f64),
        proj.lon_for_x(xzbbox.max_x() as f64 + 1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_projection_kind_display() {
        assert_eq!(ProjectionKind::WebMercator.to_string(), "web_mercator");
        assert_eq!(ProjectionKind::Local.to_string(), "local");
    }

    #[test]
    fn test_projection_kind_from_str() {
        assert_eq!(
            "web_mercator".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::WebMercator
        );
        assert_eq!(
            "webmercator".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::WebMercator
        );
        assert_eq!(
            "mercator".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::WebMercator
        );
        assert_eq!(
            "local".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::Local
        );
        assert_eq!(
            "LOCAL".parse::<ProjectionKind>().unwrap(),
            ProjectionKind::Local
        );
    }

    #[test]
    fn test_projection_kind_from_str_invalid() {
        assert!("unknown".parse::<ProjectionKind>().is_err());
    }

    #[test]
    fn test_projection_kind_roundtrip() {
        for kind in [ProjectionKind::WebMercator, ProjectionKind::Local] {
            let s = kind.to_string();
            let parsed: ProjectionKind = s.parse().unwrap();
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn snapped_rect_sits_on_chunk_edges_and_covers_the_request() {
        let proj = WebMercatorProjection::new(48.1372, 11.5755, 1.0);
        let req = LLBBox::new(48.130, 11.560, 48.145, 11.590).unwrap();
        let (rect, eff) = snap_bbox_to_chunks(&proj, &req).unwrap();
        assert_eq!(rect.min_x().rem_euclid(16), 0);
        assert_eq!(rect.min_z().rem_euclid(16), 0);
        assert_eq!((rect.max_x() + 1).rem_euclid(16), 0);
        assert_eq!((rect.max_z() + 1).rem_euclid(16), 0);
        assert!(eff.min().lat() <= req.min().lat());
        assert!(eff.max().lat() >= req.max().lat());
        assert!(eff.min().lng() <= req.min().lng());
        assert!(eff.max().lng() >= req.max().lng());
        assert!(rect.min_x() < 0 && rect.max_x() > 0);
        assert!(rect.min_z() < 0 && rect.max_z() > 0);
    }

    #[test]
    fn effective_bbox_reprojects_to_the_same_rect() {
        let proj = WebMercatorProjection::new(-33.8688, 151.2093, 0.7);
        let req = LLBBox::new(-33.880, 151.190, -33.850, 151.230).unwrap();
        let (rect, eff) = snap_bbox_to_chunks(&proj, &req).unwrap();
        let spec = ProjectionSpec {
            kind: ProjectionKind::WebMercator,
            origin: Some((-33.8688, 151.2093)),
            scale: 0.7,
            clip_pad: 0,
        };
        let (_, again) = spec.transformer(&eff).unwrap();
        assert_eq!(
            (again.min_x(), again.min_z(), again.max_x(), again.max_z()),
            (rect.min_x(), rect.min_z(), rect.max_x(), rect.max_z())
        );
        assert_eq!(llbbox_for_rect(&proj, &rect), Ok(eff));
    }

    #[test]
    fn adjacent_requests_tile_without_gap_or_overlap() {
        let proj = WebMercatorProjection::new(52.52, 13.405, 1.0);
        let west = LLBBox::new(52.510, 13.390, 52.530, 13.405).unwrap();
        let (rw, effw) = snap_bbox_to_chunks(&proj, &west).unwrap();
        let east = LLBBox::new(52.510, effw.max().lng(), 52.530, 13.420).unwrap();
        let (re, _) = snap_bbox_to_chunks(&proj, &east).unwrap();
        assert_eq!(re.min_x(), rw.max_x() + 1);
        assert_eq!(re.min_z(), rw.min_z());
    }

    #[test]
    fn a_degenerate_request_still_gets_one_chunk() {
        let proj = WebMercatorProjection::new(10.0, 10.0, 1.0);
        let req = LLBBox::new(10.0, 10.0, 10.000001, 10.000001).unwrap();
        let (rect, _) = snap_bbox_to_chunks(&proj, &req).unwrap();
        assert_eq!(rect.max_x() - rect.min_x() + 1, 16);
        assert_eq!(rect.max_z() - rect.min_z() + 1, 16);
    }

    #[test]
    fn snap_edge_absorbs_float_noise_only() {
        assert_eq!(snap_edge(16.0000000001, false), 16);
        assert_eq!(snap_edge(15.9999999999, false), 16);
        assert_eq!(snap_edge(15.9999999999, true), 16);
        assert_eq!(snap_edge(15.4, false), 15);
        assert_eq!(snap_edge(15.4, true), 16);
        assert_eq!(snap_edge(-3.7, false), -4);
        assert_eq!(snap_edge(-3.7, true), -3);
    }

    #[test]
    fn clip_bbox_grows_by_the_pad() {
        let spec = ProjectionSpec {
            kind: ProjectionKind::WebMercator,
            origin: Some((0.0, 0.0)),
            scale: 1.0,
            clip_pad: 64,
        };
        let rect = XZBBox::rect_from_min_max(-160, 0, 159, 319).unwrap();
        let clip = spec.clip_bbox(&rect);
        assert_eq!(
            (clip.min_x(), clip.min_z(), clip.max_x(), clip.max_z()),
            (-224, -64, 223, 383)
        );
        assert_eq!(ProjectionSpec::local(1.0).clip_bbox(&rect).min_x(), -160);
    }
}
