//! Random ore veins for the stone produced by `--fillground`.

use crate::block_definitions::{
    Block, COAL_ORE, COPPER_ORE, DIAMOND_ORE, GOLD_ORE, IRON_ORE, LAPIS_ORE, REDSTONE_ORE, STONE,
};
use crate::coordinate_system::cartesian::XZBBox;
use crate::deterministic_rng::coord_rng;
use crate::progress::emit_gui_progress_update;
use crate::world_editor::WorldEditor;
use colored::Colorize;
use rand::Rng;

struct OreDef {
    block: Block,
    y_min: i32,
    y_max: i32,
    vein_min: u32,
    vein_max: u32,
    /// Sampled as uniform 0..=2*avg, giving the requested mean.
    avg_veins_per_chunk: u32,
}

const ORES: &[OreDef] = &[
    OreDef {
        block: COAL_ORE,
        y_min: -20,
        y_max: 50,
        vein_min: 8,
        vein_max: 17,
        avg_veins_per_chunk: 8,
    },
    OreDef {
        block: IRON_ORE,
        y_min: -50,
        y_max: 30,
        vein_min: 5,
        vein_max: 9,
        avg_veins_per_chunk: 6,
    },
    OreDef {
        block: COPPER_ORE,
        y_min: -16,
        y_max: 30,
        vein_min: 8,
        vein_max: 12,
        avg_veins_per_chunk: 4,
    },
    OreDef {
        block: LAPIS_ORE,
        y_min: -50,
        y_max: 10,
        vein_min: 4,
        vein_max: 7,
        avg_veins_per_chunk: 2,
    },
    OreDef {
        block: GOLD_ORE,
        y_min: -50,
        y_max: -10,
        vein_min: 5,
        vein_max: 9,
        avg_veins_per_chunk: 3,
    },
    OreDef {
        block: REDSTONE_ORE,
        y_min: -60,
        y_max: -10,
        vein_min: 5,
        vein_max: 10,
        avg_veins_per_chunk: 4,
    },
    OreDef {
        block: DIAMOND_ORE,
        y_min: -60,
        y_max: -30,
        vein_min: 4,
        vein_max: 7,
        avg_veins_per_chunk: 1,
    },
];

/// Place ore veins across every chunk in `xzbbox`. Only meaningful when
/// `--fillground` populated the underground; caller gates the call.
pub fn generate_ores(editor: &mut WorldEditor, xzbbox: &XZBBox) {
    println!("{} Sprinkling ore veins...", "[6b/7]".bold());
    emit_gui_progress_update(89.0, "Sprinkling ore veins...");

    let min_chunk_x = xzbbox.min_x() >> 4;
    let max_chunk_x = xzbbox.max_x() >> 4;
    let min_chunk_z = xzbbox.min_z() >> 4;
    let max_chunk_z = xzbbox.max_z() >> 4;

    for chunk_x in min_chunk_x..=max_chunk_x {
        for chunk_z in min_chunk_z..=max_chunk_z {
            // Salt 0xC0DE so this RNG can't collide with tree-variant or biome RNGs.
            let mut rng = coord_rng(chunk_x, chunk_z, 0xC0DE);

            for ore in ORES {
                let max_veins = ore.avg_veins_per_chunk * 2;
                let n = rng.random_range(0..=max_veins);
                for _ in 0..n {
                    let cx = (chunk_x << 4) + rng.random_range(0..16);
                    let cz = (chunk_z << 4) + rng.random_range(0..16);
                    let cy = rng.random_range(ore.y_min..=ore.y_max);
                    let size = rng.random_range(ore.vein_min..=ore.vein_max);
                    place_vein(editor, ore.block, cx, cy, cz, size, &mut rng);
                }
            }
        }
    }
}

// Whitelist on set_block_absolute is required to overwrite STONE; pre-check filters AIR.
fn place_vein(
    editor: &mut WorldEditor,
    block: Block,
    x: i32,
    y: i32,
    z: i32,
    size: u32,
    rng: &mut impl Rng,
) {
    let (mut cx, mut cy, mut cz) = (x, y, z);
    for _ in 0..size {
        if editor.check_for_block_absolute(cx, cy, cz, Some(&[STONE]), None) {
            editor.set_block_absolute(block, cx, cy, cz, Some(&[STONE]), None);
        }
        match rng.random_range(0..6) {
            0 => cx += 1,
            1 => cx -= 1,
            2 => cy += 1,
            3 => cy -= 1,
            4 => cz += 1,
            _ => cz -= 1,
        }
    }
}
