//! Random ore veins for the stone produced by `--fillground`.

use crate::block_definitions::{
    Block, COAL_ORE, DIAMOND_ORE, GOLD_ORE, IRON_ORE, LAPIS_ORE, REDSTONE_ORE, STONE,
};
use crate::coordinate_system::cartesian::XZBBox;
use crate::deterministic_rng::coord_rng;
use crate::progress::emit_gui_progress_update;
use crate::world_editor::{WorldEditor, MIN_Y};
use colored::Colorize;
use rand::Rng;

struct OreDef {
    block: Block,
    /// Shallowest depth below local ground level (e.g. 3 = 3 blocks under surface).
    depth_min: i32,
    /// Deepest depth below local ground level.
    depth_max: i32,
    vein_min: u32,
    vein_max: u32,
    /// Sampled as uniform 0..=2*avg, giving the requested mean.
    avg_veins_per_chunk: u32,
}

const ORES: &[OreDef] = &[
    OreDef {
        block: COAL_ORE,
        depth_min: 3,
        depth_max: 45,
        vein_min: 8,
        vein_max: 17,
        avg_veins_per_chunk: 8,
    },
    OreDef {
        block: IRON_ORE,
        depth_min: 3,
        depth_max: 60,
        vein_min: 5,
        vein_max: 9,
        avg_veins_per_chunk: 6,
    },
    OreDef {
        block: LAPIS_ORE,
        depth_min: 25,
        depth_max: 55,
        vein_min: 4,
        vein_max: 7,
        avg_veins_per_chunk: 2,
    },
    OreDef {
        block: GOLD_ORE,
        depth_min: 40,
        depth_max: 60,
        vein_min: 5,
        vein_max: 9,
        avg_veins_per_chunk: 3,
    },
    OreDef {
        block: REDSTONE_ORE,
        depth_min: 45,
        depth_max: 65,
        vein_min: 5,
        vein_max: 10,
        avg_veins_per_chunk: 4,
    },
    OreDef {
        block: DIAMOND_ORE,
        depth_min: 50,
        depth_max: 65,
        vein_min: 4,
        vein_max: 7,
        avg_veins_per_chunk: 1,
    },
];

/// Place ore veins across every chunk; Y is relative to local ground.
pub fn generate_ores(editor: &mut WorldEditor, xzbbox: &XZBBox) {
    generate_ores_region(
        editor,
        xzbbox.min_x(),
        xzbbox.max_x(),
        xzbbox.min_z(),
        xzbbox.max_z(),
        true,
    );
}

/// Place ore veins across the chunks covering `[iter_min..=iter_max]` (per-tile callers
/// pass strict tile bounds). Chunk-coord-seeded RNG; veins truncate at tile seams.
pub fn generate_ores_region(
    editor: &mut WorldEditor,
    iter_min_x: i32,
    iter_max_x: i32,
    iter_min_z: i32,
    iter_max_z: i32,
    show_progress: bool,
) {
    if show_progress {
        println!("{} Sprinkling ore veins...", "[6b/7]".bold());
        emit_gui_progress_update(89.0, "Sprinkling ore veins...");
    }

    let min_chunk_x = iter_min_x >> 4;
    let max_chunk_x = iter_max_x >> 4;
    let min_chunk_z = iter_min_z >> 4;
    let max_chunk_z = iter_max_z >> 4;

    for chunk_x in min_chunk_x..=max_chunk_x {
        for chunk_z in min_chunk_z..=max_chunk_z {
            let ground_y = editor.get_ground_level((chunk_x << 4) + 8, (chunk_z << 4) + 8);
            let mut rng = coord_rng(chunk_x, chunk_z, 0xC0DE);

            for ore in ORES {
                let y_min = (ground_y - ore.depth_max).max(MIN_Y + 1);
                let y_max = (ground_y - ore.depth_min).max(MIN_Y + 1);
                if y_min > y_max {
                    continue;
                }
                let max_veins = ore.avg_veins_per_chunk * 2;
                let n = rng.random_range(0..=max_veins);
                for _ in 0..n {
                    let cx = (chunk_x << 4) + rng.random_range(0..16);
                    let cz = (chunk_z << 4) + rng.random_range(0..16);
                    let cy = rng.random_range(y_min..=y_max);
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
