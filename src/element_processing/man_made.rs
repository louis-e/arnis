use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::connected_blocks::{
    connected_iron_bars, is_connectable, place_connected,
};
use crate::element_processing::waterfront;
use crate::floodfill_cache::{CoordinateBitmap, FloodFillCache};
use crate::mapillary::geometry::parse_length_m;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::{world_max_y, WorldEditor};
use fastnbt::Value;
use std::collections::{HashMap, HashSet};

/// Masts up to this many blocks are a single pole.
const MAST_POLE_MAX_HEIGHT: i32 = 12;
/// Masts and chimneys from this tall carry red and white aviation bands.
const MAST_BANDED_MIN_M: f64 = 100.0;
/// Tube masts from this many blocks are thicker than one block.
const MAST_THICK_TUBE_HEIGHT: i32 = 60;
/// Chimneys up to this many blocks are a one-block stack.
const CHIMNEY_STACK_MAX_HEIGHT: i32 = 6;
/// Untagged chimneys from this tall are concrete rather than brick.
const CHIMNEY_CONCRETE_MIN_M: f64 = 80.0;
/// A peak's summit cross and a mapped cross closer than this are the same cross.
const SUMMIT_CROSS_MERGE_M: f64 = 30.0;
const CROSS_MAX_HEIGHT: i32 = 40;
/// Crosses from this tall are built solid rather than from posts.
const CROSS_SOLID_MIN_HEIGHT: i32 = 10;

pub fn generate_man_made(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &CoordinateBitmap,
) {
    // Skip if 'layer' or 'level' is negative in the tags
    if let Some(layer) = element.tags().get("layer") {
        if layer.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(level) = element.tags().get("level") {
        if level.parse::<i32>().unwrap_or(0) < 0 {
            return;
        }
    }

    if let Some(man_made_type) = element.tags().get("man_made") {
        match man_made_type.as_str() {
            "pier" => waterfront::generate_pier(editor, element, flood_fill_cache, args),
            "breakwater" | "groyne" => {
                waterfront::generate_breakwater(editor, element, flood_fill_cache, args);
            }
            "antenna" | "mast" => generate_mast(editor, element, args, building_footprints),
            "chimney" => generate_chimney(editor, element, args, building_footprints),
            "water_well" => generate_water_well(editor, element),
            "water_tower" | "silo" | "storage_tank" => {
                generate_tank_structure(editor, element, args);
            }
            "lighthouse" => place_lighthouse_way(editor, element),
            "crane" => {
                if let Some((x, z)) = anchor_point(element) {
                    crate::structures::crane::place_mapped(
                        editor,
                        x,
                        z,
                        element.tags(),
                        building_footprints,
                    );
                }
            }
            _ => {} // Unknown man_made type, ignore
        }
    }
}

/// Stamp the bundled lighthouse at the centroid of a lighthouse way/footprint.
fn place_lighthouse_way(editor: &mut WorldEditor, element: &ProcessedElement) {
    if let ProcessedElement::Way(way) = element {
        if way.nodes.is_empty() {
            return;
        }
        let (mut sx, mut sz) = (0i64, 0i64);
        for nd in &way.nodes {
            sx += nd.x as i64;
            sz += nd.z as i64;
        }
        let n = way.nodes.len() as i64;
        crate::structures::lighthouse::place(editor, (sx / n) as i32, (sz / n) as i32);
    }
}

/// Tree crowns a mast or chimney stands through.
const CANOPY: &[Block] = &[
    OAK_LEAVES,
    BIRCH_LEAVES,
    SPRUCE_LEAVES,
    DARK_OAK_LEAVES,
    JUNGLE_LEAVES,
    ACACIA_LEAVES,
    CHERRY_LEAVES,
    MANGROVE_LEAVES,
    AZALEA_LEAVES,
];

fn clear_canopy(editor: &mut WorldEditor, x: i32, y: i32, z: i32) {
    editor.set_block_absolute(AIR, x, y, z, Some(CANOPY), None);
}

/// Where a point feature stands: the node, or the middle of a way's nodes.
fn anchor_point(element: &ProcessedElement) -> Option<(i32, i32)> {
    if let ProcessedElement::Node(node) = element {
        return Some((node.x, node.z));
    }
    let nodes: Vec<(i32, i32)> = element.nodes().map(|n| (n.x, n.z)).collect();
    // A closed ring repeats its first node; count it once.
    let ring = if nodes.len() > 2 && nodes.first() == nodes.last() {
        &nodes[..nodes.len() - 1]
    } else {
        &nodes[..]
    };
    if ring.is_empty() {
        return None;
    }
    let n = ring.len() as i64;
    let (sx, sz) = ring.iter().fold((0i64, 0i64), |(sx, sz), &(x, z)| {
        (sx + x as i64, sz + z as i64)
    });
    Some(((sx / n) as i32, (sz / n) as i32))
}

/// Where each `man_made=crane` stands, for construction sites to leave room for.
pub fn mapped_crane_points(elements: &[ProcessedElement]) -> Vec<(i32, i32)> {
    elements
        .iter()
        .filter(|e| e.tags().get("man_made").map(String::as_str) == Some("crane"))
        .filter_map(anchor_point)
        .collect()
}

/// Top block of the roof at (x, z) when it lies on a building.
fn roof_top(editor: &WorldEditor, footprints: &CoordinateBitmap, x: i32, z: i32) -> Option<i32> {
    if !footprints.contains(x, z) {
        return None;
    }
    let ground = editor.get_absolute_y(x, 0, z);
    editor.highest_block_between(x, z, ground + 1, world_max_y())
}

/// A `height`-style tag in metres.
fn tag_metres(element: &ProcessedElement, key: &str) -> Option<f64> {
    element
        .tags()
        .get(key)
        .and_then(|v| parse_length_m(v))
        .filter(|m| *m > 0.0 && *m < 1000.0)
}

/// Blocks from `base` up to the world ceiling, at least `min`.
fn fit_height(metres: f64, scale: f64, base: i32, min: i32) -> i32 {
    ((metres * scale).round() as i32).clamp(min, (world_max_y() - base - 1).max(min))
}

/// Blocks a feature rises above `base`. OSM heights count from the ground, so on a roof
/// a height taller than the building reaches that height overall; a shorter one is the
/// feature's own.
fn rise_above(metres: f64, scale: f64, ground: i32, base: i32, min: i32) -> i32 {
    let blocks = (metres * scale).round() as i32;
    let roof = base - ground;
    let own = if roof > 0 && blocks > roof + 1 {
        blocks - roof
    } else {
        blocks
    };
    fit_height(f64::from(own), 1.0, base, min)
}

/// Mast shapes worth telling apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MastShape {
    /// Iron bar pole with a lightning rod tip.
    Pole,
    /// Square lattice of corner legs and bracing.
    Lattice,
    /// Solid steel or concrete tube.
    Tube,
}

