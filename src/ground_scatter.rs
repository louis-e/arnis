//! Small rocks and bushes from bundled schematics, scattered over open land.
//!
//! Each 16x16 chunk rolls once for a rock and once for a bush; a rock wins when
//! both land. The piece, its rotation and its spot are drawn from the chunk's
//! coordinates, and the spot is chosen so the rotated piece stays inside its
//! chunk. Tiles are region-aligned, so a piece always lies within one pass's
//! bounds and comes out the same whichever tile or One World piece builds it.
//!
//! Pieces only go on natural dry ground under ESA cropland or grassland, never
//! on sealed (paved or built) ground or over anything but loose plants and
//! crops. Rocks also keep off tilled farmland.

use std::sync::OnceLock;

use crate::args::Args;
use crate::block_definitions::{
    Block, AIR, CARROTS, COARSE_DIRT, DIRT, FARMLAND, GRASS_BLOCK, MOSS_BLOCK, PODZOL, POTATOES,
    WHEAT,
};
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::ground_decoration::{LOOSE_PLANTS, PLANT_LOWER_HALVES, STACKED_PLANT_PARTS};
use crate::land_cover::{LC_CROPLAND, LC_GRASSLAND};
use crate::structures::schematic::{load_structure, place_structure, StructureSchematic};
use crate::trees::schematic::rotate_xz;
use crate::world_editor::WorldEditor;
use rand::Rng;

const SALT: u64 = 0x5CA7_7E12_B0C4_0000;

/// Ground a bush may stand on.
const SOIL: &[Block] = &[GRASS_BLOCK, COARSE_DIRT, DIRT, FARMLAND, MOSS_BLOCK, PODZOL];
/// Ground a rock may stand on: farmers clear them off tilled fields.
const ROCK_SOIL: &[Block] = &[GRASS_BLOCK, COARSE_DIRT, DIRT, MOSS_BLOCK, PODZOL];
/// Crops a piece may stand in, besides loose plants.
const CROPS: &[Block] = &[WHEAT, CARROTS, POTATOES];
/// Most the ground may rise across a piece's base before it is left out.
const MAX_BASE_STEP: i32 = 2;

macro_rules! bushes {
    ($($species:literal),*) => {
        [$(
            include_bytes!(concat!("../assets/structures/", $species, "bush1.schem")).as_slice(),
            include_bytes!(concat!("../assets/structures/", $species, "bush2.schem")).as_slice(),
            include_bytes!(concat!("../assets/structures/", $species, "bush3.schem")).as_slice(),
            include_bytes!(concat!("../assets/structures/", $species, "bush4.schem")).as_slice(),
            include_bytes!(concat!("../assets/structures/", $species, "bush5.schem")).as_slice(),
            include_bytes!(concat!("../assets/structures/", $species, "bush6.schem")).as_slice(),
        )*]
    };
}

static ROCK_BYTES: [&[u8]; 8] = [
    include_bytes!("../assets/structures/rock1.schem"),
    include_bytes!("../assets/structures/rock2.schem"),
    include_bytes!("../assets/structures/rock3.schem"),
    include_bytes!("../assets/structures/rock4.schem"),
    include_bytes!("../assets/structures/rock5.schem"),
    include_bytes!("../assets/structures/rock6.schem"),
    include_bytes!("../assets/structures/rock7.schem"),
    include_bytes!("../assets/structures/rock8.schem"),
];

// Ten species of six shapes; each has a short bark pole inside the leaves' decay
// distance, so the leaves survive in game.
static BUSH_BYTES: [&[u8]; 60] = bushes!(
    "acacia",
    "azalea",
    "birch",
    "cherry",
    "darkoak",
    "flowering_azalea",
    "jungle",
    "mangrove",
    "oak",
    "spruce"
);

fn parse_pool(bytes: &[&[u8]]) -> Vec<StructureSchematic> {
    bytes
        .iter()
        .map(|b| {
            load_structure(b)
                .expect("bundled scatter schematic")
                .base_anchored()
        })
        .collect()
}

