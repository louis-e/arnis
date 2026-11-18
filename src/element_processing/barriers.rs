use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::bridges::{is_bridge_way, BridgeSurfaceMap};
use crate::element_processing::connected_blocks::{
    four_connected_line, is_connectable, place_connected, place_gate,
};
use crate::element_processing::fortifications;
use crate::element_processing::highways::{
    highway_block_range, is_carriageway, parse_width_tag_m, stamp_radius, surface_highway_type,
    tunnel_renders_at_surface,
};
use crate::floodfill_cache::CoordinateBitmap;
use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use fnv::{FnvHashMap, FnvHashSet};
use std::sync::Arc;

const BRIDGE_BARRIER_NEARBY_RADIUS: i32 = 2;

/// 32-block buckets for the road index.
const BUCKET_SHIFT: i32 = 5;
/// Spacing of the points sampled along a road to find the buckets its stamp reaches.
const SAMPLE_STEP: f32 = 8.0;
/// Barrier and road closer to parallel than this (cos of ~25°) run alongside each other.
const ALONG_COS: f32 = 0.9;
/// Furthest a barrier drawn beside a road is moved to clear its surface.
const MAX_SHIFT: i32 = 24;
/// Past its own road's edge a barrier moves this much further to clear an adjoining
/// path or kerb strip, else stays at that edge.
const EXTRA_SHIFT: i32 = 2;
/// Widest gate or opening, in blocks.
const MAX_GATE_SPAN: usize = 5;

/// How a barrier type is drawn.
struct Style {
    material: Block,
    height: i32,
    /// Stands on roads it crosses, as bollards and road blocks do, instead of opening them.
    crosses_roads: bool,
    /// Separate posts with a gap between, as bollards stand.
    spaced: bool,
}

fn barrier_style(tags: &std::collections::HashMap<String, String>) -> Option<Style> {
    let mut style = Style {
        material: COBBLESTONE_WALL,
        height: 2,
        crosses_roads: false,
        spaced: false,
    };
    let mut set = |material: Block, height: i32| {
        style.material = material;
        style.height = height;
    };
    match tags.get("barrier").map(|s| s.as_str()) {
        Some("bollard") => {
            set(COBBLESTONE_WALL, 1);
            style.crosses_roads = true;
            style.spaced = true;
        }
        // Kerbs and ditches are no raised barrier.
        Some("kerb" | "ditch") => return None,
        Some("hedge") => set(OAK_LEAVES, 2),
        Some("fence") => match tags.get("fence_type").map(|s| s.as_str()) {
            Some("railing" | "bars" | "krest") => set(STONE_BRICK_WALL, 1),
            Some(
                "chain_link" | "metal" | "wire" | "barbed_wire" | "corrugated_metal" | "electric"
                | "metal_bars",
            ) => set(STONE_BRICK_WALL, 2), // IRON_BARS
            Some("slatted" | "paling") => set(OAK_FENCE, 1),
            Some("wood" | "split_rail" | "panel" | "pole") => set(OAK_FENCE, 2),
            Some("concrete" | "stone") => set(STONE_BRICK_WALL, 2),
            Some("glass") => set(GLASS, 1),
            _ => {}
        },
        Some("wall" | "city_wall") => set(STONE_BRICK_WALL, 3),
        // Holds back the ground behind it, so only its top shows.
        Some("retaining_wall") => set(STONE_BRICK_WALL, 1),
        Some("guard_rail" | "handrail" | "cable_barrier") => set(STONE_BRICK_WALL, 1),
        Some("jersey_barrier") => {
            set(LIGHT_GRAY_CONCRETE, 1);
            style.crosses_roads = true;
        }
        Some("block") => {
            set(STONE, 1);
            style.crosses_roads = true;
        }
        _ => {}
    }
    // Tagged material takes priority over inferred
    match tags.get("material").map(|s| s.as_str()) {
        Some("brick") => style.material = BRICK,
        Some("concrete") => style.material = LIGHT_GRAY_CONCRETE,
        Some("metal") => style.material = STONE_BRICK_WALL,
        Some("wood") => style.material = OAK_FENCE,
        _ => {}
    }
    Some(style)
}

pub fn generate_barriers(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    bridge_surface: &BridgeSurfaceMap,
    clearance: &RoadClearance,
    building_footprints: &CoordinateBitmap,
) {
    let Some(style) = barrier_style(element.tags()) else {
        return;
    };
    let ProcessedElement::Way(way) = element else {
        return;
    };
    // A part floating at min_height keeps the plain barrier's handling.
    if fortifications::line_wall(&way.tags) && !way.tags.contains_key("min_height") {
        generate_town_wall(editor, way, bridge_surface, clearance, building_footprints);
        return;
    }
    // A 3D-mapped railing, say on a bridge tower, floats at min_height and its
    // height is measured from the ground, not from where it starts.
    // Both scale with the world, like the structure it is attached to.
    let scale = editor.scale();
    // Past 1000 m a value is junk, and `inf` or `1e10` would overflow the sums below.
    let metres = |key: &str| {
        element
            .tags()
            .get(key)
            .and_then(|h| h.trim_end_matches('m').trim().parse::<f64>().ok())
            .filter(|h| (0.0..=1000.0).contains(h))
    };
    let lift: i32 = metres("min_height").map_or(0, |h| (h * scale).round() as i32);
    let height_scale = if lift > 0 { scale } else { 1.0 };
    let wall_height: i32 = metres("height")
        .map(|height: f64| (height * height_scale).round() as i32 - lift)
        .unwrap_or(style.height)
        .max(1);
    let material = style.material;
    let joined = is_connectable(material) && !style.spaced;
    // Only masonry gets a coping.
    let capped = wall_height > 1 && !matches!(material, OAK_LEAVES | OAK_FENCE);
    let hedge = material == OAK_LEAVES;

    // A floating part is off the street, so it keeps its mapped line.
    let grounded = lift == 0;
    let trace = trace(way, scale, grounded);
    let spots: Vec<Spot> = if grounded {
        // Stretches on a bridge deck stay clear of the roads underneath.
        let deck = deck_cells(editor, bridge_surface, &trace.cells);
        trace
            .cells
            .iter()
            .zip(&trace.dirs)
            .zip(deck)
            .map(|((&(x, z), &dir), on_deck)| {
                // The bridge brings its own railing and keeps its deck clear.
                if on_deck && bridge_surface.keeps_clear(x, z) {
                    Spot::RoadGap
                } else if on_deck {
                    Spot::Post(x, z)
                } else {
                    clearance.place(x, z, dir, style.crosses_roads, building_footprints)
                }
            })
            .collect()
    } else {
        trace.cells.iter().map(|&(x, z)| Spot::Post(x, z)).collect()
    };
    // Links between posts keep out of buildings and off the road they cleared.
    let cost = |x: i32, z: i32| match grounded {
        true if building_footprints.contains(x, z) => 2,
        true if !style.crosses_roads && clearance.on_road(x, z) => 1,
        _ => 0,
    };
    let runs = lay_out(&trace, spots, style.spaced, joined, cost);
    // A floating part keeps one level, measured from the structure's lowest footing.
    let low = (lift > 0)
        .then(|| {
            runs.iter()
                .flatten()
                .map(|p| editor.get_absolute_y(p.x, 0, p.z))
                .min()
        })
        .flatten();

    for run in &runs {
        let bases = if lift > 0 {
            vec![low; run.len()]
        } else {
            let cells: Vec<(i32, i32)> = run.iter().map(|p| (p.x, p.z)).collect();
            barrier_bases(editor, bridge_surface, &cells)
        };
        for (post, base) in run.iter().zip(&bases) {
            let (Some(base), None) = (*base, post.gate) else {
                continue;
            };
            let (bx, bz) = (post.x, post.z);
            // Hedges stand on soil, not on the paving beside the lawn they edge.
            if hedge
                && !editor.surface_is_sealed(bx, bz)
                && base == editor.get_absolute_y(bx, 0, bz)
            {
                editor.set_block_absolute(GRASS_BLOCK, bx, base, bz, None, None);
            }
            if hedge {
                editor.register_hedge(bx, bz);
            }
            editor.register_barrier(bx, bz);
            for y in (lift + 1)..=(lift + wall_height) {
                if joined {
                    place_connected(editor, material, bx, base + y, bz);
                } else {
                    editor.set_block_absolute(material, bx, base + y, bz, None, None);
                }
            }
            if capped {
                editor.set_block_absolute(
                    STONE_BRICK_SLAB,
                    bx,
                    base + lift + wall_height + 1,
                    bz,
                    None,
                    None,
                );
            }
        }
        // Gates go in last, so they join the posts on both sides.
        for (post, base) in run.iter().zip(&bases) {
            if let (Some(base), Some(along_x)) = (*base, post.gate) {
                place_gate(
                    editor,
                    OAK_FENCE_GATE,
                    post.x,
                    base + lift + 1,
                    post.z,
                    along_x,
                );
            }
        }
    }
}