fn mast_shape(element: &ProcessedElement, height: i32, on_roof: bool) -> MastShape {
    if on_roof || height <= MAST_POLE_MAX_HEIGHT {
        return MastShape::Pole;
    }
    match element.tags().get("tower:construction").map(String::as_str) {
        Some("monopole" | "tube" | "tubular" | "guyed_tube" | "concrete") => MastShape::Tube,
        _ => MastShape::Lattice,
    }
}

/// Masts and antennas, from a rooftop pole to a banded lattice, on the roof they are mapped on.
fn generate_mast(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    footprints: &CoordinateBitmap,
) {
    let Some((x, z)) = anchor_point(element) else {
        return;
    };
    let tags = element.tags();
    let antenna = tags.get("man_made").map(String::as_str) == Some("antenna");
    let lighting = tags.get("tower:type").map(String::as_str) == Some("lighting");
    let roof = roof_top(editor, footprints, x, z);
    let ground = editor.get_absolute_y(x, 0, z);
    let base = roof.unwrap_or(ground);
    let default_m = match (roof.is_some(), antenna, lighting) {
        (true, true, _) => 3.0,
        (true, false, _) => 6.0,
        (false, _, true) => 20.0,
        (false, true, false) => 10.0,
        (false, false, false) => 30.0,
    };
    let metres = tag_metres(element, "height").unwrap_or(default_m);
    let height = rise_above(metres, args.scale, ground, base, 2);
    let top = base + height;

    match mast_shape(element, height, roof.is_some()) {
        MastShape::Pole => {
            for y in (base + 1)..top {
                editor.set_block_absolute(IRON_BARS, x, y, z, Some(CANOPY), None);
            }
            editor.set_block_absolute(LIGHTNING_ROD, x, top, z, Some(CANOPY), None);
            if height >= 6 {
                mast_head(editor, x, z, top - 2, 1, lighting);
            }
        }
        MastShape::Tube => {
            let banded = metres >= MAST_BANDED_MIN_M;
            // Tall tubes get a plus-shaped section so they do not read as a wire.
            let section: &[(i32, i32)] = if height >= MAST_THICK_TUBE_HEIGHT {
                &[(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)]
            } else {
                &[(0, 0)]
            };
            for y in (base + 1)..(top - 2) {
                let band = (y - base - 1) * 7 / (top - 2 - base).max(1);
                let block = match (banded, band % 2 == 0) {
                    (true, true) => RED_CONCRETE,
                    (true, false) => WHITE_CONCRETE,
                    (false, _) => LIGHT_GRAY_CONCRETE,
                };
                for &(dx, dz) in section {
                    editor.set_block_absolute(block, x + dx, y, z + dz, Some(CANOPY), None);
                }
            }
            for y in (top - 2)..top {
                editor.set_block_absolute(IRON_BARS, x, y, z, Some(CANOPY), None);
            }
            editor.set_block_absolute(LIGHTNING_ROD, x, top, z, Some(CANOPY), None);
            mast_head(
                editor,
                x,
                z,
                top - 4,
                1 + i32::from(section.len() > 1),
                lighting,
            );
            mast_cabinet(editor, x, z);
        }
        MastShape::Lattice => {
            // Tall masts carry aviation bands, seven to the height.
            let banded = metres >= MAST_BANDED_MIN_M;
            let lattice_top = top - 4;
            for y in (base + 1)..=lattice_top {
                let band = (y - base - 1) * 7 / (lattice_top - base).max(1);
                let corner = match (banded, band % 2 == 0) {
                    (true, true) => RED_CONCRETE,
                    (true, false) => WHITE_CONCRETE,
                    (false, _) => ANDESITE_WALL,
                };
                lattice_ring(editor, x, z, y, corner);
            }
            for y in (lattice_top + 1)..top {
                editor.set_block_absolute(IRON_BARS, x, y, z, Some(CANOPY), None);
            }
            editor.set_block_absolute(LIGHTNING_ROD, x, top, z, Some(CANOPY), None);
            mast_head(editor, x, z, lattice_top - 2, 2, lighting);
            mast_cabinet(editor, x, z);
        }
    }
}

/// One storey of a 3x3 lattice: legs in the corners, bars between them.
fn lattice_ring(editor: &mut WorldEditor, x: i32, z: i32, y: i32, corner: Block) {
    let walls = corner.name().ends_with("_wall");
    for (dx, dz) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
        if walls {
            clear_canopy(editor, x + dx, y, z + dz);
            place_connected(editor, corner, x + dx, y, z + dz);
        } else {
            editor.set_block_absolute(corner, x + dx, y, z + dz, Some(CANOPY), None);
        }
    }
    for (dx, dz) in [(0, -1), (0, 1), (-1, 0), (1, 0)] {
        if walls {
            clear_canopy(editor, x + dx, y, z + dz);
            place_connected(editor, IRON_BARS, x + dx, y, z + dz);
        } else {
            // Bars join the solid legs either side of them.
            let bars = connected_iron_bars(dx != 0, dx != 0, dz != 0, dz != 0);
            editor.set_block_with_properties_absolute(bars, x + dx, y, z + dz, Some(CANOPY), None);
        }
    }
}

