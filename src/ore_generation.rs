//! Random ore veins for the stone produced by `--fillground`.

use crate::block_definitions::{
    Block, COAL_ORE, DIAMOND_ORE, GOLD_ORE, IRON_ORE, LAPIS_ORE, REDSTONE_ORE, STONE,
};
use crate::coordinate_system::cartesian::XZBBox;
use crate::deterministic_rng::coord_rng;
use crate::progress::emit_gui_progress_update;
use crate::world_editor::{terrain_floor_y, WorldEditor};
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

/// Deepest ore-bearing column below local ground: the deepest band ends at 65, the rest is the
/// vanilla column (383 blocks at most), so vanilla is unchanged and a sunk floor is not tracked.
const MAX_ORE_DEPTH: i32 = 384;

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
                // Vein count scales with `span`, so a floor that sinks with the base would
                // multiply it; the band is capped instead of following the floor down.
                let y_min = (ground_y - MAX_ORE_DEPTH).max(terrain_floor_y() + 1);
                let y_max = (ground_y - ore.depth_min).max(y_min);
                if y_min > y_max {
                    continue;
                }
                let span = (y_max - y_min + 1) as u32;
                let orig_span = (ore.depth_max - ore.depth_min + 1) as u32;
                let max_veins = ore
                    .avg_veins_per_chunk
                    .saturating_mul(span)
                    .saturating_mul(2)
                    / orig_span;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::world_editor::{
        set_terrain_floor_y, set_world_bounds, DEFAULT_MAX_Y, DEFAULT_MIN_Y, FLOOR_TEST_LOCK,
    };
    use std::ops::RangeInclusive;
    use std::path::PathBuf;

    /// Ore blocks placed inside `band` for one chunk with its surface at `ground_y`. Only
    /// `band` is stone, and veins overwrite nothing else, so anything else there is ore.
    fn ores_in_band(ground_y: i32, band: RangeInclusive<i32>) -> usize {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 15, 15).unwrap();
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor = WorldEditor::new(PathBuf::from("/dev/null/unused"), &xzbbox, llbbox);
        editor.register_road_surface_y(8, 8, ground_y);
        for y in band.clone() {
            for x in 0..16 {
                for z in 0..16 {
                    editor.set_block_absolute(STONE, x, y, z, None, None);
                }
            }
        }

        generate_ores_region(&mut editor, 0, 15, 0, 15, false);

        band.flat_map(|y| (0..16).flat_map(move |x| (0..16).map(move |z| (x, y, z))))
            .filter(
                |&(x, y, z)| matches!(editor.get_block_absolute(x, y, z), Some(b) if b != STONE),
            )
            .count()
    }

    #[test]
    fn the_tallest_vanilla_column_still_mines_down_to_the_bedrock_plane() {
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
        set_terrain_floor_y(DEFAULT_MIN_Y + 2);
        assert_eq!(terrain_floor_y(), DEFAULT_MIN_Y);

        let floor = terrain_floor_y() + 1;
        let n = ores_in_band(DEFAULT_MAX_Y, floor..=floor + 39);

        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
        set_terrain_floor_y(DEFAULT_MIN_Y + 2);
        assert!(n > 0, "the cap must not clip the 383 block vanilla column");
    }

    #[test]
    fn a_sunk_floor_does_not_extend_the_ore_column_below_the_cap() {
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_world_bounds(-2032, 2031);
        set_terrain_floor_y(-1888);
        assert_eq!(terrain_floor_y(), -1952);

        let ground_y = 2000;
        let below = ores_in_band(ground_y, ground_y - 500..=ground_y - MAX_ORE_DEPTH - 1);
        let within = ores_in_band(ground_y, ground_y - MAX_ORE_DEPTH..=ground_y - 300);

        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
        set_terrain_floor_y(DEFAULT_MIN_Y + 2);
        assert_eq!(below, 0, "ore reached below the cap");
        assert!(within > 0, "the capped band carries no ore at all");
    }

    #[test]
    fn a_column_shallower_than_the_cap_still_reaches_the_terrain_floor() {
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_world_bounds(-2032, 2031);
        set_terrain_floor_y(-1888);

        let floor = terrain_floor_y() + 1;
        let n = ores_in_band(-1888, floor..=floor + 50);

        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
        set_terrain_floor_y(DEFAULT_MIN_Y + 2);
        assert!(n > 0, "a column within the cap must still reach bedrock");
    }
}