/// Height of a town wall mapped as a line without one, in metres; a ruin keeps half.
const TOWN_WALL_M: f64 = 8.0;

/// Thickness of a town wall mapped as a line without a width, in metres.
const TOWN_WALL_THICK_M: f64 = 3.0;

/// Past this a town wall's mapped width is junk, in metres.
const TOWN_WALL_MAX_THICK_M: f64 = 30.0;

/// A town wall mapped as a line, as thick as its width and arched over ways and gates.
fn generate_town_wall(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    bridge_surface: &BridgeSurfaceMap,
    clearance: &RoadClearance,
    building_footprints: &CoordinateBitmap,
) {
    let scale = editor.scale();
    let metres = |key: &str| {
        way.tags
            .get(key)
            .and_then(|v| v.trim_end_matches('m').trim().parse::<f64>().ok())
            .filter(|m| (0.5..=100.0).contains(m))
    };
    let ruin = way.tags.get("ruins").is_some_and(|v| v != "no")
        || way.tags.get("historic").map(String::as_str) == Some("ruins");
    let height = metres("height").unwrap_or(if ruin { TOWN_WALL_M / 2.0 } else { TOWN_WALL_M });
    let blocks = ((height * scale).round() as i32).max(1);
    let thick = parse_width_tag_m(&way.tags)
        .filter(|w| (0.5..=TOWN_WALL_MAX_THICK_M).contains(w))
        .unwrap_or(TOWN_WALL_THICK_M);
    let reach = (((thick * scale).round() as i32 - 1) / 2).max(0);
    let trace = trace(way, scale, true);
    // Stretches on a bridge deck stand on it, as other barriers do.
    let on_deck = deck_cells(editor, bridge_surface, &trace.cells);
    let water = |x: i32, z: i32| {
        let y = editor.get_absolute_y(x, 0, z);
        editor.water_at(x, z)
            || [y, y + 1]
                .iter()
                .any(|&y| editor.get_block_absolute(x, y, z) == Some(WATER))
    };
    let mut cells: FnvHashSet<(i32, i32)> = FnvHashSet::default();
    let mut posts: FnvHashSet<(i32, i32)> = FnvHashSet::default();
    let mut passages: FnvHashSet<(i32, i32)> = FnvHashSet::default();
    let mut decked: FnvHashSet<(i32, i32)> = FnvHashSet::default();
    let stamp = |set: &mut FnvHashSet<(i32, i32)>, (x, z): (i32, i32)| {
        for dx in -reach..=reach {
            for dz in -reach..=reach {
                set.insert((x + dx, z + dz));
            }
        }
    };
    // A line leaving the area is clipped along its edge, where no wall stands.
    let (lo, hi) = clearance.world;
    let edge = |x: i32, z: i32| x <= lo.0 || x >= hi.0 || z <= lo.1 || z >= hi.1;
    let mut last: Option<(i32, i32)> = None;
    for ((&(x, z), &dir), &deck) in trace.cells.iter().zip(&trace.dirs).zip(&on_deck) {
        let spot = if edge(x, z) {
            None
        } else if deck {
            // The bridge keeps its deck clear and brings its own railing.
            (!bridge_surface.keeps_clear(x, z)).then_some(Spot::Post(x, z))
        } else if water(x, z) {
            None
        } else {
            Some(clearance.place_town_wall(x, z, dir, building_footprints))
        };
        match spot {
            Some(Spot::Post(px, pz)) => {
                // Posts moved off a road are joined, as a plain barrier's are.
                let run = match last {
                    Some((lx, lz)) if (lx - px).abs().max((lz - pz).abs()) > 1 => {
                        bresenham_line(lx, 0, lz, px, 0, pz)
                            .into_iter()
                            .map(|(x, _, z)| (x, z))
                            .collect()
                    }
                    _ => vec![(px, pz)],
                };
                for c in run {
                    posts.insert(c);
                    stamp(&mut cells, c);
                    if deck {
                        stamp(&mut decked, c);
                    }
                }
                last = Some((px, pz));
            }
            Some(_) => {
                stamp(&mut cells, (x, z));
                stamp(&mut passages, (x, z));
                last = None;
            }
            None => last = None,
        }
    }
    for &(at, _, width) in &trace.marks {
        for j in span(at, width, trace.cells.len()) {
            stamp(&mut passages, trace.cells[j]);
        }
    }
    // A bridge keeps its deck, unless it passes high over the wall.
    let clears_wall = |x: i32, z: i32| {
        let deck = bridge_surface
            .deck_y_at(x, z)
            .or_else(|| bridge_surface.nearest_railing(x, z, 0).map(|(_, y)| y));
        deck.is_some_and(|d| d > editor.get_absolute_y(x, 0, z) + blocks + 2)
    };
    // On a deck it stands where the deck holds it up, as a railing does.
    let footing = |x: i32, z: i32| {
        bridge_surface
            .supported_top(editor, x, z, BRIDGE_BARRIER_NEARBY_RADIUS)
            .or_else(|| {
                let g = editor.get_absolute_y(x, 0, z);
                bridge_surface
                    .nearby_deck_y(x, z, BRIDGE_BARRIER_NEARBY_RADIUS)
                    .is_some_and(|d| d - g <= 1)
                    .then_some(g)
            })
    };
    let footings: FnvHashMap<(i32, i32), i32> = decked
        .iter()
        .filter_map(|&(x, z)| Some(((x, z), footing(x, z)?)))
        .collect();
    // A gate or tower mapped as a building stands in the wall's place; beside a road, a
    // track or water the wall keeps off it, and is arched over a road or track it crosses.
    let mut cells: Vec<(i32, i32)> = cells
        .into_iter()
        .filter(|&(x, z)| !building_footprints.contains(x, z))
        .filter(|c| {
            passages.contains(c) || posts.contains(c) || !clearance.keeps_wall_off(c.0, c.1)
        })
        .filter(|&(x, z)| !bridge_surface.keeps_clear(x, z) || clears_wall(x, z))
        .filter(|c| !decked.contains(c) || footings.contains_key(c))
        .filter(|c| decked.contains(c) || !water(c.0, c.1))
        .collect();
    cells.sort_unstable();
    for &(x, z) in &cells {
        editor.register_barrier(x, z);
    }
    // Where it meets another town wall, its end makes no face.
    let joints: FnvHashSet<(i32, i32)> = way
        .nodes
        .iter()
        .filter(|n| clearance.wall_joints.contains(&n.id))
        .flat_map(|n| {
            let r = reach + 1;
            (-r..=r).flat_map(move |dx| (-r..=r).map(move |dz| (n.x + dx, n.z + dz)))
        })
        .collect();
    let tag = |k: &str| way.tags.get(k).map(String::as_str);
    let colour = ["barrier:colour", "colour", "wall:colour"]
        .iter()
        .find_map(|k| tag(k).and_then(crate::colors::color_text_to_rgb_tuple));
    let material = ["barrier:material", "material", "wall:material"]
        .iter()
        .find_map(|k| tag(k));
    let block = match (material, colour) {
        (Some("brick" | "bricks"), _) => BRICK,
        (_, Some(rgb)) => crate::block_palette::wall_block_for_color(
            rgb,
            &mut crate::deterministic_rng::element_rng(way.id),
        ),
        (Some("sandstone"), _) => SANDSTONE,
        _ => STONE_BRICKS,
    };
    fortifications::generate(
        editor,
        &fortifications::Mass {
            cells: &cells,
            block,
            height: blocks,
            base: None,
            raised: false,
            crenellated: !ruin,
            passages,
            footings,
            abutting: &|x, z| building_footprints.contains(x, z) || joints.contains(&(x, z)),
            scale,
        },
    );
}