/// Sector panels, or floodlights on a lighting mast, two rows high around the top.
fn mast_head(editor: &mut WorldEditor, x: i32, z: i32, y: i32, reach: i32, lighting: bool) {
    let block = if lighting {
        SEA_LANTERN
    } else {
        WHITE_CONCRETE
    };
    let rows = if lighting { 1 } else { 2 };
    for (dx, dz) in [(0, -reach), (0, reach), (-reach, 0), (reach, 0)] {
        for dy in 0..rows {
            editor.set_block_absolute(block, x + dx, y + dy, z + dz, Some(CANOPY), None);
        }
    }
}

/// Equipment cabinet at the foot of a ground mast.
fn mast_cabinet(editor: &mut WorldEditor, x: i32, z: i32) {
    for dz in -1..=0 {
        for y in 1..=2 {
            editor.set_block(LIGHT_GRAY_CONCRETE, x + 3, y, z + dz, Some(CANOPY), None);
        }
    }
}

/// Chimneys: a stack on a roof, a tapered hollow shaft with a sooted crown on the ground.
fn generate_chimney(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    footprints: &CoordinateBitmap,
) {
    let Some((x, z)) = anchor_point(element) else {
        return;
    };
    let roof = roof_top(editor, footprints, x, z);
    let ground = editor.get_absolute_y(x, 0, z);
    let base = roof.unwrap_or(ground);
    let default_m = if roof.is_some() { 2.0 } else { 40.0 };
    let metres = tag_metres(element, "height").unwrap_or(default_m);
    let height = rise_above(metres, args.scale, ground, base, 2);
    let material = element
        .tags()
        .get("building:material")
        .or_else(|| element.tags().get("material"))
        .map(|m| m.to_ascii_lowercase());
    let body = match material.as_deref() {
        Some("brick") => BRICK,
        Some("concrete" | "reinforced_concrete" | "cement") => LIGHT_GRAY_CONCRETE,
        Some("metal" | "steel" | "iron") => LIGHT_GRAY_CONCRETE,
        _ if metres >= CHIMNEY_CONCRETE_MIN_M => LIGHT_GRAY_CONCRETE,
        _ => BRICK,
    };

    if height <= CHIMNEY_STACK_MAX_HEIGHT {
        for y in (base + 1)..=(base + height) {
            editor.set_block_absolute(body, x, y, z, Some(CANOPY), None);
        }
        return;
    }

    // Base radius from the outline or `diameter`, else from the height; the top is narrower.
    let mapped = TankFootprint::from_element(element);
    let outlined = element.nodes().count() >= 3;
    let base_r = if outlined {
        // The widest circle the outline holds, so a long or narrow one is not overrun.
        let pts: Vec<(i32, i32)> = element.nodes().map(|n| (n.x, n.z)).collect();
        let span = |axis: fn(&(i32, i32)) -> i32| {
            pts.iter().map(axis).max().unwrap_or(0) - pts.iter().map(axis).min().unwrap_or(0)
        };
        let (span_x, span_z) = (span(|p| p.0), span(|p| p.1));
        f64::from(span_x.min(span_z)) / 2.0 - 0.5
    } else {
        tag_metres(element, "diameter")
            .map(|d| d * args.scale / 2.0)
            .unwrap_or(1.0 + f64::from(height) / 16.0)
    }
    .clamp(1.0, 12.0);
    let top_r = (base_r * 0.6).max(1.0);
    let banded = metres >= MAST_BANDED_MIN_M && body != BRICK;
    let band = (height / 18).max(1);
    for i in 0..height {
        let y = base + 1 + i;
        let t = f64::from(i) / f64::from(height);
        let r = base_r + (top_r - base_r) * t;
        let from_top = height - 1 - i;
        let block = if from_top < 2 {
            POLISHED_BLACKSTONE
        } else if banded && from_top < 2 + 3 * band {
            // Red, white, red below the crown.
            if ((from_top - 2) / band) % 2 == 0 {
                RED_CONCRETE
            } else {
                WHITE_CONCRETE
            }
        } else {
            body
        };
        let reach = (r + 0.5).ceil() as i32;
        // Kept to the outline, with a cell of slack so the wall stays closed along it.
        let on_outline = |cx: i32, cz: i32| {
            !outlined
                || mapped.cells.is_empty()
                || [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .any(|(ox, oz)| mapped.cells.contains(&(cx + ox, cz + oz)))
        };
        for dz in -reach..=reach {
            for dx in -reach..=reach {
                let d2 = f64::from(dx * dx + dz * dz);
                if d2 <= (r + 0.5).powi(2) && d2 > (r - 0.5).powi(2) && on_outline(x + dx, z + dz) {
                    editor.set_block_absolute(block, x + dx, y, z + dz, Some(CANOPY), None);
                }
            }
        }
    }
}

/// Stone well with a roofed winch frame, or a hand pump over a basin when `pump=*` is set.
fn generate_water_well(editor: &mut WorldEditor, element: &ProcessedElement) {
    let Some((x, z)) = anchor_point(element) else {
        return;
    };
    let pump = element.tags().get("pump").is_some_and(|p| p != "no");
    let base = editor.get_absolute_y(x, 0, z);
    if pump {
        for y in 1..=2 {
            editor.set_block_absolute(BLACKSTONE_WALL, x, base + y, z, None, None);
        }
        editor.set_block_with_properties_absolute(
            BlockWithProperties::new(
                LEVER,
                Some(Value::Compound(HashMap::from([
                    ("face".to_string(), Value::String("floor".to_string())),
                    ("facing".to_string(), Value::String("east".to_string())),
                    ("powered".to_string(), Value::String("false".to_string())),
                ]))),
            ),
            x,
            base + 3,
            z,
            None,
            None,
        );
        editor.set_block_with_properties_absolute(
            BlockWithProperties::new(
                WATER_CAULDRON,
                Some(Value::Compound(HashMap::from([(
                    "level".to_string(),
                    Value::String("2".to_string()),
                )]))),
            ),
            x + 1,
            base + 1,
            z,
            None,
            None,
        );
        return;
    }

    // Shaft of water inside a two-high stone ring.
    for dz in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dz == 0 {
                for y in (base - 2)..=base {
                    editor.set_block_absolute(WATER, x, y, z, None, Some(&[]));
                }
            } else {
                editor.set_block_absolute(COBBLESTONE, x + dx, base, z + dz, None, Some(&[]));
                editor.set_block_absolute(COBBLESTONE, x + dx, base + 1, z + dz, None, None);
            }
        }
    }
    // Posts on the ring carry the beam the chain hangs from, under a little gable.
    for dx in [-1, 1] {
        for y in 2..=4 {
            editor.set_block_absolute(OAK_FENCE, x + dx, base + y, z, None, None);
        }
    }
    editor.set_block_with_properties_absolute(
        BlockWithProperties::new(
            SPRUCE_LOG,
            Some(Value::Compound(HashMap::from([(
                "axis".to_string(),
                Value::String("x".to_string()),
            )]))),
        ),
        x,
        base + 5,
        z,
        None,
        None,
    );
    for dx in [-2, -1, 1, 2] {
        editor.set_block_absolute(SPRUCE_PLANKS, x + dx, base + 5, z, None, None);
    }
    for dx in -2..=2 {
        for (dz, facing) in [(-1, StairFacing::South), (1, StairFacing::North)] {
            let stair = create_stair_with_properties(SPRUCE_STAIRS, facing, StairShape::Straight);
            editor.set_block_with_properties_absolute(stair, x + dx, base + 5, z + dz, None, None);
        }
        editor.set_block_absolute(SPRUCE_SLAB, x + dx, base + 6, z, None, None);
    }
    for y in 3..=4 {
        editor.set_block_with_properties_absolute(
            BlockWithProperties::new(
                CHAIN_X,
                Some(Value::Compound(HashMap::from([(
                    "axis".to_string(),
                    Value::String("y".to_string()),
                )]))),
            ),
            x,
            base + y,
            z,
            None,
            None,
        );
    }
}

