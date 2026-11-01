//! Golf courses: putting greens, fairways, tees and bunkers on the course's turf, and a
//! flag where each hole ends.

use crate::args::Args;
use crate::block_definitions::*;
use crate::floodfill_cache::{in_hole, FloodFillCache, Holes, InnerRings};
use crate::ground_decoration::LOOSE_PLANTS;
use crate::land_cover::coord_hash;
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Turf {
    Green,
    Fairway,
    Tee,
    Rough,
    Bunker,
    Range,
}

fn turf(tags: &HashMap<String, String>) -> Option<Turf> {
    Some(match tags.get("golf")?.as_str() {
        "green" => Turf::Green,
        "fairway" => Turf::Fairway,
        "tee" => Turf::Tee,
        "rough" => Turf::Rough,
        "bunker" => Turf::Bunker,
        "driving_range" => Turf::Range,
        _ => return None,
    })
}

/// Golf ground this module draws. Hazards mapped as water stay water, and buildings and
/// paths keep their own handlers.
pub fn claims(tags: &HashMap<String, String>) -> bool {
    turf(tags).is_some()
        && !tags.contains_key("water")
        && !matches!(
            tags.get("natural").map(String::as_str),
            Some("water" | "wetland")
        )
        && !["building", "building:part", "highway"]
            .iter()
            .any(|k| tags.contains_key(*k))
}

/// Managed turf that trees and wild plants stay off; the rough is left to grow.
pub fn is_managed(tags: &HashMap<String, String>) -> bool {
    claims(tags) && turf(tags) != Some(Turf::Rough)
}

pub fn is_hole(tags: &HashMap<String, String>) -> bool {
    tags.get("golf").map(String::as_str) == Some("hole")
}

fn turf_block(kind: Turf, tags: &HashMap<String, String>) -> Block {
    match kind {
        Turf::Green => MOSS_BLOCK,
        Turf::Bunker => match tags.get("surface").map(String::as_str) {
            Some("gravel" | "fine_gravel") => GRAVEL,
            _ => SAND,
        },
        Turf::Fairway | Turf::Tee | Turf::Rough | Turf::Range => GRASS_BLOCK,
    }
}

pub fn generate_golf(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    golf_area(editor, way, args, flood_fill_cache, None);
}

pub fn generate_golf_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    let inners = InnerRings::of(rel);
    for member in rel
        .members
        .iter()
        .filter(|m| m.role == ProcessedMemberRole::Outer)
    {
        let holes = flood_fill_cache.holes_in(&member.way, &inners, args.timeout.as_ref());
        let way = ProcessedWay {
            id: member.way.id,
            nodes: member.way.nodes.clone(),
            tags: rel.tags.clone(),
        };
        golf_area(editor, &way, args, flood_fill_cache, holes.as_ref());
    }
}

fn golf_area(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    holes: Option<&Holes>,
) {
    let Some(kind) = turf(&way.tags) else {
        return;
    };
    let block = turf_block(kind, &way.tags);
    let fill = flood_fill_cache.get_or_compute(way, args.timeout.as_ref());
    let cells: HashSet<(i32, i32)> = fill
        .iter()
        .copied()
        .filter(|&(x, z)| !in_hole(holes, x, z))
        .collect();
    // The turf a smaller area inside already laid (a green in its fairway) stays.
    let over: &[Block] = &[GRASS_BLOCK, DIRT, COARSE_DIRT, PODZOL];
    for &(x, z) in &cells {
        editor.set_block(block, x, 0, z, Some(over), None);
        if kind != Turf::Rough {
            editor.set_block(AIR, x, 1, z, Some(LOOSE_PLANTS), None);
        }
    }
    // A fringe of longer grass collars the green.
    if kind == Turf::Green {
        for &(x, z) in &cells {
            let edge = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|(dx, dz)| !cells.contains(&(x + dx, z + dz)));
            if edge {
                editor.set_block(GRASS_BLOCK, x, 0, z, Some(&[MOSS_BLOCK]), None);
            }
        }
    }
    if kind == Turf::Tee {
        tee_markers(editor, way, &cells);
    }
}