/// One cell of a barrier run; `gate` is Some(runs along x) for a fence gate.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Post {
    x: i32,
    z: i32,
    gate: Option<bool>,
}

/// What becomes of one traced barrier cell.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Spot {
    Post(i32, i32),
    Gate(i32, i32),
    /// Left open where the barrier crosses a footway or other path.
    PathGap,
    /// Left open where the barrier crosses a road.
    RoadGap,
    /// Left open for an entrance, stile or the like mapped on the barrier.
    Opening,
}

#[derive(Clone, Copy)]
enum NodeRole {
    Gate,
    Opening,
}

/// What a node on a barrier line makes of it, and over how many blocks.
fn node_role(node: &ProcessedNode, scale: f64) -> Option<(NodeRole, usize)> {
    let (role, default_width) = match node.tags.get("barrier").map(String::as_str) {
        Some(
            "gate" | "swing_gate" | "sliding_gate" | "wicket_gate" | "hampshire_gate"
            | "kissing_gate" | "bump_gate",
        ) => (NodeRole::Gate, 1),
        Some("entrance" | "stile" | "horse_stile" | "turnstile" | "full-height_turnstile") => {
            (NodeRole::Opening, 1)
        }
        // Open over a lane's width; only a bollard places something on its node.
        Some(
            "lift_gate" | "cycle_barrier" | "motorcycle_barrier" | "cattle_grid" | "border_control"
            | "toll_booth" | "sally_port" | "bollard" | "chain" | "rope" | "height_restrictor",
        ) => (NodeRole::Opening, 3),
        None if node.tags.contains_key("entrance") => (NodeRole::Opening, 1),
        None if node.tags.get("historic").map(String::as_str) == Some("city_gate") => {
            (NodeRole::Opening, 3)
        }
        _ => return None,
    };
    // A town gate opens over a lane's width whatever it is tagged as.
    let default_width = if node.tags.get("historic").map(String::as_str) == Some("city_gate") {
        default_width.max(3)
    } else {
        default_width
    };
    let width = parse_width_tag_m(&node.tags).map_or(default_width, |w| {
        ((w * scale).round() as usize).clamp(1, MAX_GATE_SPAN)
    });
    Some((role, width))
}

/// A barrier way traced cell by cell, with each cell's heading and the gates and
/// openings mapped on its nodes.
struct Trace {
    cells: Vec<(i32, i32)>,
    dirs: Vec<(f32, f32)>,
    marks: Vec<(usize, NodeRole, usize)>,
}

fn trace(way: &ProcessedWay, scale: f64, with_marks: bool) -> Trace {
    let mut trace = Trace {
        cells: Vec::new(),
        dirs: Vec::new(),
        marks: Vec::new(),
    };
    let mark = |marks: &mut Vec<_>, node: &ProcessedNode, at: usize| {
        // A bollard node on a line of bollards is just one of them.
        if with_marks && node.tags.get("barrier") != way.tags.get("barrier") {
            if let Some((role, width)) = node_role(node, scale) {
                marks.push((at, role, width));
            }
        }
    };
    let segments = way.nodes.len().saturating_sub(1);
    for (i, pair) in way.nodes.windows(2).enumerate() {
        let (a, b) = (&pair[0], &pair[1]);
        let (dx, dz) = ((b.x - a.x) as f32, (b.z - a.z) as f32);
        let len = dx.hypot(dz);
        let dir = if len > 0.0 {
            (dx / len, dz / len)
        } else {
            (1.0, 0.0)
        };
        for (k, (x, _, z)) in bresenham_line(a.x, 0, a.z, b.x, 0, b.z)
            .into_iter()
            .enumerate()
        {
            if trace.cells.last() != Some(&(x, z)) {
                trace.cells.push((x, z));
                trace.dirs.push(dir);
            }
            if k == 0 {
                mark(&mut trace.marks, a, trace.cells.len() - 1);
            }
        }
        if i + 1 == segments {
            mark(&mut trace.marks, b, trace.cells.len() - 1);
        }
    }
    trace
}

/// The runs of posts a barrier is drawn as, from what became of each traced cell, with
/// its gates and openings. Links between posts avoid cells by `cost`: 0 is clear, 1 is
/// road and 2 building.
fn lay_out(
    trace: &Trace,
    mut spots: Vec<Spot>,
    spaced: bool,
    joined: bool,
    cost: impl Fn(i32, i32) -> u8,
) -> Vec<Vec<Post>> {
    let traced = &trace.cells;
    for &(at, role, width) in &trace.marks {
        match (role, spots[at]) {
            // A gate on a footway spans the path.
            (NodeRole::Gate, Spot::PathGap) => {
                let lo = (0..at)
                    .rev()
                    .take_while(|&j| spots[j] == Spot::PathGap)
                    .last()
                    .unwrap_or(at);
                let hi = (at + 1..spots.len())
                    .take_while(|&j| spots[j] == Spot::PathGap)
                    .last()
                    .unwrap_or(at);
                if hi - lo < MAX_GATE_SPAN {
                    for j in lo..=hi {
                        spots[j] = Spot::Gate(traced[j].0, traced[j].1);
                    }
                }
            }
            // Off any path the gate takes the barrier's place.
            (NodeRole::Gate, Spot::Post(..)) => {
                for j in span(at, width, spots.len()) {
                    if let Spot::Post(x, z) = spots[j] {
                        spots[j] = Spot::Gate(x, z);
                    }
                }
            }
            (NodeRole::Opening, _) => {
                for j in span(at, width, spots.len()) {
                    if matches!(spots[j], Spot::Post(..) | Spot::Gate(..)) {
                        spots[j] = Spot::Opening;
                    }
                }
            }
            // On roads gates stand open.
            _ => {}
        }
    }

    let mut runs: Vec<Vec<Post>> = Vec::new();
    let mut run: Vec<Post> = Vec::new();
    for (j, spot) in spots.iter().enumerate() {
        let (x, z, gate) = match *spot {
            Spot::Post(x, z) => (x, z, None),
            Spot::Gate(x, z) => {
                let (dx, dz) = trace.dirs[j];
                (x, z, Some(dx.abs() >= dz.abs()))
            }
            _ => {
                if !run.is_empty() {
                    runs.push(std::mem::take(&mut run));
                }
                continue;
            }
        };
        if let Some(prev) = run.last_mut() {
            if (prev.x, prev.z) == (x, z) {
                prev.gate = prev.gate.or(gate);
                continue;
            }
            let (from, from_gate) = ((prev.x, prev.z), prev.gate);
            let (dx, dz) = (x - from.0, z - from.1);
            if dx.abs().max(dz.abs()) > 1 {
                let line: Vec<(i32, i32)> = bresenham_line(from.0, 0, from.1, x, 0, z)
                    .into_iter()
                    .map(|(x, _, z)| (x, z))
                    .collect();
                let line = if joined {
                    four_connected_line(&line)
                } else {
                    line
                };
                for &(lx, lz) in &line[1..line.len() - 1] {
                    if cost(lx, lz) == 0 {
                        run.push(Post {
                            x: lx,
                            z: lz,
                            gate: None,
                        });
                    }
                }
            } else if joined && dx != 0 && dz != 0 {
                // Walls and fences only join edge to edge: a diagonal step takes a corner,
                // the clearer one where there is a choice.
                let (cx, cz) = [(x, from.1), (from.0, z)]
                    .into_iter()
                    .min_by_key(|&(cx, cz)| cost(cx, cz))
                    .unwrap();
                // Beside a gate the corner opens too, or the gate leads into a post.
                run.push(Post {
                    x: cx,
                    z: cz,
                    gate: from_gate.or(gate),
                });
            }
        }
        run.push(Post { x, z, gate });
    }
    if !run.is_empty() {
        runs.push(run);
    }
    for run in &mut runs {
        orient_gates(run);
    }
    if spaced {
        for run in &mut runs {
            let mut i = 0;
            run.retain(|_| {
                i += 1;
                i % 2 == 1
            });
        }
    }
    runs
}

/// Turns each gate across the run where its neighbours line up on one axis; at a corner
/// it keeps the heading of the barrier.
fn orient_gates(run: &mut [Post]) {
    for i in 0..run.len() {
        if run[i].gate.is_none() {
            continue;
        }
        let (x, z) = (run[i].x, run[i].z);
        let near = [i.checked_sub(1).map(|j| run[j]), run.get(i + 1).copied()];
        let along_x = near.iter().flatten().any(|p| p.z == z);
        let along_z = near.iter().flatten().any(|p| p.x == x);
        if along_x != along_z {
            run[i].gate = Some(along_x);
        }
    }
}