/// Peaks tagged with a summit cross that is not mapped as its own `man_made=cross`.
pub fn is_summit_cross(tags: &HashMap<String, String>) -> bool {
    matches!(
        tags.get("natural").map(String::as_str),
        Some("peak" | "volcano" | "hill")
    ) && tags.get("summit:cross").map(String::as_str) == Some("yes")
        && tags.get("man_made").map(String::as_str) != Some("cross")
}

/// Drops `summit:cross` from peaks that have a mapped cross beside them, so it stands once.
pub fn drop_doubled_summit_crosses(elements: &mut [ProcessedElement], scale: f64) -> usize {
    let reach = (SUMMIT_CROSS_MERGE_M * scale).max(1.0);
    let crosses: Vec<(i32, i32)> = elements
        .iter()
        .filter_map(|e| match e {
            ProcessedElement::Node(n)
                if n.tags.get("man_made").map(String::as_str) == Some("cross") =>
            {
                Some((n.x, n.z))
            }
            _ => None,
        })
        .collect();
    if crosses.is_empty() {
        return 0;
    }
    let mut dropped = 0;
    for element in elements.iter_mut() {
        let ProcessedElement::Node(node) = element else {
            continue;
        };
        if !is_summit_cross(&node.tags) {
            continue;
        }
        // In floats, so peaks far apart on a big map cannot overflow the square.
        let near = crosses.iter().any(|&(cx, cz)| {
            (f64::from(cx) - f64::from(node.x)).hypot(f64::from(cz) - f64::from(node.z)) <= reach
        });
        if near {
            node.tags.remove("summit:cross");
            dropped += 1;
        }
    }
    dropped
}

/// A cross: fence posts with a joined arm, a solid one when tall, on a cairn at a summit.
pub fn generate_cross(editor: &mut WorldEditor, node: &ProcessedNode, args: &Args, summit: bool) {
    let element = ProcessedElement::Node(node.clone());
    let default_m = if summit { 5.0 } else { 6.0 };
    let base_ground = editor.get_absolute_y(node.x, 0, node.z);
    let metres = tag_metres(&element, "height").unwrap_or(default_m);
    let height = fit_height(metres, args.scale, base_ground, 3).min(CROSS_MAX_HEIGHT);
    let material = node.tags.get("material").map(|m| m.to_ascii_lowercase());
    let solid = height >= CROSS_SOLID_MIN_HEIGHT;
    let block = match (material.as_deref(), solid) {
        (Some("metal" | "steel" | "iron" | "aluminium"), false) => IRON_BARS,
        (Some("metal" | "steel" | "iron" | "aluminium"), true) => IRON_BLOCK,
        (Some("concrete" | "stone"), _) => WHITE_CONCRETE,
        (_, false) => DARK_OAK_FENCE,
        (_, true) => DARK_OAK_PLANKS,
    };
    let mut base = base_ground;
    if summit {
        editor.set_block_absolute(COBBLESTONE, node.x, base + 1, node.z, None, None);
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            editor.set_block_absolute(
                COBBLESTONE_SLAB,
                node.x + dx,
                base + 1,
                node.z + dz,
                None,
                None,
            );
        }
        base += 1;
    }
    let arm_y = base + (height * 2 + 2) / 3;
    let half = (height / 5).max(1);
    let along_x = node.id.is_multiple_of(2);
    let connected = is_connectable(block);
    let put = |editor: &mut WorldEditor, x: i32, y: i32, z: i32| {
        if connected {
            place_connected(editor, block, x, y, z);
        } else {
            editor.set_block_absolute(block, x, y, z, None, None);
        }
    };
    for y in (base + 1)..=(base + height) {
        put(editor, node.x, y, node.z);
    }
    for k in -half..=half {
        let (x, z) = if along_x {
            (node.x + k, node.z)
        } else {
            (node.x, node.z + k)
        };
        put(editor, x, arm_y, z);
    }
}