fn rocks() -> &'static [StructureSchematic] {
    static POOL: OnceLock<Vec<StructureSchematic>> = OnceLock::new();
    POOL.get_or_init(|| parse_pool(&ROCK_BYTES))
}

fn bushes() -> &'static [StructureSchematic] {
    static POOL: OnceLock<Vec<StructureSchematic>> = OnceLock::new();
    POOL.get_or_init(|| parse_pool(&BUSH_BYTES))
}

/// Voxel offsets from the anchor once the piece is turned `rot` quarter-turns,
/// as `place_structure` lays them out.
fn rotated_offsets(schem: &StructureSchematic, rot: u8) -> Vec<(i32, i32, i32)> {
    let (w, l) = (schem.width, schem.length);
    let (ax, az) = rotate_xz(schem.anchor_x, schem.anchor_z, w, l, rot);
    schem
        .voxels
        .iter()
        .map(|&(vx, vy, vz, _)| {
            let (rx, rz) = rotate_xz(vx, vz, w, l, rot);
            (rx - ax, vy, rz - az)
        })
        .collect()
}

/// Anchor positions within a chunk (0..16) that keep every offset inside it.
fn anchor_span(offsets: &[(i32, i32, i32)], pick: impl Fn(&(i32, i32, i32)) -> i32) -> (i32, i32) {
    let lo = offsets.iter().map(&pick).min().unwrap_or(0);
    let hi = offsets.iter().map(&pick).max().unwrap_or(0);
    (-lo, 15 - hi)
}

/// Rocks 8 blocks or more across.
fn is_large(schem: &StructureSchematic) -> bool {
    schem.width.max(schem.length) >= 8
}

/// What one chunk holds: the piece, its rotation and its anchor in world blocks.
fn chunk_piece(
    chunk_x: i32,
    chunk_z: i32,
    rock_density: f64,
    bush_density: f64,
) -> Option<(&'static StructureSchematic, bool, u8, i32, i32)> {
    let mut rng = crate::deterministic_rng::coord_rng(chunk_x, chunk_z, SALT);
    // Both rolls are drawn either way, so one kind's flag never moves the other.
    let rock_hit = rng.random::<f64>() < rock_density;
    let bush_hit = rng.random::<f64>() < bush_density;
    let pool = match (rock_hit, bush_hit) {
        (true, _) => rocks(),
        (false, true) => bushes(),
        (false, false) => return None,
    };
    let schem = if rock_hit {
        // Big rocks weigh 1 against 6 for small ones: about one rock in eleven.
        let weight = |s: &StructureSchematic| if is_large(s) { 1 } else { 6 };
        let mut roll = rng.random_range(0..pool.iter().map(weight).sum::<u32>());
        pool.iter()
            .find(|s| {
                let w = weight(s);
                let hit = roll < w;
                roll = roll.saturating_sub(w);
                hit
            })
            .unwrap_or(&pool[0])
    } else {
        &pool[rng.random_range(0..pool.len())]
    };
    let rot = rng.random_range(0..4u8);
    let offsets = rotated_offsets(schem, rot);
    let (lo_x, hi_x) = anchor_span(&offsets, |o| o.0);
    let (lo_z, hi_z) = anchor_span(&offsets, |o| o.2);
    if lo_x > hi_x || lo_z > hi_z {
        return None;
    }
    let x = (chunk_x << 4) + rng.random_range(lo_x..=hi_x);
    let z = (chunk_z << 4) + rng.random_range(lo_z..=hi_z);
    Some((schem, rock_hit, rot, x, z))
}