/// Indices `width` wide centred on `at`, within `len`.
fn span(at: usize, width: usize, len: usize) -> std::ops::RangeInclusive<usize> {
    at.saturating_sub((width - 1) / 2)..=(at + width / 2).min(len - 1)
}

/// Ground-level road centrelines near barriers. Road stamps are wider than the roads they
/// stand for, so a barrier mapped along a road edge often lands on the stamp; the
/// centreline tells that apart from a barrier crossing the road.
pub struct RoadClearance {
    roads: Arc<CoordinateBitmap>,
    segments: Vec<RoadSegment>,
    buckets: FnvHashMap<(i32, i32), Vec<u32>>,
    /// Nodes where two town walls mapped as lines meet.
    wall_joints: FnvHashSet<u64>,
    /// Track at grade near town walls, with its bed.
    rails: FnvHashSet<(i32, i32)>,
    /// Corners of the whole world, which a tile's editor does not span.
    world: ((i32, i32), (i32, i32)),
}

struct RoadSegment {
    a: (f32, f32),
    d: (f32, f32),
    /// Reach of the stamp from the centreline: the road's disc, half a block past its half-width.
    reach: f32,
    carriageway: bool,
    /// Mapped on a layer above the ground, as a wall walk is.
    raised: bool,
}

impl RoadSegment {
    fn new(
        a: &ProcessedNode,
        b: &ProcessedNode,
        reach: f32,
        carriageway: bool,
        raised: bool,
    ) -> Option<Self> {
        let d = ((b.x - a.x) as f32, (b.z - a.z) as f32);
        (d != (0.0, 0.0)).then_some(Self {
            a: (a.x as f32, a.z as f32),
            d,
            reach,
            carriageway,
            raised,
        })
    }

    fn unit(&self) -> (f32, f32) {
        let len = self.d.0.hypot(self.d.1);
        (self.d.0 / len, self.d.1 / len)
    }

    fn distance(&self, x: f32, z: f32) -> f32 {
        let (px, pz) = (x - self.a.0, z - self.a.1);
        let t = ((px * self.d.0 + pz * self.d.1) / (self.d.0 * self.d.0 + self.d.1 * self.d.1))
            .clamp(0.0, 1.0);
        (px - t * self.d.0).hypot(pz - t * self.d.1)
    }
}

fn bucket(x: i32, z: i32) -> (i32, i32) {
    (x >> BUCKET_SHIFT, z >> BUCKET_SHIFT)
}

/// Points along a to b no further apart than SAMPLE_STEP, ends included.
fn samples(a: &ProcessedNode, b: &ProcessedNode) -> impl Iterator<Item = (f32, f32)> {
    let (ax, az) = (a.x as f32, a.z as f32);
    let (dx, dz) = (b.x as f32 - ax, b.z as f32 - az);
    let n = (dx.hypot(dz) / SAMPLE_STEP).ceil().max(1.0) as usize;
    (0..=n).map(move |i| {
        let t = i as f32 / n as f32;
        (ax + dx * t, az + dz * t)
    })
}

impl RoadClearance {
    /// Indexes only the stretches of road near a barrier, so the index stays small however
    /// large the world. `roads` is the rendered road surface.
    pub fn build(
        elements: &[ProcessedElement],
        editor: &WorldEditor,
        scale: f64,
        roads: Arc<CoordinateBitmap>,
    ) -> Self {
        let mut index = Self {
            roads,
            segments: Vec::new(),
            buckets: FnvHashMap::default(),
            wall_joints: FnvHashSet::default(),
            rails: FnvHashSet::default(),
            world: (editor.get_min_coords(), editor.get_max_coords()),
        };
        let ways = || {
            elements.iter().filter_map(|e| match e {
                ProcessedElement::Way(w) => Some(w),
                _ => None,
            })
        };
        // Where town walls meet, and the track that crosses them.
        let mut wall_nodes: FnvHashMap<u64, u32> = FnvHashMap::default();
        let mut wall_buckets: FnvHashSet<(i32, i32)> = FnvHashSet::default();
        for way in ways().filter(|w| fortifications::line_wall(&w.tags)) {
            let mut ids: Vec<u64> = way.nodes.iter().map(|n| n.id).collect();
            ids.sort_unstable();
            ids.dedup();
            for id in ids {
                *wall_nodes.entry(id).or_default() += 1;
            }
            for pair in way.nodes.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                for (x, _, z) in bresenham_line(a.x, 0, a.z, b.x, 0, b.z) {
                    let (bx, bz) = bucket(x, z);
                    for dx in -1..=1 {
                        for dz in -1..=1 {
                            wall_buckets.insert((bx + dx, bz + dz));
                        }
                    }
                }
            }
        }
        index.wall_joints = wall_nodes
            .into_iter()
            .filter(|&(_, n)| n >= 2)
            .map(|(id, _)| id)
            .collect();
        if !wall_buckets.is_empty() {
            for way in ways().filter(|w| super::railways::is_at_grade_track(w)) {
                for (x, z) in super::railways::build_smoothed_centerline(way) {
                    if wall_buckets.contains(&bucket(x, z)) {
                        for dx in -1..=1 {
                            for dz in -1..=1 {
                                index.rails.insert((x + dx, z + dz));
                            }
                        }
                    }
                }
            }
        }
        // Only buckets where a barrier stands on the road surface are ever looked up.
        let mut wanted: FnvHashSet<(i32, i32)> = FnvHashSet::default();
        for way in ways().filter(|w| w.tags.contains_key("barrier")) {
            for pair in way.nodes.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                for (x, _, z) in bresenham_line(a.x, 0, a.z, b.x, 0, b.z) {
                    if index.roads.contains(x, z) {
                        wanted.insert(bucket(x, z));
                    }
                }
            }
        }
        if wanted.is_empty() {
            return index;
        }

        for way in ways() {
            let Some(highway_type) = surface_highway_type(way) else {
                continue;
            };
            // Barriers pass under bridges or run along their decks; the decks handle both.
            if is_bridge_way(way) {
                continue;
            }
            // As far as the disc stamp reaches off a centreline cell, and that cell off the
            // line between the nodes: on a shallow diagonal the edge cells lie past half + 0.5.
            let reach = stamp_radius(highway_block_range(highway_type, &way.tags, scale)) + 0.5;
            let carriageway = is_carriageway(highway_type);
            let raised = way
                .tags
                .get("layer")
                .and_then(|l| l.trim().parse::<i32>().ok())
                .is_some_and(|l| l >= 1);
            // Every cell the stamp covers lies this close to a sample, in both axes.
            let margin = reach + SAMPLE_STEP / 2.0;
            // Decided on the first sample near a barrier, as the tunnel check is costly.
            let mut at_surface: Option<bool> = None;
            'segments: for pair in way.nodes.windows(2) {
                let mut id: Option<u32> = None;
                for (x, z) in samples(&pair[0], &pair[1]) {
                    let (lo_x, lo_z) =
                        bucket((x - margin).floor() as i32, (z - margin).floor() as i32);
                    let (hi_x, hi_z) =
                        bucket((x + margin).floor() as i32, (z + margin).floor() as i32);
                    for b in (lo_x..=hi_x).flat_map(|bx| (lo_z..=hi_z).map(move |bz| (bx, bz))) {
                        if !wanted.contains(&b) {
                            continue;
                        }
                        if !*at_surface.get_or_insert_with(|| {
                            tunnel_renders_at_surface(way, Some(editor), scale)
                        }) {
                            break 'segments;
                        }
                        let seg = match id {
                            Some(seg) => seg,
                            None => {
                                let Some(segment) = RoadSegment::new(
                                    &pair[0],
                                    &pair[1],
                                    reach,
                                    carriageway,
                                    raised,
                                ) else {
                                    continue 'segments;
                                };
                                index.segments.push(segment);
                                *id.insert((index.segments.len() - 1) as u32)
                            }
                        };
                        let list = index.buckets.entry(b).or_default();
                        if !list.contains(&seg) {
                            list.push(seg);
                        }
                    }
                }
            }
        }
        index
            .buckets
            .values_mut()
            .for_each(|list| list.shrink_to_fit());
        index
    }

    fn on_road(&self, x: i32, z: i32) -> bool {
        self.roads.contains(x, z)
    }

    /// Where a town wall cell heading `dir` goes: arched over a road or track it crosses,
    /// beside a road it runs along, and kept where a path runs along it or over it, as a
    /// wall walk does.
    fn place_town_wall(
        &self,
        x: i32,
        z: i32,
        dir: (f32, f32),
        buildings: &CoordinateBitmap,
    ) -> Spot {
        if self.rails.contains(&(x, z)) {
            return Spot::RoadGap;
        }
        if self.roads.contains(x, z) {
            if let Some(path) = self.covering(x, z).filter(|s| !s.carriageway) {
                let (ux, uz) = path.unit();
                if path.raised || (dir.0 * ux + dir.1 * uz).abs() >= ALONG_COS {
                    return Spot::Post(x, z);
                }
            }
        }
        self.place(x, z, dir, false, buildings)
    }

    /// Whether a town wall keeps off this cell unless it crosses there: a road for traffic,
    /// one it does not know, or track.
    fn keeps_wall_off(&self, x: i32, z: i32) -> bool {
        self.rails.contains(&(x, z))
            || (self.roads.contains(x, z) && self.covering(x, z).is_none_or(|s| s.carriageway))
    }

    /// The indexed road whose stamp covers the cell, nearest centreline first.
    fn covering(&self, x: i32, z: i32) -> Option<&RoadSegment> {
        let (fx, fz) = (x as f32, z as f32);
        self.buckets
            .get(&bucket(x, z))?
            .iter()
            .map(|&i| &self.segments[i as usize])
            .filter(|s| s.distance(fx, fz) <= s.reach)
            .min_by(|a, b| a.distance(fx, fz).total_cmp(&b.distance(fx, fz)))
    }

    /// Where a barrier cell heading `dir` goes. Beside a road it moves off the surface,
    /// away from the centreline; across one it leaves the road open unless the barrier
    /// is meant to stand on roads.
    fn place(
        &self,
        x: i32,
        z: i32,
        dir: (f32, f32),
        crosses_roads: bool,
        buildings: &CoordinateBitmap,
    ) -> Spot {
        if !self.roads.contains(x, z) {
            return Spot::Post(x, z);
        }
        let Some(road) = self.covering(x, z) else {
            return Spot::Post(x, z);
        };
        let (ux, uz) = road.unit();
        if (dir.0 * ux + dir.1 * uz).abs() < ALONG_COS {
            return match (crosses_roads, road.carriageway) {
                (true, _) => Spot::Post(x, z),
                (false, true) => Spot::RoadGap,
                (false, false) => Spot::PathGap,
            };
        }
        let side = ux * (z as f32 - road.a.1) - uz * (x as f32 - road.a.0);
        // On the centreline, as a median is: kept as mapped.
        if side.abs() < 0.5 {
            return Spot::Post(x, z);
        }
        let (nx, nz) = (-uz * side.signum(), ux * side.signum());
        let mut edge: Option<(i32, (i32, i32))> = None;
        // Furthest out short of a building, for a barrier with a building right behind it.
        let mut last = (x, z);
        for k in 1..=MAX_SHIFT {
            let c = (
                (x as f32 + nx * k as f32).round() as i32,
                (z as f32 + nz * k as f32).round() as i32,
            );
            if buildings.contains(c.0, c.1) {
                break;
            }
            if !self.roads.contains(c.0, c.1) {
                return Spot::Post(c.0, c.1);
            }
            last = c;
            match edge {
                None if road.distance(c.0 as f32, c.1 as f32) > road.reach => edge = Some((k, c)),
                Some((k0, _)) if k >= k0 + EXTRA_SHIFT => break,
                _ => {}
            }
        }
        // Squeezed between two roads, as a kerb railing is: at its own road's edge.
        let (ex, ez) = edge.map_or(last, |(_, c)| c);
        Spot::Post(ex, ez)
    }
}

