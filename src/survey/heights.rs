//! Measured heights for buildings OpenStreetMap gives no height.
//!
//! The laser's roof points inside a building's outline say how tall it stands.
//! Only buildings with neither `height` nor `building:levels` are touched, and
//! nothing mapped is overwritten, like Overture's height hints:
//!
//! * a roof whose points span under a metre is flat (`roof:shape=flat`), and its
//!   top is the wall height;
//! * otherwise the eaves (the low end of the roof points) are, and the generator
//!   puts its own roof on top as it does for any untagged building, so shapes
//!   stay its choice while the walls are measured.
//!
//! Homes get the storeys that wall height holds (`building:levels`), not the
//! metres: Arnis draws a storey in three or four blocks plus a raised ground
//! floor, so a two-storey house with its eaves at 5.8 m would otherwise show one
//! storey and a gap. Halls, garages and anything under a storey and a half keep
//! the measured metres.
//!
//! An outline the laser mostly saw as something else (built after the scan, or
//! mapped where nothing stands) is left alone.

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::osm_parser::{ProcessedElement, ProcessedWay};

/// Share of an outline's blocks that must be roof for its measurement to count.
const MIN_COVER: f64 = 0.6;
/// Outlines smaller than this are left to the generator.
const MIN_BLOCKS: usize = 8;

/// Fill measured heights in place; returns how many buildings got one.
pub fn apply(elements: &mut [ProcessedElement], ground: &Ground, xzbbox: &XZBBox) -> usize {
    if !ground.has_laser() || !ground.laser_heights_wanted() {
        return 0;
    }
    let mut filled = 0;
    for el in elements.iter_mut() {
        let ProcessedElement::Way(way) = el else {
            continue;
        };
        let t = &way.tags;
        if !t.contains_key("building")
            || t.contains_key("height")
            || t.contains_key("building:levels")
            || t.contains_key("building:part")
        {
            continue;
        }
        if let Some((height, flat)) = measure(way, ground, xzbbox) {
            let kind = t.get("building").map(String::as_str).unwrap_or("yes");
            match storeys(kind, height) {
                Some(levels) => way
                    .tags
                    .insert("building:levels".to_string(), levels.to_string()),
                None => way
                    .tags
                    .insert("height".to_string(), format!("{height:.1}")),
            };
            if flat && !way.tags.contains_key("roof:shape") {
                way.tags
                    .insert("roof:shape".to_string(), "flat".to_string());
            }
            filled += 1;
        }
    }
    filled
}

/// (height in metres, flat roof) for one outline, if the laser saw it.
fn measure(way: &ProcessedWay, ground: &Ground, xzbbox: &XZBBox) -> Option<(f32, bool)> {
    let pts: Vec<(f64, f64)> = way
        .nodes
        .iter()
        .map(|n| (f64::from(n.x), f64::from(n.z)))
        .collect();
    if pts.len() < 4 {
        return None;
    }
    let (x0, x1) = minmax(pts.iter().map(|p| p.0));
    let (z0, z1) = minmax(pts.iter().map(|p| p.1));
    let mut blocks = 0usize;
    let mut roof: Vec<f32> = Vec::new();
    for z in z0 as i32..=z1 as i32 {
        for x in x0 as i32..=x1 as i32 {
            if !inside(&pts, f64::from(x) + 0.5, f64::from(z) + 0.5) {
                continue;
            }
            blocks += 1;
            let coord = XZPoint::new(x - xzbbox.min_x(), z - xzbbox.min_z());
            if let Some(c) = ground.laser_cell(coord) {
                if c.kind == super::Kind::Building && c.height > 1.5 {
                    roof.push(c.height);
                }
            }
        }
    }
    if blocks < MIN_BLOCKS || (roof.len() as f64) < MIN_COVER * blocks as f64 {
        return None;
    }
    roof.sort_by(f32::total_cmp);
    let at = |q: f64| roof[((roof.len() - 1) as f64 * q).round() as usize];
    let (eave, top) = (at(0.1), at(0.95));
    if top - eave < 1.0 {
        Some((top, true))
    } else {
        Some((eave.max(2.0), false))
    }
}

/// Storeys a home with walls this tall holds: floors of 2.8 m over a raised
/// ground floor. `None` for buildings that are not homes or are lower than a
/// storey and a half, which keep their height in metres.
fn storeys(kind: &str, wall_m: f32) -> Option<u32> {
    const HOMES: &[&str] = &[
        "yes",
        "house",
        "detached",
        "semidetached_house",
        "terrace",
        "bungalow",
        "villa",
        "residential",
        "apartments",
        "dormitory",
    ];
    if !HOMES.contains(&kind) || wall_m < 3.5 {
        return None;
    }
    Some((((wall_m - 0.4) / 2.8).round() as u32).max(1))
}

fn minmax(v: impl Iterator<Item = f64>) -> (f64, f64) {
    v.fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(x), b.max(x)))
}

/// Even-odd point in polygon; the ring may repeat its first node at the end.
fn inside(ring: &[(f64, f64)], x: f64, z: f64) -> bool {
    let mut odd = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (xi, zi) = ring[i];
        let (xj, zj) = ring[j];
        if (zi > z) != (zj > z) && x < (xj - xi) * (z - zi) / (zj - zi) + xi {
            odd = !odd;
        }
        j = i;
    }
    odd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homes_get_storeys_halls_keep_metres() {
        assert_eq!(storeys("house", 5.8), Some(2));
        assert_eq!(storeys("terrace", 6.4), Some(2));
        assert_eq!(storeys("yes", 4.2), Some(1));
        assert_eq!(storeys("apartments", 14.5), Some(5));
        assert_eq!(storeys("house", 2.9), None);
        assert_eq!(storeys("industrial", 9.0), None);
        assert_eq!(storeys("garage", 4.0), None);
    }

    #[test]
    fn a_square_holds_its_centre_not_its_outside() {
        let sq = [
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ];
        assert!(inside(&sq, 5.5, 5.5));
        assert!(!inside(&sq, 10.5, 5.5));
    }
}
