use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::bridges::BridgeSurfaceMap;
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

        // Process nodes to create the barrier wall
        for i in 1..way.nodes.len() {
            let prev: &crate::osm_parser::ProcessedNode = &way.nodes[i - 1];
            let x1: i32 = prev.x;
            let z1: i32 = prev.z;

            let cur: &crate::osm_parser::ProcessedNode = &way.nodes[i];
            let x2: i32 = cur.x;
            let z2: i32 = cur.z;

            // Generate the line of coordinates between the two nodes
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(x1, 0, z1, x2, 0, z2);

            for (bx, _, bz) in bresenham_points {
                let deck = bridge_surface.nearby_deck_y(bx, bz, BRIDGE_BARRIER_NEARBY_RADIUS);
                match deck {
                    Some(deck_y) => {
                        for y in 1..=wall_height {
                            editor.set_block_absolute(
                                barrier_material,
                                bx,
                                deck_y + y,
                                bz,
                                None,
                                None,
                            );
                        }
                        if wall_height > 1 {
                            editor.set_block_absolute(
                                STONE_BRICK_SLAB,
                                bx,
                                deck_y + wall_height + 1,
                                bz,
                                None,
                                None,
                            );
                        }
                    }
                    None => {
                        for y in 1..=wall_height {
                            editor.set_block(barrier_material, bx, y, bz, None, None);
                        }
                        if wall_height > 1 {
                            editor.set_block(STONE_BRICK_SLAB, bx, wall_height + 1, bz, None, None);
                        }
                    }
                }
            }
        }
    }
}

pub fn generate_barrier_nodes(
    editor: &mut WorldEditor<'_>,
    node: &ProcessedNode,
    bridge_surface: &BridgeSurfaceMap,
) {
    let deck = bridge_surface.nearby_deck_y(node.x, node.z, BRIDGE_BARRIER_NEARBY_RADIUS);
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
