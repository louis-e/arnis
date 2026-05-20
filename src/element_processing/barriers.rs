use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::osm_parser::{ProcessedElement, ProcessedNode};
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use std::collections::HashMap;

const BRIDGE_BARRIER_NEARBY_RADIUS: i32 = 2;

// Should probably be moved to a different file so it can be shared and reused.
#[derive(Clone, Copy)]
enum Axis {
    X,
    Z,
}

enum BarrierMaterial {
    Simple(BlockWithProperties),
    Axial { x: BlockWithProperties, z: BlockWithProperties },
}

#[allow(dead_code)]
impl BarrierMaterial {
    fn simple(block: Block) -> Self {
        Self::Simple(BlockWithProperties::new(block, None))
    }
    
    fn simple_with_properties(block: BlockWithProperties) -> Self {
        Self::Simple(block)
    }

    fn axial(x: Block, z: Block) -> Self {
        Self::Axial {
            x: BlockWithProperties::new(x, None),
            z: BlockWithProperties::new(z, None),
        }
    }

    fn axial_with_properties(x: BlockWithProperties, z: BlockWithProperties) -> Self {
        Self::Axial { x, z }
    }

    fn get(&self, axis: Axis) -> &BlockWithProperties {
        match self {
            BarrierMaterial::Simple(block) => block,
            BarrierMaterial::Axial { x, z } => match axis {
                Axis::X => x,
                Axis::Z => z,
            },
        }
    }
}

struct BarrierSetting {
    style: BarrierStyle,
    height: i32,
}

impl BarrierSetting {
    fn solid(block: Block, height: i32) -> Self {
        Self {
            style: BarrierStyle::Solid { material: BarrierMaterial::simple(block) },
            height,
        }
    }
}

enum BarrierStyle {
    Solid {
        material: BarrierMaterial,
    },
    Alternating {
        post_material: BarrierMaterial,
        link_material: BarrierMaterial,
        top_material: Option<BarrierMaterial>,
        spacing: usize,
        use_post_on_edge: bool,
    },
}

fn fence_axis_properties(axis: Axis) -> Value {
    let mut props = HashMap::new();
    match axis {
        Axis::X => {
            props.insert("east".to_string(), Value::String("true".to_string()));
            props.insert("west".to_string(), Value::String("true".to_string()));
        }
        Axis::Z => {
            props.insert("north".to_string(), Value::String("true".to_string()));
            props.insert("south".to_string(), Value::String("true".to_string()));
        }
    }
    Value::Compound(props)
}