/// Y each barrier cell stands on: the deck along stretches that run on a bridge, the terrain
/// elsewhere (including under a bridge), None where the cell would hang in the air. Relies on
/// the priority order rendering highways, and so their decks, before barriers.
fn barrier_bases(
    editor: &WorldEditor,
    surface: &BridgeSurfaceMap,
    cells: &[(i32, i32)],
) -> Vec<Option<i32>> {
    let ground = |(x, z): (i32, i32)| editor.get_absolute_y(x, 0, z);
    let deck = |(x, z): (i32, i32)| surface.nearby_deck_y(x, z, BRIDGE_BARRIER_NEARBY_RADIUS);
    deck_cells(editor, surface, cells)
        .into_iter()
        .zip(cells)
        .map(|(on_deck, &cell)| {
            if !on_deck {
                return Some(ground(cell));
            }
            surface
                .supported_top(editor, cell.0, cell.1, BRIDGE_BARRIER_NEARBY_RADIUS)
                .or_else(|| {
                    // Beside the deck: only where the deck is still at grade.
                    let g = ground(cell);
                    deck(cell).is_some_and(|d| d - g <= 1).then_some(g)
                })
        })
        .collect()
}

/// Whether each cell of a barrier line belongs on a bridge deck: a stretch near a deck gets
/// onto it only where the deck meets the ground; otherwise it passes underneath.
fn deck_cells(editor: &WorldEditor, surface: &BridgeSurfaceMap, cells: &[(i32, i32)]) -> Vec<bool> {
    let mut on_deck = vec![false; cells.len()];
    if surface.is_empty() {
        return on_deck;
    }
    let ground = |(x, z): (i32, i32)| editor.get_absolute_y(x, 0, z);
    let deck = |(x, z): (i32, i32)| surface.nearby_deck_y(x, z, BRIDGE_BARRIER_NEARBY_RADIUS);
    let mut i = 0;
    while i < cells.len() {
        if deck(cells[i]).is_none() {
            i += 1;
            continue;
        }
        let start = i;
        while i < cells.len() && deck(cells[i]).is_some() {
            i += 1;
        }
        let at_grade = |inside: usize, outside: usize| {
            deck(cells[inside]).is_some_and(|d| d - ground(cells[outside]) <= 2)
        };
        let on_bridge = (start == 0 && i == cells.len())
            || (start > 0 && at_grade(start, start - 1))
            || (i < cells.len() && at_grade(i - 1, i));
        if on_bridge {
            on_deck[start..i].fill(true);
        }
    }
    on_deck
}

