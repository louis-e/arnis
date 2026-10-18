//! Piers, breakwaters, groynes and mooring dolphins.

use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::connected_blocks::place_connected;
use crate::floodfill::flood_fill_area;
use crate::floodfill_cache::FloodFillCache;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use crate::world_editor::{min_y, WorldEditor};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Pier width in metres when the way has no `width` tag.
const PIER_WIDTH_M: f64 = 3.0;
/// Most a fixed pier deck rises above the lowest ground or water under it.
const PIER_MAX_RISE: i32 = 3;
/// Deck rows above the water from which a pier gets a railing.
const PIER_RAILING_RISE: i32 = 3;
/// Metres between the piles along a pier's edge.
const PILE_SPACING_M: f64 = 4.0;
/// How far around a line pier's end to look for the land it starts from.
const PIER_LAND_END_REACH: i32 = 3;
/// Every this many edge piles of a lit pier carries a lantern.
const LANTERN_EVERY: usize = 3;
/// Jetties up to this wide get mooring posts standing above the deck.
const MOORING_POST_MAX_WIDTH: f64 = 4.0;
const BREAKWATER_WIDTH_M: f64 = 5.0;
const GROYNE_WIDTH_M: f64 = 3.0;
/// Tallest a breakwater may stand above the water, whatever its `height` tag says.
const BREAKWATER_MAX_HEIGHT: i32 = 6;

/// One column of a structure's footprint.
#[derive(Clone, Copy)]
struct FootCell {
    x: i32,
    z: i32,
    /// On the outer ring of the footprint.
    edge: bool,
}

fn tag<'a>(tags: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    tags.get(key).map(String::as_str)
}

fn width_blocks(tags: &HashMap<String, String>, default_m: f64, scale: f64) -> f64 {
    let metres = tag(tags, "width")
        .and_then(|w| w.trim_end_matches(" m").trim().parse::<f64>().ok())
        .filter(|w| *w > 0.0 && *w < 200.0)
        .unwrap_or(default_m);
    (metres * scale).max(1.0)
}

/// Closed ways are areas unless tagged `area=no`.
fn is_area(way: &ProcessedWay) -> bool {
    let closed = way.nodes.len() > 3
        && way.nodes.first().map(|n| (n.x, n.z)) == way.nodes.last().map(|n| (n.x, n.z));
    closed && tag(&way.tags, "area") != Some("no")
}

/// Cells within half the width of a line.
fn line_footprint(way: &ProcessedWay, width: f64) -> Vec<FootCell> {
    let half = width / 2.0;
    let reach = half.ceil() as i32 + 1;
    let mut nearest: HashMap<(i32, i32), f64> = HashMap::new();
    for pair in way.nodes.windows(2) {
        let (ax, az) = (pair[0].x as f64, pair[0].z as f64);
        let (bx, bz) = (pair[1].x as f64, pair[1].z as f64);
        let (dx, dz) = (bx - ax, bz - az);
        let len2 = (dx * dx + dz * dz).max(1e-9);
        let (x0, x1) = (ax.min(bx) as i32 - reach, ax.max(bx) as i32 + reach);
        let (z0, z1) = (az.min(bz) as i32 - reach, az.max(bz) as i32 + reach);
        for x in x0..=x1 {
            for z in z0..=z1 {
                let t = (((x as f64 - ax) * dx + (z as f64 - az) * dz) / len2).clamp(0.0, 1.0);
                let d =
                    ((x as f64 - ax - t * dx).powi(2) + (z as f64 - az - t * dz).powi(2)).sqrt();
                if d <= half {
                    let e = nearest.entry((x, z)).or_insert(d);
                    *e = e.min(d);
                }
            }
        }
    }
    let mut cells: Vec<FootCell> = nearest
        .into_iter()
        .map(|((x, z), d)| FootCell {
            x,
            z,
            edge: d > half - 1.0,
        })
        .collect();
    cells.sort_unstable_by_key(|c| (c.x, c.z));
    cells
}

