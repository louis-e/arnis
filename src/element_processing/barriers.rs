use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::connected_blocks::{
    four_connected_line, is_connectable, place_connected,
};
use crate::osm_parser::{ProcessedElement, ProcessedNode};
use crate::world_editor::WorldEditor;

const BRIDGE_BARRIER_NEARBY_RADIUS: i32 = 2;

pub fn generate_barriers(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    bridge_surface: &BridgeSurfaceMap,
) {
    // Default values
    let mut barrier_material: Block = COBBLESTONE_WALL;
    let mut barrier_height: i32 = 2;

    match element.tags().get("barrier").map(|s| s.as_str()) {
        Some("bollard") => {
            barrier_material = COBBLESTONE_WALL;
            barrier_height = 1;
        }
        Some("kerb") => {
            // Ignore kerbs
            return;
        }
        Some("hedge") => {
            barrier_material = OAK_LEAVES;
            barrier_height = 2;
        }
        Some("fence") => {
            // Handle fence sub-types
            match element.tags().get("fence_type").map(|s| s.as_str()) {
                Some("railing" | "bars" | "krest") => {
                    barrier_material = STONE_BRICK_WALL;
                    barrier_height = 1;
                }
                Some(
                    "chain_link" | "metal" | "wire" | "barbed_wire" | "corrugated_metal"
                    | "electric" | "metal_bars",
                ) => {
                    barrier_material = STONE_BRICK_WALL; // IRON_BARS
                    barrier_height = 2;
                }
                Some("slatted" | "paling") => {
                    barrier_material = OAK_FENCE;
                    barrier_height = 1;
                }
                Some("wood" | "split_rail" | "panel" | "pole") => {
                    barrier_material = OAK_FENCE;
                    barrier_height = 2;
                }
                Some("concrete" | "stone") => {
                    barrier_material = STONE_BRICK_WALL;
                    barrier_height = 2;
                }
                Some("glass") => {
                    barrier_material = GLASS;
                    barrier_height = 1;
                }
                _ => {}
            }
        }
        Some("wall") => {
            barrier_material = STONE_BRICK_WALL;
            barrier_height = 3;
        }
        _ => {}
    }
    // Tagged material takes priority over inferred
    if let Some(barrier_mat) = element.tags().get("material") {
        if barrier_mat == "brick" {
            barrier_material = BRICK;
        }
        if barrier_mat == "concrete" {
            barrier_material = LIGHT_GRAY_CONCRETE;
        }
        if barrier_mat == "metal" {
            barrier_material = STONE_BRICK_WALL;
        }
        if barrier_mat == "wood" {
            barrier_material = OAK_FENCE;
        }
    }

    if let ProcessedElement::Way(way) = element {
        // Determine wall height
        let wall_height: i32 = element
            .tags()
            .get("height")
            .and_then(|height: &String| height.parse::<f32>().ok())
            .map(|height: f32| height.round() as i32)
            .unwrap_or(barrier_height)
            .max(2); // Minimum height of 2
        let joined = is_connectable(barrier_material);
        // Only masonry gets a coping.
        let capped = wall_height > 1 && !matches!(barrier_material, OAK_LEAVES | OAK_FENCE);
        let hedge = barrier_material == OAK_LEAVES;

        let mut cells: Vec<(i32, i32)> = Vec::new();
        for pair in way.nodes.windows(2) {
            for (x, _, z) in bresenham_line(pair[0].x, 0, pair[0].z, pair[1].x, 0, pair[1].z) {
                if cells.last() != Some(&(x, z)) {
                    cells.push((x, z));
                }
            }
        }
        // Walls and fences only join edge to edge.
        if joined {
            cells = four_connected_line(&cells);
        }

        let bases = barrier_bases(editor, bridge_surface, &cells);
        for (&(bx, bz), base) in cells.iter().zip(bases) {
            let Some(base) = base else {
                continue;
            };
            // Hedges stand on soil, not on the paving beside the lawn they edge.
            if hedge
                && !editor.surface_is_sealed(bx, bz)
                && base == editor.get_absolute_y(bx, 0, bz)
            {
                editor.set_block_absolute(GRASS_BLOCK, bx, base, bz, None, None);
            }
            for y in 1..=wall_height {
                if joined {
                    place_connected(editor, barrier_material, bx, base + y, bz);
                } else {
                    editor.set_block_absolute(barrier_material, bx, base + y, bz, None, None);
                }
            }
            if capped {
                editor.set_block_absolute(
                    STONE_BRICK_SLAB,
                    bx,
                    base + wall_height + 1,
                    bz,
                    None,
                    None,
                );
            }
        }
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
    let mut bases: Vec<Option<i32>> = cells.iter().map(|&c| Some(ground(c))).collect();
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
        // A barrier gets onto a deck only where the deck meets the ground; otherwise it
        // passes underneath.
        let at_grade = |inside: usize, outside: usize| {
            deck(cells[inside]).is_some_and(|d| d - ground(cells[outside]) <= 2)
        };
        let on_bridge = (start == 0 && i == cells.len())
            || (start > 0 && at_grade(start, start - 1))
            || (i < cells.len() && at_grade(i - 1, i));
        if !on_bridge {
            continue;
        }
        for (cell, base) in cells[start..i].iter().zip(&mut bases[start..i]) {
            *base = surface
                .supported_top(editor, cell.0, cell.1, BRIDGE_BARRIER_NEARBY_RADIUS)
                .or_else(|| {
                    // Beside the deck: only where the deck is still at grade.
                    let g = ground(*cell);
                    deck(*cell).is_some_and(|d| d - g <= 1).then_some(g)
                });
        }
    }
    bases
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
        Some("stile" | "gate" | "swing_gate" | "lift_gate") => {
            /*editor.set_block(
                OAK_TRAPDOOR,
                node.x,
                1,
                node.z,
                Some(&[
                    COBBLESTONE_WALL,
                    OAK_FENCE,
                    STONE_BRICK_WALL,
                    OAK_LEAVES,
                    STONE_BRICK_SLAB,
                ]),
                None,
            );
            editor.set_block(
                AIR,
                node.x,
                2,
                node.z,
                Some(&[
                    COBBLESTONE_WALL,
                    OAK_FENCE,
                    STONE_BRICK_WALL,
                    OAK_LEAVES,
                    STONE_BRICK_SLAB,
                ]),
                None,
            );
            editor.set_block(
                AIR,
                node.x,
                3,
                node.z,
                Some(&[
                    COBBLESTONE_WALL,
                    OAK_FENCE,
                    STONE_BRICK_WALL,
                    OAK_LEAVES,
                    STONE_BRICK_SLAB,
                ]),
                None,
            );*/
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
    use crate::osm_parser::ProcessedWay;

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
        let elements = vec![ProcessedElement::Way(bridge)];
        let outlines = BridgeOutlineIndex::build(&elements);
        let structures = BridgeStructureMap::build(&elements, &editor, &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&elements, &structures, 1.0);
        let member = structures.lookup_member(1).unwrap();
        // Stand in for the rendered deck: one row at the deck Y over the road stamp.
        for x in 10..=70 {
            for z in 38..=42 {
                editor.set_block_absolute(STONE, x, member.y_at((x - 10) as usize), z, None, None);
            }
        }
        let deck_y = member.y_at(30);
        assert!(deck_y >= 4, "the span is raised");

        let fence = |id, points: &[(i32, i32)]| {
            ProcessedElement::Way(way(id, &[("barrier", "wall")], points))
        };
        // Along the deck: on the deck.
        generate_barriers(&mut editor, &fence(2, &[(12, 40), (68, 40)]), &surface);
        assert!(editor.block_exists_absolute(40, deck_y + 1, 40));
        assert!(!editor.block_exists_absolute(40, 1, 40));
        // Passing underneath: on the ground.
        generate_barriers(&mut editor, &fence(3, &[(35, 10), (35, 70)]), &surface);
        assert!(editor.block_exists_absolute(35, 1, 40));
        // Off the deck edge mid-span: not hanging beside it.
        generate_barriers(&mut editor, &fence(4, &[(12, 44), (68, 44)]), &surface);
        assert!(!editor.block_exists_absolute(40, deck_y + 1, 44));
    }

    fn draw<R>(tags: &[(&str, &str)], read: impl FnOnce(&WorldEditor) -> R) -> R {
        let xzbbox = XZBBox::rect_from_xz_lengths(40.0, 40.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let outlines = BridgeOutlineIndex::build(&[]);
        let structures = BridgeStructureMap::build(&[], &editor, &outlines, 1.0);
        let surface = BridgeSurfaceMap::build(&[], &structures, 1.0);
        let way = ProcessedElement::Way(rect_way(1, 5, 5, 30, 30, tags));
        generate_barriers(&mut editor, &way, &surface);
        read(&editor)
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

    #[test]
    fn hedges_grow_from_soil_and_only_masonry_gets_a_coping() {
        assert!(capped(&[("barrier", "wall")]));
        assert!(!capped(&[("barrier", "hedge")]));
        assert!(!capped(&[("barrier", "fence"), ("fence_type", "wood")]));
        assert!(on_soil(&[("barrier", "hedge")]));
        assert!(!on_soil(&[("barrier", "wall")]));
    }
}