/// Polygon-aware footprint summary for tank-style structures (water_tower,
/// silo, storage_tank). For node elements `nodes` is a single point and
/// `radius` defaults to a small fixed value.
struct TankFootprint {
    center_x: i32,
    center_z: i32,
    /// Approximate radius in blocks. For polygon ways this is half the
    /// average of width and length of the bounding box.
    radius: f64,
    /// Cells *inside* the polygon. For node elements this is just the
    /// single centre cell. Used to clip the cylinder so it never extends
    /// past the OSM-mapped outline.
    cells: HashSet<(i32, i32)>,
}

impl TankFootprint {
    fn from_element(element: &ProcessedElement) -> Self {
        let nodes: Vec<(i32, i32)> = element.nodes().map(|n| (n.x, n.z)).collect();
        if nodes.is_empty() {
            return Self {
                center_x: 0,
                center_z: 0,
                radius: 2.0,
                cells: HashSet::new(),
            };
        }

        if nodes.len() < 3 {
            // Single-node mapping - use a default 5×5 footprint around the
            // point so tank structures still have visible bulk even when
            // mapped as a POI.
            let (cx, cz) = nodes[0];
            let mut cells = HashSet::new();
            for dx in -2..=2 {
                for dz in -2..=2 {
                    cells.insert((cx + dx, cz + dz));
                }
            }
            return Self {
                center_x: cx,
                center_z: cz,
                radius: 2.5,
                cells,
            };
        }

        let (mut min_x, mut max_x) = (i32::MAX, i32::MIN);
        let (mut min_z, mut max_z) = (i32::MAX, i32::MIN);
        for &(x, z) in &nodes {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_z = min_z.min(z);
            max_z = max_z.max(z);
        }
        let center_x = (min_x + max_x) / 2;
        let center_z = (min_z + max_z) / 2;
        let half_w = (max_x - min_x).max(1) as f64 / 2.0;
        let half_l = (max_z - min_z).max(1) as f64 / 2.0;
        let radius = (half_w + half_l) / 2.0;

        // Rasterize the polygon interior using a point-in-polygon test on
        // every cell of the bounding box. Tanks are small so this is cheap.
        let mut cells = HashSet::new();
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                if point_in_polygon(x, z, &nodes) {
                    cells.insert((x, z));
                }
            }
        }
        Self {
            center_x,
            center_z,
            radius,
            cells,
        }
    }

    /// Iterates the cells of a filled disc of `disc_radius` centred on
    /// the footprint, clipped to the polygon `cells`.
    fn cells_in_disc(&self, disc_radius: f64) -> impl Iterator<Item = (i32, i32)> + '_ {
        let r2 = disc_radius * disc_radius;
        let r_int = disc_radius.ceil() as i32 + 1;
        let cx = self.center_x;
        let cz = self.center_z;
        (-r_int..=r_int)
            .flat_map(move |dx| (-r_int..=r_int).map(move |dz| (dx, dz)))
            .filter(move |(dx, dz)| ((*dx as f64).powi(2) + (*dz as f64).powi(2)) <= r2)
            .map(move |(dx, dz)| (cx + dx, cz + dz))
            .filter(|cell| self.cells.contains(cell))
    }
}