/// Two markers across the front of the tee box, coloured for the tee they set.
fn tee_markers(editor: &mut WorldEditor, way: &ProcessedWay, cells: &HashSet<(i32, i32)>) {
    if cells.len() < 12 {
        return;
    }
    let n = cells.len() as f64;
    let cx = cells.iter().map(|c| c.0 as f64).sum::<f64>() / n;
    let cz = cells.iter().map(|c| c.1 as f64).sum::<f64>() / n;
    let Some((ux, uz)) = crate::element_processing::sport_pitches::longest_edge_dir(way) else {
        return;
    };
    let marker = match way.tags.get("tee").map(String::as_str) {
        Some("championship" | "black") => BLACK_WOOL,
        Some("men" | "white") => WHITE_WOOL,
        Some("women" | "ladies" | "red") => RED_WOOL,
        Some("senior" | "yellow" | "gold") => YELLOW_WOOL,
        _ => BLUE_WOOL,
    };
    // Across the long axis, a couple of blocks either side of the middle.
    let (vx, vz) = (-uz, ux);
    for side in [-2.0, 2.0] {
        let (x, z) = (
            (cx + vx * side).round() as i32,
            (cz + vz * side).round() as i32,
        );
        if cells.contains(&(x, z)) {
            editor.set_block(marker, x, 1, z, None, None);
        }
    }
}

/// The flagstick at the end of a hole: a white pole with a pennant at the top.
pub fn generate_hole_flag(editor: &mut WorldEditor, way: &ProcessedWay) {
    let (Some(first), Some(last)) = (way.nodes.first(), way.nodes.last()) else {
        return;
    };
    if way.nodes.len() < 2 || (first.x, first.z) == (last.x, last.z) {
        return;
    }
    let (x, z) = (last.x, last.z);
    for y in 1..=3 {
        editor.set_block(END_ROD, x, y, z, None, None);
    }
    // The pennant flies away from the tee.
    let prev = &way.nodes[way.nodes.len() - 2];
    let (dx, dz) = ((x - prev.x) as f64, (z - prev.z) as f64);
    let (fx, fz) = if dx.abs() >= dz.abs() {
        (dx.signum() as i32, 0)
    } else {
        (0, dz.signum() as i32)
    };
    let flag = [RED_WOOL, YELLOW_WOOL, WHITE_WOOL][(coord_hash(x, z) % 3) as usize];
    editor.set_block(flag, x + fx, 3, z + fz, None, None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::osm_parser::ProcessedNode;
    use std::path::PathBuf;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn turf_is_claimed_but_hazards_mapped_as_water_are_not() {
        assert!(claims(&tags(&[("golf", "green")])));
        assert!(claims(&tags(&[("golf", "fairway"), ("landuse", "grass")])));
        assert!(claims(&tags(&[("golf", "bunker"), ("natural", "sand")])));
        assert!(!claims(&tags(&[
            ("golf", "water_hazard"),
            ("natural", "water")
        ])));
        assert!(!claims(&tags(&[("golf", "green"), ("natural", "water")])));
        assert!(!claims(&tags(&[("golf", "cartpath"), ("highway", "path")])));
        assert!(!claims(&tags(&[
            ("golf", "clubhouse"),
            ("building", "yes")
        ])));
    }

    #[test]
    fn rough_is_left_to_grow() {
        assert!(is_managed(&tags(&[("golf", "fairway")])));
        assert!(!is_managed(&tags(&[("golf", "rough")])));
    }

    #[test]
    fn a_hole_ends_in_a_flagstick() {
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor = WorldEditor::new(PathBuf::from("/dev/null/unused"), &xzbbox, llbbox);
        let node = |id: u64, x: i32, z: i32| ProcessedNode {
            id,
            tags: HashMap::new(),
            x,
            z,
        };
        let hole = ProcessedWay {
            id: 1,
            nodes: vec![node(1, 10, 50), node(2, 60, 50), node(3, 80, 50)],
            tags: tags(&[("golf", "hole")]),
        };
        generate_hole_flag(&mut editor, &hole);
        for y in 1..=3 {
            assert!(editor.check_for_block(80, y, 50, Some(&[END_ROD])));
        }
        assert!(editor.check_for_block(81, 3, 50, Some(&[RED_WOOL, YELLOW_WOOL, WHITE_WOOL])));
        assert!(
            !editor.check_for_block(10, 1, 50, Some(&[END_ROD])),
            "no flag on the tee"
        );
    }
}
