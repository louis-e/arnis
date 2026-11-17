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
//! mapped where nothing stands) is left alone. A roof whose top reaches the
//! 51 m the survey records at most may stand higher, whatever its eaves: such
//! a building waits for Overture's height and gets what was measured only
//! where Overture has none.

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::osm_parser::{ProcessedElement, ProcessedWay};

/// Share of an outline's blocks that must be roof for its measurement to count.
const MIN_COVER: f64 = 0.6;
/// Outlines smaller than this are left to the generator.
const MIN_BLOCKS: usize = 8;
/// The highest height the survey records (255 fifths of a metre): a roof there
/// may stand any height above it.
const SATURATED: f32 = 51.0;

/// A building whose roof's top reached the highest height the survey records:
/// its place in the elements and what was measured, for [`apply_deferred`].
pub struct Deferred {
    index: usize,
    height: f32,
    flat: bool,
}

/// Fill measured heights in place; returns how many buildings got one, and
/// the buildings left for after Overture's heights.
pub fn apply(
    elements: &mut [ProcessedElement],
    ground: &Ground,
    xzbbox: &XZBBox,
) -> (usize, Vec<Deferred>) {
    let mut deferred = Vec::new();
    if !ground.has_laser() || !ground.laser_heights_wanted() {
        return (0, deferred);
    }
    let mut filled = 0;
    for (index, el) in elements.iter_mut().enumerate() {
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
        if let Some(Measured { height, flat, top }) = measure(way, ground, xzbbox) {
            if top >= SATURATED {
                deferred.push(Deferred {
                    index,
                    height,
                    flat,
                });
            } else {
                fill(way, height, flat);
                filled += 1;
            }
        }
    }
    (filled, deferred)
}

/// The deferred buildings that still have no height (Overture had none for
/// them) get what was measured; returns how many.
pub fn apply_deferred(elements: &mut [ProcessedElement], deferred: Vec<Deferred>) -> usize {
    let mut filled = 0;
    for d in deferred {
        let Some(ProcessedElement::Way(way)) = elements.get_mut(d.index) else {
            continue;
        };
        if !way.tags.contains_key("height") && !way.tags.contains_key("building:levels") {
            fill(way, d.height, d.flat);
            filled += 1;
        }
    }
    filled
}

fn fill(way: &mut ProcessedWay, height: f32, flat: bool) {
    let kind = way
        .tags
        .get("building")
        .map(String::as_str)
        .unwrap_or("yes");
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
}

/// What the laser saw of one outline's roof.
struct Measured {
    /// The height its walls get, in metres: the top of a flat roof, else the
    /// eaves.
    height: f32,
    flat: bool,
    /// The top of the roof (its 95th percentile), in metres.
    top: f32,
}

/// What the laser saw of one outline's roof, if it saw enough of it.
fn measure(way: &ProcessedWay, ground: &Ground, xzbbox: &XZBBox) -> Option<Measured> {
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
    let flat = top - eave < 1.0;
    Some(Measured {
        height: if flat { top } else { eave.max(2.0) },
        flat,
        top,
    })
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

    /// A 20 x 20 block world whose laser saw a roof `byte` fifths of a metre up
    /// over (2, 2) .. (12, 12), and a way around it.
    fn roof(byte: u8) -> (Ground, ProcessedWay, XZBBox) {
        roof_with_eaves(byte, byte)
    }

    /// As [`roof`], with the outermost ring of the roof at `eave` fifths of a
    /// metre.
    fn roof_with_eaves(byte: u8, eave: u8) -> (Ground, ProcessedWay, XZBBox) {
        let (w, h) = (20usize, 20usize);
        let mut cells = vec![[0u8; 5]; w * h];
        for z in 2..13 {
            for x in 2..13 {
                let edge = x == 2 || x == 12 || z == 2 || z == 12;
                let q = if edge { eave } else { byte };
                cells[z * w + x] = [super::super::classify::BUILDING, q, 0, 0, 0];
            }
        }
        let features = super::super::Features {
            garden: false,
            trees: false,
            heights: true,
        };
        let mut ground = Ground::new_flat(0);
        ground.set_world_dims(w, h);
        ground.set_survey(Some(super::super::SurveyData::from_cells(
            cells, w, h, features,
        )));
        let node = |id, x, z| crate::osm_parser::ProcessedNode {
            id,
            tags: Default::default(),
            x,
            z,
        };
        let way = ProcessedWay {
            id: 1,
            nodes: vec![
                node(1, 2, 2),
                node(2, 12, 2),
                node(3, 12, 12),
                node(4, 2, 12),
                node(1, 2, 2),
            ],
            tags: Default::default(),
        };
        let xzbbox = XZBBox::rect_from_xz_lengths(19.0, 19.0).unwrap();
        (ground, way, xzbbox)
    }

    #[test]
    fn a_roof_at_the_highest_height_recorded_waits_for_overture() {
        let tagged = |els: &[ProcessedElement]| els[0].tags().get("height").cloned();
        let office = |mut w: ProcessedWay| {
            w.tags.insert("building".into(), "office".into());
            ProcessedElement::Way(w)
        };
        let (ground, way, xzbbox) = roof(150);
        let mut els = vec![office(way)];
        assert_eq!(apply(&mut els, &ground, &xzbbox).0, 1);
        assert_eq!(tagged(&els).as_deref(), Some("30.0"));

        let (ground, way, xzbbox) = roof(255);
        let mut els = vec![office(way)];
        let (filled, deferred) = apply(&mut els, &ground, &xzbbox);
        assert_eq!((filled, deferred.len()), (0, 1));
        assert_eq!(tagged(&els), None);
        // Overture knew its height: that stays.
        let mut known = els.clone();
        if let ProcessedElement::Way(w) = &mut known[0] {
            w.tags.insert("height".into(), "112".into());
        }
        let (_, deferred) = apply(&mut els, &ground, &xzbbox);
        assert_eq!(apply_deferred(&mut known, deferred), 0);
        assert_eq!(tagged(&known).as_deref(), Some("112"));
        // It did not: the 51 m measured.
        let (_, deferred) = apply(&mut els, &ground, &xzbbox);
        assert_eq!(apply_deferred(&mut els, deferred), 1);
        assert_eq!(tagged(&els).as_deref(), Some("51.0"));

        // A tower whose top reached 51 m waits too, though its eaves (here a
        // lower ring around it) are far below: those would make it 30 m.
        let (ground, way, xzbbox) = roof_with_eaves(255, 150);
        let mut els = vec![office(way)];
        let (filled, deferred) = apply(&mut els, &ground, &xzbbox);
        assert_eq!((filled, deferred.len()), (0, 1));
        assert_eq!(apply_deferred(&mut els, deferred), 1);
        assert_eq!(tagged(&els).as_deref(), Some("30.0"));
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