/// Ray-cast point-in-polygon test sampling the cell centre.
fn point_in_polygon(px: i32, pz: i32, polygon: &[(i32, i32)]) -> bool {
    let px = px as f64 + 0.5;
    let pz = pz as f64 + 0.5;
    let mut inside = false;
    let n = polygon.len();
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (xi, zi) = polygon[i];
        let (xj, zj) = polygon[j];
        let zi = zi as f64;
        let zj = zj as f64;
        let xi = xi as f64;
        let xj = xj as f64;
        let intersect = ((zi > pz) != (zj > pz)) && (px < (xj - xi) * (pz - zi) / (zj - zi) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Reads a building height from `height=*` (in metres / blocks) with a
/// caller-supplied default. Stripping a trailing 'm' keeps OSM values like
/// `height=18m` working.
fn read_height(element: &ProcessedElement, default: i32, scale_factor: f64) -> i32 {
    element
        .tags()
        .get("height")
        .and_then(|s| s.trim_end_matches('m').trim().parse::<f64>().ok())
        .map(|h| (h * scale_factor).round() as i32)
        .unwrap_or(default)
        .max(3)
}

/// Public entry point used by [buildings::generate_buildings] when a way
/// is tagged as a tank structure (`man_made=*` or `building=*` of
/// `water_tower` / `silo` / `storage_tank`). Dispatches to the right
/// renderer based on the most specific tag available.
pub fn generate_tank_structure(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    // Skip relations and other elements that have no node geometry; the
    // renderers below would otherwise build at world origin (0, 0).
    if element.nodes().next().is_none() {
        return;
    }

    let pick_tank = |key: &str| {
        element
            .tags()
            .get(key)
            .map(|s| s.as_str())
            .filter(|k| matches!(*k, "water_tower" | "silo" | "storage_tank"))
    };
    let kind = pick_tank("man_made").or_else(|| pick_tank("building"));

    match kind {
        Some("water_tower") => generate_water_tower(editor, element, args),
        Some("silo") => generate_silo(editor, element, args),
        Some("storage_tank") => generate_storage_tank(editor, element, args),
        _ => {}
    }
}

/// Generate a water tower - a tall cylindrical / rectangular tank
/// elevated on legs. Polygon-aware: legs are placed at the polygon
/// corners (or 4 cardinal points for round mappings), and the tank
/// itself is a filled cylinder clipped to the polygon outline.
fn generate_water_tower(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    let footprint = TankFootprint::from_element(element);
    let total_height = read_height(element, 20, args.scale);
    // Lower 60% is the supports, upper 40% is the tank itself.
    let support_height = (total_height as f64 * 0.6).round() as i32;
    let tank_height = total_height - support_height;
    if tank_height < 2 {
        return;
    }

    // --- Support structure ---
    // Legs at the 4 cardinal extremes of the footprint. We pick cells
    // that sit roughly at radius * 0.85 from centre so the legs frame
    // the structure without poking outside the polygon.
    let leg_offset = (footprint.radius * 0.85).max(1.0).round() as i32;
    let leg_positions: [(i32, i32); 4] = [
        (-leg_offset, 0),
        (leg_offset, 0),
        (0, -leg_offset),
        (0, leg_offset),
    ];
    for &(dx, dz) in &leg_positions {
        let lx = footprint.center_x + dx;
        let lz = footprint.center_z + dz;
        // Only place legs on cells inside the polygon (so weird-shaped
        // mapped polygons don't get legs floating in air).
        if !footprint.cells.contains(&(lx, lz)) {
            continue;
        }
        for y in 0..support_height {
            editor.set_block(IRON_BLOCK, lx, y, lz, None, None);
        }
    }

    // Cross-bracing every 5 blocks of height - gives the tower its
    // characteristic lattice silhouette. Bracing follows the polygon
    // outline (bresenham between consecutive nodes) at the tier height.
    if let ProcessedElement::Way(way) = element {
        for tier_y in (5..support_height).step_by(5) {
            let mut prev: Option<(i32, i32)> = None;
            for node in &way.nodes {
                if let Some((px, pz)) = prev {
                    let pts = bresenham_line(px, tier_y, pz, node.x, tier_y, node.z);
                    for (bx, by, bz) in pts {
                        editor.set_block(SMOOTH_STONE, bx, by, bz, None, None);
                    }
                }
                prev = Some((node.x, node.z));
            }
        }
    }

    // Central pipe / column down to the ground.
    for y in 0..support_height {
        editor.set_block(
            POLISHED_ANDESITE,
            footprint.center_x,
            y,
            footprint.center_z,
            None,
            None,
        );
    }

    // Tank sits at one absolute Y so it stays level on sloped terrain.
    let tank_base =
        editor.get_ground_level(footprint.center_x, footprint.center_z) + support_height;
    for y in tank_base..(tank_base + tank_height) {
        for (cx, cz) in footprint.cells_in_disc(footprint.radius) {
            editor.set_block_absolute(POLISHED_ANDESITE, cx, y, cz, None, None);
        }
    }
    let cap_y = tank_base + tank_height;
    for (cx, cz) in footprint.cells_in_disc(footprint.radius) {
        editor.set_block_absolute(SMOOTH_STONE_SLAB, cx, cap_y, cz, None, None);
    }
}

/// Generate a silo - a tall cylindrical narrow tower. Polygon-aware
/// filled cylinder running floor-to-cap. Material follows
/// `building:material=*` (cement/stone → smooth stone; metal → iron;
/// default smooth stone).
fn generate_silo(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    let footprint = TankFootprint::from_element(element);
    let height = read_height(element, 25, args.scale);

    let material_tag = element
        .tags()
        .get("building:material")
        .or_else(|| element.tags().get("material"))
        .map(|s| s.to_lowercase());
    let body_block = match material_tag.as_deref() {
        Some("metal" | "steel" | "aluminium" | "aluminum" | "iron" | "tin") => IRON_BLOCK,
        Some("concrete" | "cement" | "reinforced_concrete") => GRAY_CONCRETE,
        _ => SMOOTH_STONE,
    };

    let base = editor.get_ground_level(footprint.center_x, footprint.center_z);
    for y in base..(base + height) {
        for (cx, cz) in footprint.cells_in_disc(footprint.radius) {
            editor.set_block_absolute(body_block, cx, y, cz, None, None);
        }
    }
    // Domed cap: small slab on top to suggest a rounded lid.
    for (cx, cz) in footprint.cells_in_disc(footprint.radius) {
        editor.set_block_absolute(SMOOTH_STONE_SLAB, cx, base + height, cz, None, None);
    }
}

/// Generate a storage tank - short squat cylinder. Material follows
/// `content=*` for a colour hint (water → light grey, oil → black,
/// gas/lng → white).
fn generate_storage_tank(editor: &mut WorldEditor, element: &ProcessedElement, args: &Args) {
    let footprint = TankFootprint::from_element(element);
    let default_h = ((footprint.radius * 1.2).round() as i32).max(6);
    let height =
        read_height(element, default_h, args.scale).min((footprint.radius * 1.5) as i32 + 4);

    let content = element.tags().get("content").map(|s| s.to_lowercase());
    let body_block = match content.as_deref() {
        Some("oil" | "fuel" | "diesel" | "petroleum" | "tar") => BLACK_TERRACOTTA,
        Some("gas" | "lng" | "methane" | "lpg") => WHITE_CONCRETE,
        Some("water" | "wastewater") => LIGHT_GRAY_CONCRETE,
        _ => SMOOTH_STONE,
    };

    let base = editor.get_ground_level(footprint.center_x, footprint.center_z);
    for y in base..(base + height) {
        for (cx, cz) in footprint.cells_in_disc(footprint.radius) {
            editor.set_block_absolute(body_block, cx, y, cz, None, None);
        }
    }
    // Flat lid.
    for (cx, cz) in footprint.cells_in_disc(footprint.radius) {
        editor.set_block_absolute(SMOOTH_STONE_SLAB, cx, base + height, cz, None, None);
    }
}

/// Returns true if the element is one of the tank-style structures
/// handled by [generate_tank_structure]. Used by the building dispatcher
/// to decide whether to short-circuit normal building generation.
pub fn is_tank_structure(way: &ProcessedWay) -> bool {
    matches!(
        way.tags.get("man_made").map(|s| s.as_str()),
        Some("water_tower" | "silo" | "storage_tank")
    ) || matches!(
        way.tags.get("building").map(|s| s.as_str()),
        Some("water_tower" | "silo" | "storage_tank")
    )
}

/// Generate man_made structures for node elements
pub fn generate_man_made_nodes(
    editor: &mut WorldEditor,
    node: &ProcessedNode,
    args: &Args,
    building_footprints: &CoordinateBitmap,
) {
    if let Some(man_made_type) = node.tags.get("man_made") {
        let element = ProcessedElement::Node(node.clone());

        match man_made_type.as_str() {
            "antenna" | "mast" => generate_mast(editor, &element, args, building_footprints),
            "chimney" => generate_chimney(editor, &element, args, building_footprints),
            "water_well" => generate_water_well(editor, &element),
            "water_tower" | "silo" | "storage_tank" => {
                generate_tank_structure(editor, &element, args);
            }
            "lighthouse" => crate::structures::lighthouse::place(editor, node.x, node.z),
            "dolphin" => waterfront::generate_dolphin(editor, node),
            "crane" => crate::structures::crane::place_mapped(
                editor,
                node.x,
                node.z,
                &node.tags,
                building_footprints,
            ),
            "cross" => {
                let summit = node.tags.get("summit:cross").map(String::as_str) == Some("yes")
                    || node.tags.contains_key("natural");
                generate_cross(editor, node, args, summit);
            }
            _ => {} // Unknown man_made type, ignore
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{tag_map, test_editor};
    use clap::Parser;

    fn args() -> Args {
        Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter())
    }

    fn node(id: u64, x: i32, z: i32, tags: &[(&str, &str)]) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: tag_map(tags),
            x,
            z,
        }
    }

    #[test]
    fn a_rooftop_antenna_stands_on_the_roof_at_its_tagged_height() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let ground = editor.get_absolute_y(10, 0, 10);
        let mut footprints = CoordinateBitmap::new(&xzbbox);
        footprints.set(10, 10);
        for y in (ground + 1)..=(ground + 8) {
            editor.set_block_absolute(STONE, 10, y, 10, None, None);
        }
        let antenna = node(1, 10, 10, &[("man_made", "antenna"), ("height", "4 m")]);
        generate_man_made_nodes(&mut editor, &antenna, &args(), &footprints);
        assert_eq!(
            editor.get_block_absolute(10, ground + 9, 10),
            Some(IRON_BARS)
        );
        assert_eq!(
            editor.get_block_absolute(10, ground + 12, 10),
            Some(LIGHTNING_ROD)
        );
        assert_eq!(editor.get_block_absolute(10, ground + 13, 10), None);
    }

    #[test]
    fn a_tall_mast_is_a_banded_lattice_with_its_tip_at_the_tagged_height() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let ground = editor.get_absolute_y(20, 0, 20);
        let mast = node(2, 20, 20, &[("man_made", "mast"), ("height", "120")]);
        generate_man_made_nodes(&mut editor, &mast, &args(), &CoordinateBitmap::new_empty());
        let top = (ground + 120).min(world_max_y() - 1);
        assert_eq!(editor.get_block_absolute(20, top, 20), Some(LIGHTNING_ROD));
        assert_eq!(
            editor.get_block_absolute(19, ground + 1, 19),
            Some(RED_CONCRETE)
        );
        assert_eq!(
            editor.get_block_absolute(20, ground + 1, 19),
            Some(IRON_BARS)
        );
        assert_eq!(editor.get_block_absolute(20, ground + 1, 20), None);
    }

    #[test]
    fn an_industrial_chimney_is_a_hollow_shaft_as_tall_as_tagged() {
        let xzbbox = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let ground = editor.get_absolute_y(30, 0, 30);
        let chimney = node(3, 30, 30, &[("man_made", "chimney"), ("height", "60")]);
        generate_man_made_nodes(
            &mut editor,
            &chimney,
            &args(),
            &CoordinateBitmap::new_empty(),
        );
        // The shaft tapers, so find its wall on each storey by walking out from the axis.
        let wall = |y: i32| (1..=8).find(|d| editor.block_exists_absolute(30, y, 30 + d));
        let (foot, crown) = (wall(ground + 1).unwrap(), wall(ground + 60).unwrap());
        assert!(crown < foot, "narrower at the top: {crown} vs {foot}");
        assert_eq!(wall(ground + 61), None);
        assert!(!editor.block_exists_absolute(30, ground + 30, 30), "hollow");
        assert_eq!(
            editor.get_block_absolute(30, ground + 60, 30 + crown),
            Some(POLISHED_BLACKSTONE)
        );
        assert_eq!(
            editor.get_block_absolute(30, ground + 1, 30 + foot),
            Some(BRICK)
        );
    }

    #[test]
    fn a_rooftop_chimney_reaches_its_tagged_height_from_the_ground() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let ground = editor.get_absolute_y(10, 0, 10);
        let mut footprints = CoordinateBitmap::new(&xzbbox);
        footprints.set(10, 10);
        for y in (ground + 1)..=(ground + 15) {
            editor.set_block_absolute(STONE, 10, y, 10, None, None);
        }
        // Twenty metres from the ground on a fifteen-block building: five above the roof.
        let tall = node(9, 10, 10, &[("man_made", "chimney"), ("height", "20")]);
        generate_man_made_nodes(&mut editor, &tall, &args(), &footprints);
        assert!(editor.block_exists_absolute(10, ground + 20, 10));
        assert!(!editor.block_exists_absolute(10, ground + 21, 10));
        // A height below the roof is the stack's own.
        let mut editor = test_editor(&xzbbox);
        for y in (ground + 1)..=(ground + 15) {
            editor.set_block_absolute(STONE, 10, y, 10, None, None);
        }
        let short = node(10, 10, 10, &[("man_made", "chimney"), ("height", "3")]);
        generate_man_made_nodes(&mut editor, &short, &args(), &footprints);
        assert!(editor.block_exists_absolute(10, ground + 18, 10));
        assert!(!editor.block_exists_absolute(10, ground + 19, 10));
    }

    #[test]
    fn a_mapped_chimney_stays_inside_its_outline() {
        let xzbbox = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let ground = editor.get_absolute_y(30, 0, 30);
        // A long, narrow chimney outline: 6 wide, 20 long.
        let chimney = ProcessedElement::Way(ProcessedWay {
            id: 11,
            nodes: [(27, 20), (33, 20), (33, 40), (27, 40), (27, 20)]
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: 1100 + i as u64,
                    tags: HashMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags: tag_map(&[("man_made", "chimney"), ("height", "40")]),
        });
        generate_man_made(
            &mut editor,
            &chimney,
            &args(),
            &FloodFillCache::new(),
            &CoordinateBitmap::new_empty(),
        );
        let y = ground + 5;
        let placed: Vec<(i32, i32)> = (15..=45)
            .flat_map(|x| (10..=50).map(move |z| (x, z)))
            .filter(|&(x, z)| editor.block_exists_absolute(x, y, z))
            .collect();
        assert!(!placed.is_empty(), "the shaft is there");
        assert!(
            placed
                .iter()
                .all(|&(x, z)| (26..=34).contains(&x) && (19..=41).contains(&z)),
            "nothing past the outline: {placed:?}"
        );
    }

    #[test]
    fn the_well_roof_rests_on_its_posts() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let base = editor.get_absolute_y(20, 0, 20);
        let well = node(4, 20, 20, &[("man_made", "water_well")]);
        generate_man_made_nodes(&mut editor, &well, &args(), &CoordinateBitmap::new_empty());
        for dx in [-1, 1] {
            assert_eq!(
                editor.get_block_absolute(20 + dx, base + 1, 20),
                Some(COBBLESTONE)
            );
            for y in 2..=4 {
                assert_eq!(
                    editor.get_block_absolute(20 + dx, base + y, 20),
                    Some(OAK_FENCE)
                );
            }
            assert_eq!(
                editor.get_block_absolute(20 + dx, base + 5, 20),
                Some(SPRUCE_PLANKS)
            );
        }
        assert_eq!(editor.get_block_absolute(20, base, 20), Some(WATER));
    }

    #[test]
    fn masts_and_chimneys_wait_for_the_building_parts_they_stand_on() {
        use crate::osm_parser::get_priority;
        let part = ProcessedElement::Way(ProcessedWay {
            id: 1,
            nodes: Vec::new(),
            tags: tag_map(&[("building:part", "yes")]),
        });
        for kind in ["antenna", "mast", "chimney"] {
            let feature = ProcessedElement::Node(node(2, 0, 0, &[("man_made", kind)]));
            assert!(get_priority(&feature) > get_priority(&part), "{kind}");
        }
        // A chimney mapped as a building is still drawn with the buildings.
        let building = ProcessedElement::Node(node(
            3,
            0,
            0,
            &[("man_made", "chimney"), ("building", "yes")],
        ));
        assert!(get_priority(&building) < get_priority(&part));
    }

    #[test]
    fn a_summit_cross_mapped_twice_stands_once() {
        let mut elements = vec![
            ProcessedElement::Node(node(
                5,
                0,
                0,
                &[("natural", "peak"), ("summit:cross", "yes")],
            )),
            ProcessedElement::Node(node(6, 10, 10, &[("man_made", "cross")])),
            ProcessedElement::Node(node(
                7,
                200,
                0,
                &[("natural", "peak"), ("summit:cross", "yes")],
            )),
        ];
        assert_eq!(drop_doubled_summit_crosses(&mut elements, 1.0), 1);
        assert!(!is_summit_cross(elements[0].tags()));
        assert!(is_summit_cross(elements[2].tags()));
    }

    #[test]
    fn a_summit_cross_has_a_joined_arm_on_a_cairn() {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let base = editor.get_absolute_y(20, 0, 20);
        let peak = node(8, 20, 20, &[("natural", "peak"), ("summit:cross", "yes")]);
        generate_cross(&mut editor, &peak, &args(), true);
        assert_eq!(
            editor.get_block_absolute(20, base + 1, 20),
            Some(COBBLESTONE)
        );
        assert_eq!(
            editor.get_block_absolute(20, base + 6, 20),
            Some(DARK_OAK_FENCE)
        );
        assert_eq!(editor.get_block_absolute(20, base + 7, 20), None);
        // Even ids run the arm along x.
        let arm_y = base + 1 + (5 * 2 + 2) / 3;
        assert_eq!(
            editor.get_block_absolute(21, arm_y, 20),
            Some(DARK_OAK_FENCE)
        );
        assert_eq!(
            editor.get_block_absolute(19, arm_y, 20),
            Some(DARK_OAK_FENCE)
        );
    }
}