/// Scatters rocks and bushes over `[min_x..=max_x] x [min_z..=max_z]`, once the
/// ground there is finished. Needs land cover; does nothing unless asked for.
#[allow(clippy::too_many_arguments)]
pub fn scatter_region(
    editor: &mut WorldEditor,
    ground: &Ground,
    args: &Args,
    xzbbox: &XZBBox,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) {
    let opts = &args.scatter;
    let rock_density = if opts.rocks { opts.rock_density } else { 0.0 };
    let bush_density = if opts.bushes { opts.bush_density } else { 0.0 };
    if (rock_density <= 0.0 && bush_density <= 0.0)
        || !ground.has_land_cover()
        || !ground.body().is_earth()
        || min_x > max_x
        || min_z > max_z
    {
        return;
    }
    let inside = |x: i32, z: i32| {
        (min_x..=max_x).contains(&x)
            && (min_z..=max_z).contains(&z)
            && ground.is_in_rotated_bounds(x, z)
    };
    let ground_y = |editor: &WorldEditor, x: i32, z: i32| {
        if ground.elevation_enabled {
            editor.get_ground_level(x, z)
        } else {
            args.ground_level
        }
    };

    for chunk_x in (min_x >> 4)..=(max_x >> 4) {
        for chunk_z in (min_z >> 4)..=(max_z >> 4) {
            let Some((schem, is_rock, rot, ax, az)) =
                chunk_piece(chunk_x, chunk_z, rock_density, bush_density)
            else {
                continue;
            };
            let cover = ground.cover_class(XZPoint::new(ax - xzbbox.min_x(), az - xzbbox.min_z()));
            if cover != LC_CROPLAND && cover != LC_GRASSLAND {
                continue;
            }
            let offsets = rotated_offsets(schem, rot);
            if !offsets.iter().all(|&(dx, _, dz)| inside(ax + dx, az + dz)) {
                continue;
            }

            // The base sits on the lowest ground it covers, so nothing floats.
            let soil = if is_rock { ROCK_SOIL } else { SOIL };
            let (mut low, mut high, mut ok) = (i32::MAX, i32::MIN, true);
            for &(dx, dy, dz) in &offsets {
                let (x, z) = (ax + dx, az + dz);
                if dy != 0 {
                    continue;
                }
                let y = ground_y(editor, x, z);
                if editor.surface_is_sealed(x, z)
                    || !editor.check_for_block_absolute(x, y, z, Some(soil), None)
                {
                    ok = false;
                    break;
                }
                low = low.min(y);
                high = high.max(y);
            }
            if !ok || low == i32::MAX || high - low > MAX_BASE_STEP {
                continue;
            }
            let base_y = low + 1;
            let occupied = offsets.iter().any(|&(dx, dy, dz)| {
                let (x, y, z) = (ax + dx, base_y + dy, az + dz);
                y > ground_y(editor, x, z)
                    && editor.block_exists_absolute(x, y, z)
                    && !editor.check_for_block_absolute(x, y, z, Some(LOOSE_PLANTS), None)
                    && !editor.check_for_block_absolute(x, y, z, Some(CROPS), None)
            });
            if occupied {
                continue;
            }

            place_structure(editor, schem, ax, az, base_y, rot, None);
            // Two-block plants cut in half by the piece go entirely.
            for &(dx, dy, dz) in &offsets {
                let (x, y, z) = (ax + dx, base_y + dy, az + dz);
                editor.set_block_absolute(AIR, x, y + 1, z, Some(STACKED_PLANT_PARTS), None);
                editor.set_block_absolute(AIR, x, y - 1, z, Some(PLANT_LOWER_HALVES), None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_palette_entry_maps_to_a_block() {
        use crate::structures::schematic::load_palettized;
        use fastnbt::Value;
        use std::io::Read;
        for bytes in ROCK_BYTES.iter().chain(BUSH_BYTES.iter()) {
            let mut raw = Vec::new();
            flate2::read::GzDecoder::new(*bytes)
                .read_to_end(&mut raw)
                .unwrap();
            let root: Value = fastnbt::from_bytes(&raw).unwrap();
            let palette = |v: &Value| -> Option<usize> {
                let Value::Compound(c) = v else { return None };
                let scm = match c.get("Schematic") {
                    Some(Value::Compound(s)) => s,
                    _ => c,
                };
                let pal = match scm.get("Blocks") {
                    Some(Value::Compound(b)) => b.get("Palette"),
                    _ => scm.get("Palette"),
                };
                let Some(Value::Compound(p)) = pal else {
                    return None;
                };
                Some(p.keys().filter(|k| !k.ends_with(":air")).count())
            };
            // Unmapped names are dropped silently, so count them against the source.
            let solid = palette(&root).unwrap();
            assert_eq!(load_palettized(bytes).unwrap().palette.len(), solid);
            assert!(!load_structure(bytes).unwrap().voxels.is_empty());
        }
        assert_eq!(rocks().len(), 8);
        assert_eq!(bushes().len(), 60);
    }

    #[test]
    fn every_piece_fits_its_chunk_in_every_rotation() {
        for schem in rocks().iter().chain(bushes()) {
            for rot in 0..4 {
                let offsets = rotated_offsets(schem, rot);
                let (lo_x, hi_x) = anchor_span(&offsets, |o| o.0);
                let (lo_z, hi_z) = anchor_span(&offsets, |o| o.2);
                assert!(lo_x <= hi_x && lo_z <= hi_z, "piece wider than a chunk");
            }
        }
        for cx in -40..40 {
            for cz in -40..40 {
                if let Some((schem, _, rot, x, z)) = chunk_piece(cx, cz, 0.5, 0.5) {
                    for (dx, _, dz) in rotated_offsets(schem, rot) {
                        assert_eq!(((x + dx) >> 4, (z + dz) >> 4), (cx, cz));
                    }
                }
            }
        }
    }

    #[test]
    fn densities_set_the_share_of_chunks() {
        let count = |rock: f64, bush: f64| {
            let mut rocks = 0;
            let mut bushes = 0;
            for cx in 0..100 {
                for cz in 0..100 {
                    match chunk_piece(cx, cz, rock, bush) {
                        Some((_, true, ..)) => rocks += 1,
                        Some((_, false, ..)) => bushes += 1,
                        None => {}
                    }
                }
            }
            (rocks, bushes)
        };
        assert_eq!(count(0.0, 0.0), (0, 0));
        let (rocks, bushes) = count(0.02, 0.05);
        assert!((150..=250).contains(&rocks), "{rocks} rocks in 10k chunks");
        assert!(
            (400..=600).contains(&bushes),
            "{bushes} bushes in 10k chunks"
        );
        // A bush chunk stays a bush chunk whether or not rocks are on.
        for cx in 0..30 {
            for cz in 0..30 {
                if let Some((_, false, rot, x, z)) = chunk_piece(cx, cz, 0.02, 0.05) {
                    let alone = chunk_piece(cx, cz, 0.0, 0.05).map(|p| (p.2, p.3, p.4));
                    assert_eq!(alone, Some((rot, x, z)));
                }
            }
        }
    }

    #[test]
    fn scatter_is_piece_invariant() {
        use crate::coordinate_system::geographic::LLBBox;
        use crate::ground::test_support::ground_with_land_cover_and_elevation;
        use crate::land_cover::LandCoverData;
        use clap::Parser;
        use std::sync::Arc;

        let side = 128usize;
        let grid: Vec<Vec<u8>> = (0..side)
            .map(|z| {
                (0..side)
                    .map(|x| {
                        if (x + z) % 50 < 40 {
                            LC_GRASSLAND
                        } else {
                            LC_CROPLAND
                        }
                    })
                    .collect()
            })
            .collect();
        let lc = LandCoverData {
            grid,
            water_distance: vec![vec![0u8; side]; side],
            water_blend_cache: once_cell::sync::OnceCell::new(),
            beach_tiles: once_cell::sync::OnceCell::new(),
            inland_water: Default::default(),
            width: side,
            height: side,
            cells_per_meter: 1.0,
        };
        let ground = Arc::new(ground_with_land_cover_and_elevation(lc, side, side));
        let max = side as i32 - 1;
        let xzbbox = XZBBox::rect_from_min_max(0, 0, max, max).unwrap();
        let llbbox = LLBBox::new(48.0, 11.0, 48.01, 11.01).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().to_str().unwrap();
        let args = Args::parse_from([
            "arnis",
            "--output-dir",
            out,
            "--bbox",
            "48.0,11.0,48.01,11.01",
            "--rocks",
            "--bushes",
            "--rock-density",
            "0.3",
            "--bush-density",
            "0.5",
        ]);

        let run = |tiles: &[(i32, i32)]| -> Vec<Option<Block>> {
            let mut editor = WorldEditor::new(tmp.path().to_path_buf(), &xzbbox, llbbox);
            editor.set_ground(ground.clone());
            for x in 0..=max {
                for z in 0..=max {
                    let y = if ground.elevation_enabled {
                        editor.get_ground_level(x, z)
                    } else {
                        args.ground_level
                    };
                    let soil = if x < 64 { GRASS_BLOCK } else { FARMLAND };
                    editor.set_block_absolute(soil, x, y, z, None, None);
                }
            }
            for &(min_x, max_x) in tiles {
                scatter_region(&mut editor, &ground, &args, &xzbbox, min_x, max_x, 0, max);
            }
            (0..=max)
                .flat_map(|x| (0..=max).flat_map(move |z| (-70..=20).map(move |y| (x, y, z))))
                .map(|(x, y, z)| editor.get_block_absolute(x, y, z))
                .collect()
        };

        let whole = run(&[(0, max)]);
        let split = run(&[(0, 63), (64, max)]);
        let blocks = |v: &[Option<Block>], b: Block| v.iter().filter(|&&x| x == Some(b)).count();
        let leaves = whole
            .iter()
            .filter(|b| b.is_some_and(|b| b.name().ends_with("leaves")))
            .count();
        assert!(leaves > 50, "bushes should have been placed");
        assert!(
            blocks(&whole, crate::block_definitions::ANDESITE)
                + blocks(&whole, crate::block_definitions::TUFF)
                > 0,
            "rocks should have been placed"
        );
        assert!(whole == split, "a tile seam changed the scatter");
    }

    #[test]
    fn large_rocks_are_rare() {
        let (mut rocks, mut large) = (0, 0);
        for cx in 0..100 {
            for cz in 0..100 {
                if let Some((schem, true, ..)) = chunk_piece(cx, cz, 1.0, 0.0) {
                    rocks += 1;
                    large += is_large(schem) as u32;
                }
            }
        }
        assert_eq!(rocks, 10_000);
        assert!(
            (500..=1500).contains(&large),
            "{large} large of {rocks} rock picks"
        );
    }

    #[test]
    fn rock_wins_when_both_roll() {
        // Both densities at 1.0: every chunk rolls both, and every piece is a rock.
        for cx in 0..20 {
            for cz in 0..20 {
                if let Some((_, is_rock, ..)) = chunk_piece(cx, cz, 1.0, 1.0) {
                    assert!(is_rock);
                }
            }
        }
    }

    #[test]
    fn density_zero_places_nothing() {
        for cx in -20..20 {
            for cz in -20..20 {
                assert!(chunk_piece(cx, cz, 0.0, 0.0).is_none());
            }
        }
    }

    #[test]
    fn nothing_is_placed_unless_asked_for() {
        use clap::Parser;
        let args = Args::parse_from(["arnis", "--bbox", "48.0,11.0,48.01,11.01"]);
        assert!(!args.scatter.rocks && !args.scatter.bushes);
        assert_eq!(args.scatter.rock_density, 0.02);
        assert_eq!(args.scatter.bush_density, 0.05);
    }
}