/// Filled cells of an area, or of a relation's outer rings less its inner ones.
fn area_footprint(
    element: &ProcessedElement,
    cache: &FloodFillCache,
    args: &Args,
) -> Vec<FootCell> {
    let timeout = args.timeout.as_ref();
    let mut inside: HashSet<(i32, i32)> = HashSet::new();
    match element {
        ProcessedElement::Way(way) => inside.extend(cache.get_or_compute(way, timeout).iter()),
        ProcessedElement::Relation(rel) => {
            for member in rel
                .members
                .iter()
                .filter(|m| m.role == ProcessedMemberRole::Outer)
            {
                inside.extend(cache.get_or_compute(&member.way, timeout).iter());
            }
            for member in rel
                .members
                .iter()
                .filter(|m| m.role == ProcessedMemberRole::Inner)
            {
                for cell in cache.get_or_compute(&member.way, timeout).iter() {
                    inside.remove(cell);
                }
            }
        }
        ProcessedElement::Node(_) => {}
    }
    let mut cells: Vec<FootCell> = inside
        .iter()
        .map(|&(x, z)| FootCell {
            x,
            z,
            edge: [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|(dx, dz)| !inside.contains(&(x + dx, z + dz))),
        })
        .collect();
    cells.sort_unstable_by_key(|c| (c.x, c.z));
    cells
}

/// Points every `spacing` blocks along the rings or lines of an element, ends included.
fn stations(lines: &[&ProcessedWay], spacing: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for way in lines {
        let mut carry = 0.0;
        for pair in way.nodes.windows(2) {
            let (ax, az) = (pair[0].x as f64, pair[0].z as f64);
            let (dx, dz) = (pair[1].x as f64 - ax, pair[1].z as f64 - az);
            let len = (dx * dx + dz * dz).sqrt();
            let mut s = carry;
            while s < len {
                out.push((ax + dx * s / len, az + dz * s / len));
                s += spacing;
            }
            carry = s - len;
        }
        if let Some(last) = way.nodes.last() {
            out.push((last.x as f64, last.z as f64));
        }
    }
    out
}

/// A column of `block` from the bed or terrain up to `top_y`.
fn drive_pile(editor: &mut WorldEditor, x: i32, z: i32, top_y: i32, block: Block) {
    let ground = editor.get_ground_level(x, z);
    for y in (ground + 1)..=top_y {
        editor.set_block_absolute(block, x, y, z, None, None);
    }
    // A water carve after this one extends the pile itself; water already there is pierced now.
    editor.register_support_column(x, z, block);
    let mut y = ground.min(top_y);
    while y > min_y() && editor.check_for_block_absolute(x, y, z, Some(&[WATER]), None) {
        editor.set_block_absolute(block, x, y, z, Some(&[WATER]), None);
        y -= 1;
    }
}

struct PierMaterials {
    deck: Block,
    pile: Block,
    railing: Block,
}

fn pier_materials(tags: &HashMap<String, String>) -> PierMaterials {
    let solid = |v: &str| {
        matches!(
            v,
            "concrete"
                | "asphalt"
                | "paved"
                | "paving_stones"
                | "sett"
                | "stone"
                | "reinforced_concrete"
        )
    };
    let stone = tag(tags, "surface").is_some_and(solid) || tag(tags, "material").is_some_and(solid);
    if stone {
        PierMaterials {
            deck: SMOOTH_STONE,
            pile: POLISHED_ANDESITE,
            railing: IRON_BARS,
        }
    } else {
        PierMaterials {
            deck: SPRUCE_PLANKS,
            pile: SPRUCE_LOG,
            railing: SPRUCE_FENCE,
        }
    }
}

