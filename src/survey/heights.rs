//! Measured heights for buildings OpenStreetMap gives no height.
//!
//! The laser's roof points inside a building's outline say how tall it stands.
//! Only buildings with neither `height` nor `building:levels` are touched, and
//! nothing mapped is overwritten, like Overture's height hints:
//!
//! * a roof whose points span under a metre is flat (`roof:shape=flat`), and its
//!   top is the wall height; a house's only if its middle stands no higher than
//!   its edges, since a small or gently pitched roof spans under a metre too;
//! * otherwise the eaves (the low end of the roof points) are, and the generator
//!   puts its own roof on top as it does for any untagged building, so shapes
//!   stay its choice while the walls are measured.
//!
//! A building is measured by its main part: a lower wing, extension or garage
//! under the same outline does not set its walls, and a block's or a hall's
//! roof flat over most of it is flat whatever its lower parts do.
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
    scale: f64,
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
        if let Some(Measured { height, flat, top }) = measure(way, ground, xzbbox, scale) {
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

/// Kinds of building that are houses, whatever their size.
const HOUSES: &[&str] = &[
    "house",
    "detached",
    "semidetached_house",
    "terrace",
    "bungalow",
    "villa",
    "farm",
    "cabin",
];
/// Any building up to this footprint is house-sized, in square metres.
const HOUSE_FOOTPRINT_M2: f64 = 250.0;

/// What the laser saw of one outline's roof, if it saw enough of it.
fn measure(way: &ProcessedWay, ground: &Ground, xzbbox: &XZBBox, scale: f64) -> Option<Measured> {
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
    // The roof along the outline and inside it.
    let (mut rim, mut middle): (Vec<f32>, Vec<f32>) = (Vec::new(), Vec::new());
    for z in z0 as i32..=z1 as i32 {
        for x in x0 as i32..=x1 as i32 {
            let (cx, cz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
            if !inside(&pts, cx, cz) {
                continue;
            }
            blocks += 1;
            let coord = XZPoint::new(x - xzbbox.min_x(), z - xzbbox.min_z());
            if let Some(c) = ground.laser_cell(coord) {
                if c.kind == super::Kind::Building && c.height > 1.5 {
                    roof.push(c.height);
                    let on_rim = [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)]
                        .iter()
                        .any(|&(dx, dz)| !inside(&pts, cx + dx, cz + dz));
                    if on_rim { &mut rim } else { &mut middle }.push(c.height);
                }
            }
        }
    }
    if blocks < MIN_BLOCKS || (roof.len() as f64) < MIN_COVER * blocks as f64 {
        return None;
    }
    roof.sort_by(f32::total_cmp);
    let at = |v: &[f32], q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    let top = at(&roof, 0.95);
    let eave = at(main_roof(&roof), 0.1);
    // Half the roof within a metre of its top: a block's or a hall's flat roof
    // over its main part (a house's could be a lean-to, and stays pitched).
    let flat_main = at(&roof, 0.5) >= top - 1.0;
    // A house's roof spans under a metre when it is small or gently pitched: it
    // is flat only where its middle stands no higher than its rim, as a flat
    // roof's does (or lower, inside a parapet), and left to the generator where
    // too small to have a middle. A pitched roof drawn flat loses its eaves.
    let house = way
        .tags
        .get("building")
        .is_some_and(|k| HOUSES.contains(&k.as_str()))
        || blocks as f64 / (scale * scale) <= HOUSE_FOOTPRINT_M2;
    let level = match (median(&mut middle), median(&mut rim)) {
        (Some(m), Some(r)) => m - r < 0.1,
        _ => false,
    };
    let flat = if house {
        top - eave < 1.0 && level
    } else {
        top - eave < 1.0 || flat_main
    };
    Some(Measured {
        height: if flat { top } else { eave.max(2.0) },
        flat,
        top,
    })
}

/// Storeys a home with walls this tall holds: floors of 2.8 m over a raised
/// ground floor. `None` for buildings that are not homes or are lower than a
/// storey and a half, which keep their height in metres.
///
/// An untagged building of more than [`HOME_STOREYS`] is built on 4 m storeys,
/// not a home's: it gets as many of those as its walls hold, and where that
/// would make it a low building again (built on a home's storeys after all),
/// its height in metres.
fn storeys(kind: &str, wall_m: f32) -> Option<u32> {
    const HOMES: &[&str] = &[
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
    if wall_m < 3.5 {
        return None;
    }
    let levels = (((wall_m - 0.4) / 2.8).round() as u32).max(1);
    if HOMES.contains(&kind) {
        return Some(levels);
    }
    if kind != "yes" {
        return None;
    }
    if levels <= HOME_STOREYS {
        return Some(levels);
    }
    let tall = ((wall_m - 1.0) / TALL_STOREY_M).round() as u32;
    (tall > HOME_STOREYS).then_some(tall)
}

/// Most storeys an untagged building is built on a home's storeys at.
const HOME_STOREYS: u32 = 7;
/// Storey an untagged building taller than that is built on, in metres.
const TALL_STOREY_M: f32 = 4.0;

/// Height step that parts a lower wing from the main roof, in metres.
const WING_STEP_M: f32 = 1.5;

/// The points of a building's main roof, of all its roof points sorted: those
/// above the last step of [`WING_STEP_M`] within a twentieth of the points in
/// their lower half, when at least half stand above it. A wing, an extension
/// or a garage under the outline makes such a step; a pitched roof, even a
/// steep one, climbs over many points and makes none.
fn main_roof(roof: &[f32]) -> &[f32] {
    let n = roof.len();
    let k = (n / 20).max(1);
    let mut from = 0;
    for i in n / 20..=n / 2 {
        if i + k < n && roof[i + k] - roof[i] >= WING_STEP_M {
            from = i + 1;
        }
    }
    if from > 0 && n - from >= n / 2 {
        &roof[from..]
    } else {
        roof
    }
}

fn median(v: &mut [f32]) -> Option<f32> {
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied()
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
        // An untagged tower is built on 4 m storeys, so it gets that many; where
        // those would be a home's few again, its height.
        assert_eq!(storeys("yes", 32.4), Some(8));
        assert_eq!(storeys("yes", 45.0), Some(11));
        assert_eq!(storeys("yes", 24.0), None);
        assert_eq!(storeys("yes", 20.0), Some(7));
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
            roofs: false,
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

    /// A roof over (2, 2) .. (12, 12) at `rim` fifths of a metre along its
    /// outline and `middle` inside it, on a building of `kind` in a run at
    /// `scale`, after the heights are applied.
    fn rimmed(kind: &str, rim: u8, middle: u8, scale: f64) -> Vec<ProcessedElement> {
        let (_, mut way, xzbbox) = roof(0);
        way.tags.insert("building".into(), kind.into());
        let (w, h) = (20usize, 20usize);
        let mut cells = vec![[0u8; 5]; w * h];
        for z in 2..12 {
            for x in 2..12 {
                let edge = x == 2 || x == 11 || z == 2 || z == 11;
                let q = if edge { rim } else { middle };
                cells[z * w + x] = [super::super::classify::BUILDING, q, 0, 0, 0];
            }
        }
        let features = super::super::Features {
            garden: false,
            trees: false,
            heights: true,
            roofs: false,
        };
        let mut ground = Ground::new_flat(0);
        ground.set_world_dims(w, h);
        ground.set_survey(Some(super::super::SurveyData::from_cells(
            cells, w, h, features,
        )));
        let mut els = vec![ProcessedElement::Way(way)];
        apply(&mut els, &ground, &xzbbox, scale);
        els
    }

    #[test]
    fn a_lower_wing_does_not_set_the_walls() {
        let sorted = |mut v: Vec<f32>| {
            v.sort_by(f32::total_cmp);
            v
        };
        let at = |v: &[f32], q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
        // A two-storey house, eaves at 6 m and ridge at 10 m, with a flat
        // one-storey extension at 3 m over a quarter of it.
        let mut house: Vec<f32> = (0..75).map(|i| 6.0 + 4.0 * i as f32 / 74.0).collect();
        house.extend([3.0; 25]);
        let house = sorted(house);
        assert!((at(main_roof(&house), 0.1) - 6.4).abs() < 0.1);
        // A steep roof, 8 m from eaves to ridge, keeps its eaves.
        let steep = sorted((0..100).map(|i| 6.0 + 8.0 * i as f32 / 99.0).collect());
        assert_eq!(main_roof(&steep).len(), 100);
        // Where the lower part is most of the building, it is the building.
        let mut hall: Vec<f32> = vec![5.0; 60];
        hall.extend([12.0; 40]);
        assert_eq!(main_roof(&sorted(hall)).len(), 100);
    }

    #[test]
    fn a_block_flat_over_most_of_it_is_flat() {
        let shape = |els: &[ProcessedElement]| els[0].tags().get("roof:shape").cloned();
        let height = |els: &[ProcessedElement]| els[0].tags().get("height").cloned();
        // An office block flat at 13 m whose rim the laser saw at 8 m: flat, 13 m.
        let block = rimmed("office", 40, 65, 0.5);
        assert_eq!(shape(&block).as_deref(), Some("flat"));
        assert_eq!(height(&block).as_deref(), Some("13.0"));
        // A shed or extension the same shape at house size keeps its roof.
        assert_eq!(shape(&rimmed("yes", 40, 65, 1.0)), None);
    }

    #[test]
    fn a_house_is_flat_only_where_its_middle_is_no_higher_than_its_rim() {
        let shape = |els: &[ProcessedElement]| els[0].tags().get("roof:shape").cloned();
        // A gently pitched house: its points span 0.8 m, its middle higher.
        assert_eq!(shape(&rimmed("house", 30, 34, 1.0)), None);
        // So does a small untagged building's.
        assert_eq!(shape(&rimmed("yes", 30, 34, 1.0)), None);
        // A flat roof, and one inside a parapet, are flat.
        assert_eq!(
            shape(&rimmed("house", 30, 30, 1.0)).as_deref(),
            Some("flat")
        );
        assert_eq!(
            shape(&rimmed("house", 33, 30, 1.0)).as_deref(),
            Some("flat")
        );
        // A hall's roof under a metre is flat whatever its middle does: here
        // 100 blocks at half scale, 400 m².
        let hall = rimmed("industrial", 30, 34, 0.5);
        assert_eq!(shape(&hall).as_deref(), Some("flat"));
        // A house is a house at any size.
        assert_eq!(shape(&rimmed("terrace", 30, 34, 0.5)), None);
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
        assert_eq!(apply(&mut els, &ground, &xzbbox, 1.0).0, 1);
        assert_eq!(tagged(&els).as_deref(), Some("30.0"));

        let (ground, way, xzbbox) = roof(255);
        let mut els = vec![office(way)];
        let (filled, deferred) = apply(&mut els, &ground, &xzbbox, 1.0);
        assert_eq!((filled, deferred.len()), (0, 1));
        assert_eq!(tagged(&els), None);
        // Overture knew its height: that stays.
        let mut known = els.clone();
        if let ProcessedElement::Way(w) = &mut known[0] {
            w.tags.insert("height".into(), "112".into());
        }
        let (_, deferred) = apply(&mut els, &ground, &xzbbox, 1.0);
        assert_eq!(apply_deferred(&mut known, deferred), 0);
        assert_eq!(tagged(&known).as_deref(), Some("112"));
        // It did not: the 51 m measured.
        let (_, deferred) = apply(&mut els, &ground, &xzbbox, 1.0);
        assert_eq!(apply_deferred(&mut els, deferred), 1);
        assert_eq!(tagged(&els).as_deref(), Some("51.0"));

        // A tower whose top reached 51 m waits too, though a lower part of it
        // (here a ring at 30 m along two sides) is far below.
        let (ground, way, xzbbox) = roof_with_eaves(255, 150);
        let mut els = vec![office(way)];
        let (filled, deferred) = apply(&mut els, &ground, &xzbbox, 1.0);
        assert_eq!((filled, deferred.len()), (0, 1));
        assert_eq!(apply_deferred(&mut els, deferred), 1);
        // That lower part is a wing of a fifth of it: the tower is its main part.
        assert_eq!(tagged(&els).as_deref(), Some("51.0"));
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