pub fn generate_barrier_nodes(
    editor: &mut WorldEditor<'_>,
    node: &ProcessedNode,
    bridge_surface: &BridgeSurfaceMap,
) {
    // On a deck only where the deck is in this very column; beside it, the terrain.
    let deck = bridge_surface
        .contains(node.x, node.z)
        .then(|| bridge_surface.supported_top(editor, node.x, node.z, BRIDGE_BARRIER_NEARBY_RADIUS))
        .flatten();
    let place = |editor: &mut WorldEditor<'_>, block: Block, dy: i32| match deck {
        Some(deck_y) => editor.set_block_absolute(block, node.x, deck_y + dy, node.z, None, None),
        None => editor.set_block(block, node.x, dy, node.z, None, None),
    };
    match node.tags.get("barrier").map(|s| s.as_str()) {
        Some("bollard") => {
            place(editor, COBBLESTONE_WALL, 1);
        }
        Some("block") => {
            place(editor, STONE, 1);
        }
        Some("entrance") => {
            place(editor, AIR, 1);
        }
        None => {}
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::bridge_styles::BridgeOutlineIndex;
    use crate::element_processing::bridges::BridgeStructureMap;
    use crate::element_processing::building_test_support::{rect_way, tag_map, test_editor};
    use crate::element_processing::highways::build_highway_connectivity_map;
    use crate::element_processing::highways::collect_road_surface_coords;
    use crate::osm_parser::ProcessedWay;
    use std::collections::HashMap;

    fn way(id: u64, tags: &[(&str, &str)], points: &[(i32, i32)]) -> ProcessedWay {
        ProcessedWay {
            id,
            nodes: points
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: id * 100 + i as u64,
                    tags: Default::default(),
                    x,
                    z,
                })
                .collect(),
            tags: tag_map(tags),
        }
    }

    /// A road along x at `z`, across a 100-block world.
    fn road(id: u64, highway: &str, z: i32) -> ProcessedElement {
        ProcessedElement::Way(way(id, &[("highway", highway)], &[(0, z), (99, z)]))
    }

    fn reach(highway: &str) -> i32 {
        highway_block_range(highway, &HashMap::new(), 1.0)
    }

    /// A barrier way whose node `at` carries `node_tags`.
    fn barrier(
        id: u64,
        tags: &[(&str, &str)],
        points: &[(i32, i32)],
        at: usize,
        node_tags: &[(&str, &str)],
    ) -> ProcessedElement {
        let mut w = way(id, tags, points);
        w.nodes[at].tags = tag_map(node_tags);
        ProcessedElement::Way(w)
    }

    fn fence(id: u64, points: &[(i32, i32)]) -> ProcessedElement {
        ProcessedElement::Way(way(id, &[("barrier", "fence")], points))
    }

    /// Draws every barrier in `elements` the way a world run does, against its roads.
    fn render<R>(
        size: f64,
        elements: &[ProcessedElement],
        read: impl FnOnce(&WorldEditor, &RoadClearance) -> R,
    ) -> R {
        render_beside(size, elements, &[], read)
    }

    /// As `render`, with building footprints over the given cells.
    fn render_beside<R>(
        size: f64,
        elements: &[ProcessedElement],
        building_cells: &[(i32, i32)],
        read: impl FnOnce(&WorldEditor, &RoadClearance) -> R,
    ) -> R {
        let xzbbox = XZBBox::rect_from_xz_lengths(size, size).unwrap();
        let mut editor = test_editor(&xzbbox);
        let outlines = BridgeOutlineIndex::build(elements);
        let structures = BridgeStructureMap::build(
            elements,
            &editor,
            &build_highway_connectivity_map(elements),
            &outlines,
            1.0,
        );
        let surface = BridgeSurfaceMap::build(elements, &structures, 1.0).with_parts(
            elements,
            &structures,
            &editor,
            1.0,
        );
        let roads = Arc::new(collect_road_surface_coords(
            elements,
            &editor,
            &xzbbox,
            1.0,
            Default::default(),
        ));
        let clearance = RoadClearance::build(elements, &editor, 1.0, roads);
        let mut buildings = CoordinateBitmap::new(&xzbbox);
        for &(x, z) in building_cells {
            buildings.set(x, z);
        }
        for element in elements.iter().filter(|e| e.tags().contains_key("barrier")) {
            generate_barriers(&mut editor, element, &surface, &clearance, &buildings);
        }
        read(&editor, &clearance)
    }

    fn post(editor: &WorldEditor, x: i32, z: i32) -> bool {
        editor.block_exists_absolute(x, 1, z)
    }

    #[test]
    fn barriers_rest_on_the_deck_or_the_ground_never_in_between() {
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let bridge = way(
            1,
            &[
                ("highway", "residential"),
                ("bridge", "yes"),
                ("layer", "1"),
            ],
            &[(10, 40), (70, 40)],
        );
        let wall = |id, points: &[(i32, i32)]| {
            ProcessedElement::Way(way(id, &[("barrier", "wall")], points))
        };
        let along = wall(2, &[(12, 40), (68, 40)]);
        let under = wall(3, &[(35, 10), (35, 70)]);
        let beside = wall(4, &[(12, 44), (68, 44)]);
        // A ground road crossing under the bridge must not cut the railing on the deck.
        let below =
            ProcessedElement::Way(way(5, &[("highway", "residential")], &[(40, 0), (40, 99)]));
        let elements = vec![
            ProcessedElement::Way(bridge),
            below,
            along.clone(),
            under.clone(),
            beside.clone(),
        ];
        let outlines = BridgeOutlineIndex::build(&elements);
        let structures = BridgeStructureMap::build(
            &elements,
            &editor,
            &build_highway_connectivity_map(&elements),
            &outlines,
            1.0,
        );
        let surface = BridgeSurfaceMap::build(&elements, &structures, 1.0);
        let roads = Arc::new(collect_road_surface_coords(
            &elements,
            &editor,
            &xzbbox,
            1.0,
            Default::default(),
        ));
        let clearance = RoadClearance::build(&elements, &editor, 1.0, roads);
        let buildings = CoordinateBitmap::new_empty();
        let member = structures.lookup_member(1).unwrap();
        // Stand in for the rendered deck: one row at the deck Y over the road stamp.
        for x in 10..=70 {
            for z in 38..=42 {
                editor.set_block_absolute(STONE, x, member.y_at((x - 10) as usize), z, None, None);
            }
        }
        let deck_y = member.y_at(30);
        assert!(deck_y >= 4, "the span is raised");

        // Along the deck: left to the bridge, which keeps its deck clear.
        generate_barriers(&mut editor, &along, &surface, &clearance, &buildings);
        assert!(!editor.block_exists_absolute(40, deck_y + 1, 40));
        assert!(!editor.block_exists_absolute(40, 1, 40));
        // Passing underneath: on the ground, not cut open for the road above.
        generate_barriers(&mut editor, &under, &surface, &clearance, &buildings);
        assert!(editor.block_exists_absolute(35, 1, 40));
        // Off the deck edge mid-span: not hanging beside it.
        generate_barriers(&mut editor, &beside, &surface, &clearance, &buildings);
        assert!(!editor.block_exists_absolute(40, deck_y + 1, 44));
    }

    #[test]
    fn a_barrier_across_a_low_deck_leaves_its_road_open() {
        let r = reach("residential");
        let bridge = ProcessedElement::Way(way(
            1,
            &[("highway", "residential"), ("bridge", "yes")],
            &[(10, 40), (70, 40)],
        ));
        let elements = [bridge, fence(2, &[(40, 10), (40, 70)])];
        render(100.0, &elements, |editor, _| {
            for z in 40 - r..=40 + r {
                assert!(!post(editor, 40, z), "open at z={z}");
            }
            assert!(post(editor, 40, 20) && post(editor, 40, 60));
        });
    }

    #[test]
    fn a_wall_mapped_along_a_bridge_leaves_it_its_own_railing() {
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let edge = 40 + reach("residential") + 1;
        let elements = vec![
            ProcessedElement::Way(way(
                1,
                &[
                    ("highway", "residential"),
                    ("bridge", "yes"),
                    ("layer", "1"),
                ],
                &[(10, 40), (70, 40)],
            )),
            ProcessedElement::Way(way(2, &[("barrier", "wall")], &[(12, edge), (68, edge)])),
        ];
        let outlines = BridgeOutlineIndex::build(&elements);
        let structures = BridgeStructureMap::build(
            &elements,
            &editor,
            &build_highway_connectivity_map(&elements),
            &outlines,
            1.0,
        );
        let surface = BridgeSurfaceMap::build(&elements, &structures, 1.0).with_parts(
            &elements,
            &structures,
            &editor,
            1.0,
        );
        let member = structures.lookup_member(1).unwrap();
        let design = structures.structure_of(member).design;
        crate::element_processing::bridge_decks::render(
            &mut editor,
            design,
            surface.parts_of(1).unwrap(),
        );
        let deck_y = member.y_at(30);
        let railing = editor.get_block_absolute(40, deck_y + 1, edge);
        assert!(
            railing.is_some(),
            "the bridge stands its railing on the edge"
        );
        let roads = Arc::new(collect_road_surface_coords(
            &elements,
            &editor,
            &xzbbox,
            1.0,
            Default::default(),
        ));
        let clearance = RoadClearance::build(&elements, &editor, 1.0, roads);
        let buildings = CoordinateBitmap::new_empty();
        generate_barriers(&mut editor, &elements[1], &surface, &clearance, &buildings);
        assert_eq!(editor.get_block_absolute(40, deck_y + 1, edge), railing);
        assert!(
            !editor.block_exists_absolute(40, deck_y + 2, edge),
            "nothing stacks on the railing"
        );
    }

    #[test]
    fn a_barrier_across_a_road_leaves_the_road_open() {
        let r = reach("residential");
        let elements = [road(1, "residential", 40), fence(2, &[(50, 10), (50, 90)])];
        render(100.0, &elements, |editor, _| {
            for z in 40 - r..=40 + r {
                assert!(!post(editor, 50, z), "open at z={z}");
            }
            assert!(post(editor, 50, 40 - r - 1) && post(editor, 50, 40 + r + 1));
            assert!(post(editor, 50, 20) && post(editor, 50, 80));
        });
    }

    #[test]
    fn a_barrier_drawn_beside_a_road_moves_off_its_surface_to_its_side() {
        let r = reach("secondary");
        let elements = [
            road(1, "secondary", 40),
            fence(2, &[(10, 41), (90, 41)]),
            fence(3, &[(10, 38), (90, 38)]),
        ];
        render(100.0, &elements, |editor, _| {
            for x in [20, 50, 80] {
                for z in 40 - r..=40 + r {
                    assert!(!post(editor, x, z), "road clear at ({x}, {z})");
                }
                assert!(post(editor, x, 40 + r + 1), "north fence at its edge");
                assert!(post(editor, x, 40 - r - 1), "south fence at its edge");
            }
        });
    }

    #[test]
    fn a_barrier_on_a_road_centreline_stays_as_a_median() {
        let elements = [
            road(1, "primary", 40),
            ProcessedElement::Way(way(2, &[("barrier", "guard_rail")], &[(10, 40), (90, 40)])),
        ];
        render(100.0, &elements, |editor, _| {
            assert!(post(editor, 50, 40));
            assert!(
                !editor.block_exists_absolute(50, 2, 40),
                "a guard rail is one high"
            );
        });
    }

    #[test]
    fn gates_span_the_path_they_cross_and_stand_open_on_roads() {
        let points = [(50, 10), (50, 40), (50, 90)];
        let gate = [("barrier", "gate")];
        let on_path = [
            road(1, "footway", 40),
            barrier(2, &[("barrier", "fence")], &points, 1, &gate),
        ];
        let r = reach("footway");
        render(100.0, &on_path, |editor, _| {
            for z in 40 - r..=40 + r {
                assert_eq!(editor.get_block_absolute(50, 1, z), Some(OAK_FENCE_GATE));
                assert!(
                    !editor.block_exists_absolute(50, 2, z),
                    "nothing over the gate"
                );
            }
            let facing = match editor.block_properties_absolute(50, 1, 40) {
                Some(fastnbt::Value::Compound(p)) => p.get("facing").cloned(),
                _ => None,
            };
            assert_eq!(facing, Some(fastnbt::Value::String("east".into())));
            assert!(post(editor, 50, 30));
        });

        let on_road = [
            road(1, "residential", 40),
            barrier(2, &[("barrier", "fence")], &points, 1, &gate),
        ];
        let r = reach("residential");
        render(100.0, &on_road, |editor, _| {
            for z in 40 - r..=40 + r {
                assert!(!post(editor, 50, z), "open at z={z}");
            }
        });

        // Off any road, a gate takes the place of the fence.
        let alone = [barrier(2, &[("barrier", "fence")], &points, 1, &gate)];
        render(100.0, &alone, |editor, _| {
            assert_eq!(editor.get_block_absolute(50, 1, 40), Some(OAK_FENCE_GATE));
            assert!(post(editor, 50, 39) && post(editor, 50, 41));
        });
    }

    #[test]
    fn an_entrance_on_a_barrier_leaves_an_opening() {
        let elements = [barrier(
            1,
            &[("barrier", "wall")],
            &[(50, 10), (50, 50), (50, 90)],
            1,
            &[("barrier", "entrance")],
        )];
        render(100.0, &elements, |editor, _| {
            assert!(!post(editor, 50, 50));
            assert!(post(editor, 50, 49) && post(editor, 50, 51));
        });
    }

    #[test]
    fn bollards_stand_apart_and_stay_on_the_road() {
        let r = reach("residential");
        let elements = [
            road(1, "residential", 40),
            ProcessedElement::Way(way(2, &[("barrier", "bollard")], &[(50, 10), (50, 90)])),
        ];
        render(100.0, &elements, |editor, _| {
            assert!((40 - r..=40 + r).any(|z| post(editor, 50, z)));
            for z in 10..90 {
                assert!(
                    !(post(editor, 50, z) && post(editor, 50, z + 1)),
                    "gap at z={z}"
                );
            }
            assert!(!editor.block_exists_absolute(50, 2, 40));
        });
        // Bollards mapped on each node of the line keep the line whole.
        let tagged = [barrier(
            1,
            &[("barrier", "bollard")],
            &[(50, 10), (50, 50), (50, 90)],
            1,
            &[("barrier", "bollard")],
        )];
        render(100.0, &tagged, |editor, _| {
            assert!((48..=52).filter(|&z| post(editor, 50, z)).count() >= 2);
        });
    }

    #[test]
    fn only_roads_near_a_barrier_are_indexed() {
        let far = [road(1, "residential", 290), fence(2, &[(10, 10), (20, 10)])];
        render(300.0, &far, |_, clearance| {
            assert!(clearance.segments.is_empty());
        });
        let beside = [road(1, "residential", 30), fence(2, &[(10, 10), (20, 10)])];
        render(300.0, &beside, |_, clearance| {
            assert!(
                clearance.segments.is_empty(),
                "a barrier off the road needs no index"
            );
        });
        let near = [road(1, "residential", 15), fence(2, &[(15, 5), (15, 25)])];
        render(300.0, &near, |_, clearance| {
            assert_eq!(clearance.segments.len(), 1);
        });
        let no_barrier = [road(1, "residential", 15)];
        render(300.0, &no_barrier, |_, clearance| {
            assert!(clearance.segments.is_empty() && clearance.buckets.is_empty());
        });
    }

    #[test]
    fn a_gate_in_a_diagonal_wall_leaves_a_way_through() {
        let elements = [barrier(
            1,
            &[("barrier", "wall")],
            &[(10, 10), (20, 20), (30, 30)],
            1,
            &[("barrier", "gate")],
        )];
        render(100.0, &elements, |editor, _| {
            assert_eq!(editor.get_block_absolute(20, 1, 20), Some(OAK_FENCE_GATE));
            // The staircase cells beside the gate open with it, so it can be walked through.
            for (x, z) in [(20, 19), (21, 20)] {
                assert_eq!(editor.get_block_absolute(x, 1, z), Some(OAK_FENCE_GATE));
                assert!(!editor.block_exists_absolute(x, 2, z));
            }
            assert_eq!(editor.get_block_absolute(19, 1, 19), Some(STONE_BRICK_WALL));
        });
    }

    #[test]
    fn a_barrier_beside_a_diagonal_road_steps_around_its_surface() {
        let elements = [
            ProcessedElement::Way(way(1, &[("highway", "residential")], &[(0, 0), (99, 99)])),
            fence(2, &[(10, 13), (85, 88)]),
        ];
        render(100.0, &elements, |editor, clearance| {
            let mut posts = 0;
            for x in 20..75 {
                for z in 20..80 {
                    if post(editor, x, z) {
                        posts += 1;
                        assert!(!clearance.on_road(x, z), "post on the road at ({x}, {z})");
                    }
                }
            }
            assert!(posts > 50);
        });
    }

    #[test]
    fn a_barrier_moved_off_a_road_stays_out_of_buildings() {
        let r = reach("residential");
        let building: Vec<(i32, i32)> = (50..70)
            .flat_map(|x| (40 + r + 1..40 + r + 8).map(move |z| (x, z)))
            .collect();
        let elements = [road(1, "residential", 40), fence(2, &[(10, 41), (90, 41)])];
        render_beside(100.0, &elements, &building, |editor, _| {
            for &(x, z) in &building {
                assert!(!post(editor, x, z), "post in the building at ({x}, {z})");
            }
            assert!(post(editor, 30, 40 + r + 1));
        });
    }

    fn draw<R>(tags: &[(&str, &str)], read: impl FnOnce(&WorldEditor) -> R) -> R {
        let elements = [ProcessedElement::Way(rect_way(1, 5, 5, 30, 30, tags))];
        render(40.0, &elements, |editor, _| read(editor))
    }

    fn capped(tags: &[(&str, &str)]) -> bool {
        draw(tags, |editor| {
            (1..=4).any(|y| editor.check_for_block(15, y, 5, Some(&[STONE_BRICK_SLAB])))
        })
    }

    fn on_soil(tags: &[(&str, &str)]) -> bool {
        draw(tags, |editor| {
            editor.check_for_block(15, 0, 5, Some(&[GRASS_BLOCK]))
        })
    }

    fn height(tags: &[(&str, &str)]) -> i32 {
        draw(tags, |editor| {
            (1..=6)
                .take_while(|&y| editor.block_exists_absolute(15, y, 5))
                .count() as i32
        })
    }

    #[test]
    fn hedges_grow_from_soil_and_only_masonry_gets_a_coping() {
        assert!(capped(&[("barrier", "wall")]));
        assert!(!capped(&[("barrier", "hedge")]));
        assert!(!capped(&[("barrier", "fence"), ("fence_type", "wood")]));
        assert!(on_soil(&[("barrier", "hedge")]));
        assert!(!on_soil(&[("barrier", "wall")]));
    }

    #[test]
    fn hedges_mark_their_columns_so_plant_cleanups_spare_them() {
        assert!(draw(&[("barrier", "hedge")], |editor| editor.is_hedge(15, 5)));
        assert!(!draw(&[("barrier", "wall")], |editor| editor.is_hedge(15, 5)));
        // Every mapped barrier marks its columns, hedge or not.
        assert!(draw(&[("barrier", "wall")], |editor| editor.is_barrier(15, 5)));
        assert!(draw(&[("barrier", "hedge")], |editor| editor.is_barrier(15, 5)));
    }

    #[test]
    fn barriers_keep_their_height() {
        // Masonry over one block carries a coping slab.
        assert_eq!(height(&[("barrier", "wall")]), 4);
        assert_eq!(height(&[("barrier", "fence"), ("fence_type", "wood")]), 2);
        assert_eq!(
            height(&[("barrier", "fence"), ("fence_type", "railing")]),
            1
        );
        assert_eq!(height(&[("barrier", "fence"), ("height", "1")]), 1);
        assert_eq!(height(&[("barrier", "retaining_wall")]), 1);
        assert_eq!(height(&[("barrier", "kerb")]), 0);
    }

    #[test]
    fn town_walls_are_thick_crenellated_and_arched_over_roads() {
        let elements = [
            road(1, "residential", 20),
            ProcessedElement::Way(way(2, &[("barrier", "city_wall")], &[(15, 5), (15, 35)])),
        ];
        render(40.0, &elements, |editor, _| {
            let solid = |x, z, y| editor.block_exists_absolute(x, y, z);
            for x in 14..=16 {
                assert!((1..=8).all(|y| solid(x, 10, y)), "wall at x {x}");
            }
            assert!(!solid(13, 10, 1) && !solid(17, 10, 1));
            // Merlons on both faces, one block in two.
            assert!(solid(14, 10, 9) != solid(14, 11, 9));
            assert!(solid(16, 10, 9) != solid(16, 11, 9));
            // The road passes under an arch.
            assert!((1..=4).all(|y| !solid(15, 20, y)));
            assert!(solid(15, 20, 8));
        });
    }

    #[test]
    fn a_town_wall_clipped_along_the_world_edge_stands_only_inside() {
        // The clipper runs a wall leaving the area along its edge.
        let elements = [ProcessedElement::Way(way(
            1,
            &[("barrier", "city_wall")],
            &[(20, 40), (0, 40), (0, 20), (20, 20)],
        ))];
        render(40.0, &elements, |editor, _| {
            let solid = |x, z| (1..=8).any(|y| editor.block_exists_absolute(x, y, z));
            assert!((25..=35).all(|z| !solid(0, z) && !solid(1, z)));
            assert!((5..=15).all(|x| !solid(x, 40) && !solid(x, 39)));
            assert!((5..=15).all(|x| solid(x, 20)));
        });
    }

    #[test]
    fn a_wall_walk_on_a_town_wall_keeps_it_whole() {
        // Ramparts: a footway mapped along the top of the wall.
        let elements = [
            ProcessedElement::Way(way(1, &[("highway", "footway")], &[(15, 5), (15, 35)])),
            ProcessedElement::Way(way(2, &[("barrier", "city_wall")], &[(15, 5), (15, 35)])),
        ];
        render(40.0, &elements, |editor, _| {
            for x in 14..=16 {
                assert!((1..=8).all(|y| editor.block_exists_absolute(x, y, 20)));
            }
        });
    }

    #[test]
    fn town_walls_meeting_carry_no_merlons_across_their_walk() {
        let mut first = way(1, &[("barrier", "city_wall")], &[(15, 5), (15, 20)]);
        let mut second = way(2, &[("barrier", "city_wall")], &[(15, 20), (15, 35)]);
        first.nodes[1].id = 7;
        second.nodes[0].id = 7;
        let elements = [ProcessedElement::Way(first), ProcessedElement::Way(second)];
        render(40.0, &elements, |editor, _| {
            assert!((17..=23).all(|z| !editor.block_exists_absolute(15, 9, z)));
            assert!((1..=8).all(|y| editor.block_exists_absolute(15, y, 20)));
        });
    }

    #[test]
    fn a_town_wall_is_arched_over_track_and_at_a_gate_mapped_on_it() {
        let elements = [
            ProcessedElement::Way(way(1, &[("railway", "rail")], &[(0, 20), (39, 20)])),
            ProcessedElement::Way(way(2, &[("barrier", "city_wall")], &[(15, 5), (15, 35)])),
            barrier(
                3,
                &[("barrier", "city_wall")],
                &[(25, 5), (25, 12), (25, 18)],
                1,
                &[("historic", "city_gate")],
            ),
        ];
        render(40.0, &elements, |editor, _| {
            let solid = |x, z, y| editor.block_exists_absolute(x, y, z);
            assert!((1..=4).all(|y| !solid(15, 20, y)));
            assert!(solid(15, 20, 8));
            assert!((1..=4).all(|y| !solid(25, 12, y)));
            assert!(solid(25, 8, 4));
        });
    }

    #[test]
    fn a_town_wall_beside_a_road_keeps_off_it() {
        let elements = [
            road(1, "residential", 20),
            ProcessedElement::Way(way(2, &[("barrier", "city_wall")], &[(5, 22), (75, 22)])),
        ];
        render(80.0, &elements, |editor, clearance| {
            for x in 10..=70 {
                for z in 12..=28 {
                    if clearance.on_road(x, z) {
                        assert!(
                            !editor.block_exists_absolute(x, 4, z),
                            "on the road at {x} {z}"
                        );
                    }
                }
                assert!((12..=28).any(|z| editor.block_exists_absolute(x, 8, z)));
            }
        });
    }

    #[test]
    fn a_low_town_wall_is_open_over_the_road_and_a_junk_width_falls_back() {
        let tags = [("barrier", "city_wall"), ("height", "5"), ("width", "300")];
        let elements = [
            road(1, "residential", 20),
            ProcessedElement::Way(way(2, &tags, &[(15, 5), (15, 35)])),
        ];
        render(40.0, &elements, |editor, _| {
            let solid = |x, z, y| editor.block_exists_absolute(x, y, z);
            // Too low for a lintel: open to the sky, with no merlons over the road.
            assert!((1..=7).all(|y| !solid(15, 20, y)));
            assert!((1..=5).all(|y| solid(15, 10, y)));
            assert!(!solid(13, 10, 1) && !solid(17, 10, 1));
        });
    }

    #[test]
    fn a_town_wall_makes_way_for_a_bridge_over_it() {
        let bridge = way(
            1,
            &[
                ("highway", "residential"),
                ("bridge", "yes"),
                ("layer", "1"),
            ],
            &[(10, 40), (70, 40)],
        );
        let wall = way(2, &[("barrier", "city_wall")], &[(40, 10), (40, 70)]);
        let elements = [ProcessedElement::Way(bridge), ProcessedElement::Way(wall)];
        render(100.0, &elements, |editor, _| {
            assert!((1..=12).all(|y| !editor.block_exists_absolute(40, y, 40)));
            assert!((1..=8).all(|y| editor.block_exists_absolute(40, y, 20)));
        });
    }

    #[test]
    fn absurd_heights_fall_back_to_the_barrier_default() {
        for junk in ["inf", "1e10", "NaN", "-1e10"] {
            assert_eq!(height(&[("barrier", "wall"), ("min_height", junk)]), 4);
            assert_eq!(height(&[("barrier", "wall"), ("height", junk)]), 4);
        }
        let floating = [
            ("barrier", "wall"),
            ("min_height", "3"),
            ("height", "-1e10"),
        ];
        let blocks = draw(&floating, |editor| {
            (1..=10)
                .filter(|&y| editor.block_exists_absolute(15, y, 5))
                .count()
        });
        assert_eq!(blocks, 4);
    }
}