/// A level deck on piles; floating piers lie on the water without them.
pub fn generate_pier(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    cache: &FloodFillCache,
    args: &Args,
) {
    let tags = element.tags();
    let (cells, outline): (Vec<FootCell>, Vec<&ProcessedWay>) = match element {
        ProcessedElement::Way(way) if way.nodes.len() >= 2 => {
            if is_area(way) {
                (area_footprint(element, cache, args), vec![way])
            } else {
                let width = width_blocks(tags, PIER_WIDTH_M, args.scale);
                (line_footprint(way, width), vec![way])
            }
        }
        ProcessedElement::Relation(rel) => (
            area_footprint(element, cache, args),
            rel.members
                .iter()
                .filter(|m| m.role == ProcessedMemberRole::Outer)
                .map(|m| m.way.as_ref())
                .collect(),
        ),
        _ => return,
    };
    if cells.is_empty() {
        return;
    }
    let line = matches!(element, ProcessedElement::Way(way) if !is_area(way));
    let floating = tag(tags, "floating") == Some("yes");
    let lit = tag(tags, "lit") == Some("yes");
    let mat = pier_materials(tags);

    let grounds: Vec<i32> = cells
        .iter()
        .map(|c| editor.get_ground_level(c.x, c.z))
        .collect();
    let base = *grounds.iter().min().unwrap_or(&0);
    let top = *grounds.iter().max().unwrap_or(&0);
    let deck_y = if floating {
        base + 1
    } else {
        base + 1 + (top - base).clamp(1, PIER_MAX_RISE)
    };
    let railing = !floating && deck_y - base >= PIER_RAILING_RISE;
    let deck_at = |ground: i32| deck_y.max(ground + 1);

    for (c, &ground) in cells.iter().zip(&grounds) {
        editor.set_block_absolute(mat.deck, c.x, deck_at(ground), c.z, None, None);
    }
    if floating {
        return;
    }

    let footprint: HashMap<(i32, i32), (bool, i32)> = cells
        .iter()
        .zip(&grounds)
        .map(|(c, &g)| ((c.x, c.z), (c.edge, g)))
        .collect();
    // Piles along the edges, standing proud of the deck as mooring posts on a small jetty.
    let width = width_blocks(tags, PIER_WIDTH_M, args.scale);
    let posts_above = line && width <= MOORING_POST_MAX_WIDTH;
    let spacing = PILE_SPACING_M * args.scale.max(0.25);
    let edge_inset = if line {
        (width / 2.0 - 0.5).max(0.0)
    } else {
        0.0
    };
    let mut edge_piles: Vec<(i32, i32)> = Vec::new();
    for (sx, sz) in stations(&outline, spacing) {
        if line {
            // Both sides of the line at this station.
            let way = outline[0];
            let (dx, dz) = nearest_heading(way, sx, sz);
            for sign in [1.0, -1.0] {
                let cell = (
                    (sx - dz * edge_inset * sign).round() as i32,
                    (sz + dx * edge_inset * sign).round() as i32,
                );
                edge_piles.push(cell);
            }
        } else {
            edge_piles.push((sx.round() as i32, sz.round() as i32));
        }
    }
    edge_piles.dedup();
    let mut lanterns = 0usize;
    for &(x, z) in &edge_piles {
        let Some(&(_, ground)) = footprint.get(&(x, z)) else {
            continue;
        };
        let y = deck_at(ground);
        if y - ground < 2 {
            continue;
        }
        let pile_top = if posts_above { y + 1 } else { y - 1 };
        drive_pile(editor, x, z, pile_top, mat.pile);
        if lit && posts_above {
            if lanterns.is_multiple_of(LANTERN_EVERY) {
                editor.set_block_absolute(LANTERN, x, y + 2, z, None, None);
            }
            lanterns += 1;
        }
    }
    // A grid of piles under wide decks.
    if !line {
        let grid = spacing.round().max(2.0) as i32;
        for c in cells.iter().filter(|c| !c.edge) {
            if c.x.rem_euclid(grid) == 0 && c.z.rem_euclid(grid) == 0 {
                let ground = footprint[&(c.x, c.z)].1;
                if deck_at(ground) - ground >= 2 {
                    drive_pile(editor, c.x, c.z, deck_at(ground) - 1, mat.pile);
                }
            }
        }
    }

    if !railing {
        return;
    }
    // A line pier's end that meets higher ground stays open, even off a quay wall.
    let land_ends: Vec<(i32, i32)> = if line {
        let way = outline[0];
        [way.nodes.first(), way.nodes.last()]
            .into_iter()
            .flatten()
            .map(|n| (n.x, n.z))
            .filter(|&(x, z)| {
                let r = PIER_LAND_END_REACH;
                (-r..=r).any(|dx| {
                    (-r..=r).any(|dz| editor.get_ground_level(x + dx, z + dz) >= deck_y - 1)
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let gate = (width / 2.0 + 1.0).powi(2);
    let mut lanterns = 0usize;
    for (c, &ground) in cells.iter().zip(&grounds) {
        let y = deck_at(ground);
        // Open where the pier meets the land.
        let at_land_end = land_ends
            .iter()
            .any(|&(x, z)| ((c.x - x).pow(2) + (c.z - z).pow(2)) as f64 <= gate);
        if !c.edge || y - ground < 2 || at_land_end {
            continue;
        }
        place_connected(editor, mat.railing, c.x, y + 1, c.z);
        if lit && edge_piles.contains(&(c.x, c.z)) {
            if lanterns.is_multiple_of(LANTERN_EVERY) {
                editor.set_block_absolute(LANTERN, c.x, y + 2, c.z, None, None);
            }
            lanterns += 1;
        }
    }
}

/// Unit direction of the segment of `way` nearest to (x, z).
fn nearest_heading(way: &ProcessedWay, x: f64, z: f64) -> (f64, f64) {
    let mut best = (f64::MAX, (1.0, 0.0));
    for pair in way.nodes.windows(2) {
        let (ax, az) = (pair[0].x as f64, pair[0].z as f64);
        let (dx, dz) = (pair[1].x as f64 - ax, pair[1].z as f64 - az);
        let len2 = dx * dx + dz * dz;
        if len2 <= 0.0 {
            continue;
        }
        let t = (((x - ax) * dx + (z - az) * dz) / len2).clamp(0.0, 1.0);
        let d = (x - ax - t * dx).powi(2) + (z - az - t * dz).powi(2);
        if d < best.0 {
            let len = len2.sqrt();
            best = (d, (dx / len, dz / len));
        }
    }
    best.1
}

/// Share of a path's cells that must lie on a pier for the deck to stand in for it.
const PATH_ON_PIER_SHARE: f64 = 0.75;

/// Drops footpaths that run along a pier, whose deck is the walking surface.
pub fn drop_paths_on_piers(
    elements: &mut Vec<ProcessedElement>,
    scale: f64,
    timeout: Option<&Duration>,
) -> usize {
    let mut deck: HashSet<(i32, i32)> = HashSet::new();
    let ring =
        |way: &ProcessedWay| -> Vec<(i32, i32)> { way.nodes.iter().map(|n| (n.x, n.z)).collect() };
    for element in elements.iter() {
        if tag(element.tags(), "man_made") != Some("pier") {
            continue;
        }
        match element {
            ProcessedElement::Way(way) if way.nodes.len() >= 2 => {
                if is_area(way) {
                    deck.extend(flood_fill_area(&ring(way), timeout));
                } else {
                    let width = width_blocks(&way.tags, PIER_WIDTH_M, scale);
                    deck.extend(line_footprint(way, width).iter().map(|c| (c.x, c.z)));
                }
            }
            ProcessedElement::Relation(rel) => {
                for m in rel
                    .members
                    .iter()
                    .filter(|m| m.role == ProcessedMemberRole::Outer)
                {
                    deck.extend(flood_fill_area(&ring(&m.way), timeout));
                }
            }
            _ => {}
        }
    }
    if deck.is_empty() {
        return 0;
    }
    let on_pier = |way: &ProcessedWay| {
        let walkway = matches!(
            tag(&way.tags, "highway"),
            Some(
                "footway" | "path" | "pedestrian" | "cycleway" | "steps" | "bridleway" | "corridor"
            )
        );
        if !walkway || tag(&way.tags, "man_made") == Some("pier") {
            return false;
        }
        let cells: Vec<(i32, i32)> = way
            .nodes
            .windows(2)
            .flat_map(|p| bresenham_line(p[0].x, 0, p[0].z, p[1].x, 0, p[1].z))
            .map(|(x, _, z)| (x, z))
            .collect();
        let on = cells.iter().filter(|c| deck.contains(c)).count();
        !cells.is_empty() && on as f64 >= cells.len() as f64 * PATH_ON_PIER_SHARE
    };
    let before = elements.len();
    elements.retain(|e| !matches!(e, ProcessedElement::Way(way) if on_pier(way)));
    before - elements.len()
}

/// Rubble block for one column, so a breakwater doesn't read as one smooth wall.
fn rubble(x: i32, z: i32) -> Block {
    let h = (x as u32).wrapping_mul(73_856_093) ^ (z as u32).wrapping_mul(19_349_663);
    match h % 10 {
        0..=3 => STONE,
        4..=6 => COBBLESTONE,
        7..=8 => ANDESITE,
        _ => MOSSY_COBBLESTONE,
    }
}

/// A rubble mound standing out of the water; wooden groynes are a row of piles.
pub fn generate_breakwater(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    cache: &FloodFillCache,
    args: &Args,
) {
    let tags = element.tags();
    let groyne = tag(tags, "man_made") == Some("groyne");
    let wooden = tag(tags, "material") == Some("wood");
    let cells = match element {
        ProcessedElement::Way(way) if way.nodes.len() >= 2 => {
            if is_area(way) {
                area_footprint(element, cache, args)
            } else {
                let default_m = match (groyne, wooden) {
                    (_, true) => 1.0,
                    (true, false) => GROYNE_WIDTH_M,
                    (false, false) => BREAKWATER_WIDTH_M,
                };
                line_footprint(way, width_blocks(tags, default_m, args.scale))
            }
        }
        ProcessedElement::Relation(_) => area_footprint(element, cache, args),
        _ => return,
    };
    if cells.is_empty() {
        return;
    }
    let grounds: Vec<i32> = cells
        .iter()
        .map(|c| editor.get_ground_level(c.x, c.z))
        .collect();
    let base = *grounds.iter().min().unwrap_or(&0);
    let rise = if tag(tags, "submerged") == Some("yes") {
        0
    } else if let Some(h) = tag(tags, "height").and_then(|h| h.parse::<f64>().ok()) {
        ((h * args.scale).round() as i32).clamp(1, BREAKWATER_MAX_HEIGHT)
    } else if groyne {
        1
    } else {
        2
    };
    for (c, &ground) in cells.iter().zip(&grounds) {
        if wooden {
            // Weathered piles of uneven height.
            let uneven = i32::from((c.x + c.z).rem_euclid(3) == 0);
            drive_pile(editor, c.x, c.z, base + rise.max(1) + uneven, SPRUCE_LOG);
            continue;
        }
        // The crown stands a row above the flanks.
        let top = base + rise - i32::from(c.edge && rise > 1);
        if top > ground || ground <= base {
            drive_pile(editor, c.x, c.z, top.max(ground), rubble(c.x, c.z));
        }
    }
}

/// A cluster of mooring piles standing out of the water.
pub fn generate_dolphin(editor: &mut WorldEditor, node: &ProcessedNode) {
    let top = editor.get_ground_level(node.x, node.z) + 2;
    for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        drive_pile(editor, node.x + dx, node.z + dz, top, SPRUCE_LOG);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::osm_parser::ProcessedNode;
    use clap::Parser;
    use std::path::PathBuf;
    use std::sync::Arc;

    const RUBBLE: &[Block] = &[STONE, COBBLESTONE, ANDESITE, MOSSY_COBBLESTONE];

    fn editor(xzbbox: &XZBBox) -> WorldEditor<'_> {
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        WorldEditor::new(PathBuf::from("/dev/null/unused"), xzbbox, llbbox)
    }

    fn args() -> Args {
        Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter())
    }

    fn way(id: u64, tags: &[(&str, &str)], points: &[(i32, i32)]) -> ProcessedWay {
        ProcessedWay {
            id,
            nodes: points
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: id * 100 + i as u64,
                    tags: HashMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    /// A lake already carved down to y=-3 over the whole test area.
    fn carved_lake(editor: &mut WorldEditor) {
        for x in 10..60 {
            for z in 30..70 {
                for y in -3..=0 {
                    editor.set_block_absolute(WATER, x, y, z, None, None);
                }
            }
        }
    }

    fn is(editor: &WorldEditor, x: i32, y: i32, z: i32, blocks: &[Block]) -> bool {
        editor.check_for_block_absolute(x, y, z, Some(blocks), None)
    }

    #[test]
    fn jetty_stands_on_piles_down_to_the_lake_bed() {
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut editor = editor(&xzbbox);
        carved_lake(&mut editor);
        let pier = way(1, &[("man_made", "pier")], &[(20, 50), (40, 50)]);
        generate_pier(
            &mut editor,
            &ProcessedElement::Way(pier),
            &FloodFillCache::new(),
            &args(),
        );

        for z in 49..=51 {
            assert!(is(&editor, 30, 2, z, &[SPRUCE_PLANKS]), "deck at z={z}");
        }
        assert!(!is(&editor, 30, 2, 52, &[SPRUCE_PLANKS]), "three wide");
        assert!(
            is(&editor, 24, 3, 49, &[SPRUCE_LOG]),
            "mooring post above the deck"
        );
        assert!(
            is(&editor, 24, -3, 49, &[SPRUCE_LOG]),
            "pile down through the water"
        );
    }

    #[test]
    fn floating_pontoon_lies_on_the_water() {
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut editor = editor(&xzbbox);
        carved_lake(&mut editor);
        let pontoon = way(
            1,
            &[("man_made", "pier"), ("floating", "yes")],
            &[(20, 40), (30, 40), (30, 50), (20, 50), (20, 40)],
        );
        generate_pier(
            &mut editor,
            &ProcessedElement::Way(pontoon),
            &FloodFillCache::new(),
            &args(),
        );

        assert!(
            is(&editor, 25, 1, 45, &[SPRUCE_PLANKS]),
            "filled, not just outlined"
        );
        assert!(is(&editor, 24, 0, 44, &[WATER]), "no piles under a pontoon");
    }

    #[test]
    fn raised_pier_gets_a_railing_but_stays_open_to_the_land() {
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut editor = editor(&xzbbox);
        // A promenade three blocks above the sea west of x=22.
        let row: Vec<f32> = (0..100).map(|x| if x < 22 { 3.0 } else { 0.0 }).collect();
        let ground = crate::ground::Ground::new_elevation_test(vec![row; 100], 100, 100);
        editor.set_ground(Arc::new(ground));
        let pier = way(
            1,
            &[("man_made", "pier")],
            &[(18, 40), (40, 40), (40, 50), (18, 50), (18, 40)],
        );
        generate_pier(
            &mut editor,
            &ProcessedElement::Way(pier),
            &FloodFillCache::new(),
            &args(),
        );

        let deck_y = editor.get_ground_level(30, 45) + 1 + 3;
        assert!(
            is(&editor, 30, deck_y, 45, &[SPRUCE_PLANKS]),
            "deck level with the promenade"
        );
        let fenced = |x: i32| (38..=42).any(|z| is(&editor, x, deck_y + 1, z, &[SPRUCE_FENCE]));
        assert!((30..40).any(fenced), "railing along the seaward edge");
        let landward = editor.get_ground_level(19, 45) + 1;
        assert!(
            !(38..=52).any(|z| is(&editor, 19, landward + 1, z, &[SPRUCE_FENCE])),
            "open where it meets the promenade"
        );
    }

    #[test]
    fn breakwater_crown_stands_above_its_flanks() {
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut editor = editor(&xzbbox);
        carved_lake(&mut editor);
        let breakwater = way(1, &[("man_made", "breakwater")], &[(20, 50), (40, 50)]);
        generate_breakwater(
            &mut editor,
            &ProcessedElement::Way(breakwater),
            &FloodFillCache::new(),
            &args(),
        );

        assert!(is(&editor, 30, 2, 50, RUBBLE), "crown");
        assert!(is(&editor, 30, 1, 52, RUBBLE), "flank");
        assert!(!is(&editor, 30, 2, 52, RUBBLE), "flank stays a row lower");
        assert!(is(&editor, 30, -3, 50, RUBBLE), "mound reaches the bed");
    }

    #[test]
    fn wooden_groyne_is_a_row_of_piles() {
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut editor = editor(&xzbbox);
        carved_lake(&mut editor);
        let groyne = way(
            1,
            &[("man_made", "groyne"), ("material", "wood")],
            &[(20, 50), (40, 50)],
        );
        generate_breakwater(
            &mut editor,
            &ProcessedElement::Way(groyne),
            &FloodFillCache::new(),
            &args(),
        );

        assert!(
            is(&editor, 30, 1, 50, &[SPRUCE_LOG]),
            "pile above the water"
        );
        assert!(is(&editor, 30, -3, 50, &[SPRUCE_LOG]), "pile into the bed");
        assert!(!is(&editor, 30, 1, 51, &[SPRUCE_LOG]), "one pile wide");
    }

    #[test]
    fn footpath_along_a_pier_gives_way_to_the_deck() {
        let pier = way(1, &[("man_made", "pier")], &[(20, 50), (40, 50)]);
        let on = way(2, &[("highway", "footway")], &[(21, 50), (39, 50)]);
        let ashore = way(3, &[("highway", "footway")], &[(20, 60), (40, 60)]);
        let mut elements: Vec<ProcessedElement> = [pier, on, ashore]
            .into_iter()
            .map(ProcessedElement::Way)
            .collect();
        assert_eq!(drop_paths_on_piers(&mut elements, 1.0, None), 1);
        let ids: Vec<u64> = elements.iter().map(|e| e.id()).collect();
        assert_eq!(ids, vec![1, 3]);
    }
}
