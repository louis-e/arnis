use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::osm_parser::{ProcessedElement, ProcessedNode};
use crate::world_editor::WorldEditor;

const BRIDGE_BARRIER_NEARBY_RADIUS: i32 = 2;

struct BarrierSetting {
    pub style: BarrierStyle,
    pub height: i32,
}

enum BarrierStyle {
    Solid {
        material: Block,
    },
    Alternating {
        post_xz: [Block; 2],
        chain_xz: [Block; 2],
        spacing: usize,
        use_post_on_edge: bool,
    },
}

fn get_setting_for_barrier(element: &ProcessedElement) -> Option<BarrierSetting> {
    // Determine the base settinguration from tags.
    let setting = match element.tags().get("barrier").map(|s| s.as_str()) {
        Some("bollard") => {
            Some(BarrierSetting {
                style: BarrierStyle::Solid { material: COBBLESTONE_WALL },
                height: 1
            })
        }
        Some("kerb") => None, // Ignore kerbs.
        Some("hedge") => {
            Some(BarrierSetting {
                style: BarrierStyle::Solid { material: OAK_LEAVES },
                height: 2
            })
        }
        Some("wall") => {
            Some(BarrierSetting {
                style: BarrierStyle::Solid { material: STONE_BRICK_WALL },
                height: 3
            })
        }
        Some("fence") => {
            match element.tags().get("fence_type").map(|s| s.as_str()) {
                Some("railing" | "bars" | "krest") => {
                    Some(BarrierSetting {
                        style: BarrierStyle::Solid { material: STONE_BRICK_WALL },
                        height: 1
                    })
                }
                Some("chain_link" | "metal" | "wire" | "barbed_wire" | "corrugated_metal" | "metal_bars") => {
                    Some(BarrierSetting {
                        style: BarrierStyle::Solid { material: STONE_BRICK_WALL },
                        height: 2
                    })
                }
                Some("electric") => {
                    Some(BarrierSetting {
                        style: BarrierStyle::Alternating {
                            post_xz: [OAK_FENCE, OAK_FENCE],
                            chain_xz: [CHAIN_X, CHAIN_Z],
                            spacing: 2,
                            use_post_on_edge: true,
                        },
                        height: 1
                    })
                }
                Some("slatted" | "paling") => {
                    Some(BarrierSetting {
                        style: BarrierStyle::Solid { material: OAK_FENCE },
                        height: 1
                    })
                }
                Some("wood" | "split_rail" | "panel" | "pole") => {
                    Some(BarrierSetting {
                        style: BarrierStyle::Solid { material: OAK_FENCE },
                        height: 2
                    })
                }
                Some("concrete" | "stone") => {
                    Some(BarrierSetting {
                        style: BarrierStyle::Solid { material: STONE_BRICK_WALL },
                        height: 2
                    })
                }
                Some("glass") => {
                    Some(BarrierSetting {
                        style: BarrierStyle::Solid { material: GLASS },
                        height: 1
                    })
                }
                _ => None
            }
        }
        _ => None
    };

    let Some(mut setting) = setting else {
        return None; // Skip processing if no valid barrier type is found.
    };

    // Apply tag overrides for the material.
    if let Some(barrier_mat) = element.tags().get("material") {
        if let BarrierStyle::Solid { ref mut material } = setting.style {
            match barrier_mat.as_str() {
                "brick" => *material = BRICK,
                "concrete" => *material = LIGHT_GRAY_CONCRETE,
                "metal" => *material = STONE_BRICK_WALL,
                _ => {}
            }
        }
    }
    
    // Apply tag overrides for the height.
    if let Some(h_str) = element.tags().get("height") {
        if let Ok(h_f32) = h_str.parse::<f32>() {
            // Apply custom height (with a floor of 1 if user manually overrode it).
            setting.height = (h_f32.round() as i32).max(1);
        }
    }

    Some(setting)
}

fn place_barrier(
    setting: &mut BarrierSetting, 
    editor: &mut WorldEditor, 
    bx: i32, 
    bz: i32, 
    deck_y: Option<i32>,
    counter: &mut usize,
    axis: usize,
    edge: bool,
) {
    let place_block = |editor: &mut WorldEditor, block: Block, dy: i32| match deck_y {
        Some(y) => editor.set_block_absolute(block, bx, y + dy, bz, None, None),
        None => editor.set_block(block, bx, dy, bz, None, None),
    };

    match setting.style {
        BarrierStyle::Solid { material } => {
            for y in 1..=setting.height {
                place_block(editor, material, y);
            }
            if setting.height > 1 {
                place_block(editor, STONE_BRICK_SLAB, setting.height + 1);
            }
        }
        BarrierStyle::Alternating { post_xz, chain_xz, spacing, use_post_on_edge } => {
            let material =
                if (edge && use_post_on_edge) || *counter >= spacing { post_xz[axis] }
                else { chain_xz[axis] };

            for y in 1..=setting.height {
                place_block(editor, material, y);
            }

            // Reset the counter if we've reached the spacing limit. This is to avoid using modulos.
            if *counter >= spacing {
                *counter = 0;
            }
        }
    }
}

pub fn generate_barriers(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    bridge_surface: &BridgeSurfaceMap,
) {
    let Some(mut setting) = get_setting_for_barrier(element) else {
        return; // Skip processing if no valid barrier type is found.
    };

    if let ProcessedElement::Way(way) = element {
        for i in 1..way.nodes.len() {
            let prev = &way.nodes[i - 1];
            let cur = &way.nodes[i];

            let bresenham_points = bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z);

            let mut counter = 0;
            let axis = if prev.x.abs_diff(cur.x) > prev.z.abs_diff(cur.z) {  0 } else { 1 };

            for (bx, _, bz) in bresenham_points {
                let edge = (bx, bz) == (prev.x, prev.z) || (bx, bz) == (cur.x, cur.z);

                let deck_y = bridge_surface.nearby_deck_y(bx, bz, BRIDGE_BARRIER_NEARBY_RADIUS);
                let mut local_counter = counter;

                place_barrier(&mut setting, editor, bx, bz, deck_y, &mut local_counter, axis, edge);

                // Increment counter only if it hasn't been modified in place_barrier.
                counter = if counter == local_counter { counter + 1 } else { 0 };
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
    let place_block = |editor: &mut WorldEditor<'_>, block: Block, dy: i32| match deck {
        Some(deck_y) => editor.set_block_absolute(block, node.x, deck_y + dy, node.z, None, None),
        None => editor.set_block(block, node.x, dy, node.z, None, None),
    };
    
    match node.tags.get("barrier").map(|s| s.as_str()) {
        Some("bollard") => place_block(editor, COBBLESTONE_WALL, 1),
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
        Some("block") => place_block(editor, STONE, 1),
        Some("entrance") => place_block(editor, AIR, 1),
        _ => {}
    }
}