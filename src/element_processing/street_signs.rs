use crate::block_definitions::*;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

pub fn generate_street_signs(editor: &mut WorldEditor, way: &ProcessedWay, ground_level: i32) {
    // Check if the way has a name tag
    if let Some(street_name) = way.tags.get("name") {
        // Iterate over the nodes in the way
        for node in &way.nodes {
            let x: i32 = node.x;
            let z: i32 = node.z;

            // Place a sign at the intersection
            editor.set_block(OAK_SIGN, x, ground_level + 1, z, None, None);

            // Embed the street name using blocks
            let mut offset = 0;
            for ch in street_name.chars() {
                let block = match ch {
                    'A' => OAK_PLANKS,
                    'B' => BIRCH_PLANKS,
                    'C' => SPRUCE_PLANKS,
                    'D' => JUNGLE_PLANKS,
                    'E' => ACACIA_PLANKS,
                    'F' => DARK_OAK_PLANKS,
                    'G' => CRIMSON_PLANKS,
                    'H' => WARPED_PLANKS,
                    'I' => STONE,
                    'J' => COBBLESTONE,
                    'K' => SANDSTONE,
                    'L' => RED_SANDSTONE,
                    'M' => NETHER_BRICKS,
                    'N' => END_STONE_BRICKS,
                    'O' => PRISMARINE,
                    'P' => QUARTZ_BLOCK,
                    'Q' => PURPUR_BLOCK,
                    'R' => MAGMA_BLOCK,
                    'S' => SEA_LANTERN,
                    'T' => GLOWSTONE,
                    'U' => HONEY_BLOCK,
                    'V' => SLIME_BLOCK,
                    'W' => HAY_BLOCK,
                    'X' => BONE_BLOCK,
                    'Y' => COAL_BLOCK,
                    'Z' => IRON_BLOCK,
                    ' ' => AIR,
                    _ => OAK_PLANKS,
                };

                // Place the block to form the letter
                editor.set_block(block, x + offset, ground_level + 2, z, None, None);
                offset += 1;
            }
        }
    }
}