fn get_setting_for_barrier(element: &ProcessedElement) -> Option<BarrierSetting> {
    let iron_bars_x: BlockWithProperties = BlockWithProperties::new(IRON_BARS, Some(fence_axis_properties(Axis::X)));
    let iron_bars_z: BlockWithProperties = BlockWithProperties::new(IRON_BARS, Some(fence_axis_properties(Axis::Z)));

    // Determine the base settinguration from tags.
    let setting = match element.tags().get("barrier").map(|s| s.as_str()) {
        Some("bollard") => Some(BarrierSetting::solid(COBBLESTONE_WALL, 1)),
        Some("kerb") => None, // Ignore kerbs.
        Some("hedge") => Some(BarrierSetting::solid(OAK_LEAVES, 2)),
        Some("wall") => Some(BarrierSetting::solid(STONE_BRICK_WALL, 3)),
        Some("fence") => {
            // Handle fence sub-types
            match element.tags().get("fence_type").map(|s| s.as_str()) {
                Some("railing" | "bars" | "krest") => Some(BarrierSetting::solid(STONE_BRICK_WALL, 1)),
                Some("chain_link" | "metal" | "wire" | "corrugated_metal" | "metal_bars") => Some(BarrierSetting::solid(STONE_BRICK_WALL, 2)),
                Some("slatted" | "paling") => Some(BarrierSetting::solid(OAK_FENCE, 1)),
                Some("wood" | "split_rail" | "panel" | "pole") => Some(BarrierSetting::solid(OAK_FENCE, 2)),
                Some("concrete" | "stone") => Some(BarrierSetting::solid(STONE_BRICK_WALL, 2)),
                Some("glass") => Some(BarrierSetting::solid(GLASS, 1)),
                Some("barbed_wire") => {
                    Some(BarrierSetting {
                        style: BarrierStyle::Alternating {
                            post_material: BarrierMaterial::simple(ANDESITE_WALL),
                            link_material: BarrierMaterial::axial_with_properties(iron_bars_x.clone(), iron_bars_z.clone()),
                            top_material: Some(BarrierMaterial::simple(COBWEB)),
                            spacing: 3,
                            use_post_on_edge: true,
                        },
                        height: 2
                    })
                }
                Some("electric") => {
                    Some(BarrierSetting {
                        style: BarrierStyle::Alternating {
                            post_material: BarrierMaterial::simple(OAK_FENCE),
                            link_material: BarrierMaterial::axial(CHAIN_X, CHAIN_Z),
                            top_material: None,
                            spacing: 2,
                            use_post_on_edge: true,
                        },
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
                "brick" => *material = BarrierMaterial::simple(BRICK),
                "concrete" => *material = BarrierMaterial::simple(LIGHT_GRAY_CONCRETE),
                "metal" => *material = BarrierMaterial::simple(STONE_BRICK_WALL),
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

pub fn generate_barriers(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    bridge_surface: &BridgeSurfaceMap,
) {
    let Some(setting) = get_setting_for_barrier(element) else {
        return; // Skip processing if no valid barrier type is found.
    };

    if let ProcessedElement::Way(way) = element {
        for i in 1..way.nodes.len() {
            let prev = &way.nodes[i - 1];
            let cur = &way.nodes[i];

            let bresenham_points = bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z);

            let axis = if prev.x.abs_diff(cur.x) > prev.z.abs_diff(cur.z) {
                Axis::X
            } else {
                Axis::Z
            };

            let mut counter = 0;
            for (bx, _, bz) in bresenham_points {
                let edge = (bx, bz) == (prev.x, prev.z) || (bx, bz) == (cur.x, cur.z);
                let deck_y = bridge_surface.nearby_deck_y(bx, bz, BRIDGE_BARRIER_NEARBY_RADIUS);
                place_barrier(&setting, editor, bx, bz, deck_y, counter, axis, edge);
                counter += 1;
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

fn place_barrier(
    setting: &BarrierSetting, 
    editor: &mut WorldEditor, 
    bx: i32, 
    bz: i32, 
    deck_y: Option<i32>,
    counter: usize,
    axis: Axis,
    edge: bool,
) {
    let place_block = |editor: &mut WorldEditor, block: BlockWithProperties, dy: i32| match deck_y {
        Some(y) => editor.set_block_with_properties_absolute(block, bx, y + dy, bz, None, None),
        None => editor.set_block_with_properties(block, bx, dy, bz, None, None),
    };

    match &setting.style {
        BarrierStyle::Solid { material } => {
            for y in 1..=setting.height {
                place_block(editor, material.get(axis).clone(), y);
            }
            if setting.height > 1 {
                place_block(editor, BlockWithProperties::new(STONE_BRICK_SLAB, None), setting.height + 1);
            }
        }
        BarrierStyle::Alternating { post_material, link_material, top_material, spacing, use_post_on_edge } => {
            let is_post = (edge && *use_post_on_edge) || counter % spacing == 0;
            let material =
                if is_post { post_material.get(axis).clone() }
                else { link_material.get(axis).clone() };

            for y in 1..=setting.height {
                place_block(editor, material.clone(), y);
            }

            if let Some(top_material) = top_material {
                place_block(editor, top_material.get(axis).clone(), setting.height + 1);
            }
        }
    }
}