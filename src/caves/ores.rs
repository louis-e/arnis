//! Clean-room port of vanilla 1.21.8 overworld blob-ore generation, placed into the carved solid
//! underground (so `discard_chance_on_air_exposure` works against real cave walls).
//! Vanilla-LOOK, terrain-decoupled: per-chunk deterministic RNG, vanilla ore table
//! (size / count / Y-distribution / discard), deepslate variant wherever the host rock is
//! deepslate. Absolute bands are vanilla's, translated onto this world's floor.

use super::rng::XoroRandom;
use crate::block_definitions::*;
use crate::world_editor::WorldEditor;
use std::f64::consts::PI;

/// Rock the ore pass may replace (same as the cave host: stone + its variants).
const REPLACEABLE: &[Block] = &[
    STONE,
    DEEPSLATE,
    TUFF,
    COBBLED_DEEPSLATE,
    GRANITE,
    DIORITE,
    ANDESITE,
    GRAVEL,
    DIRT,
];

#[derive(Clone, Copy)]
enum Count {
    N(i32),
    Rarity(i32), // 1-in-C chunks
}
#[derive(Clone, Copy)]
enum Dist {
    Uniform,
    Trapezoid,
}

struct Ore {
    stone: Block,
    deep: Block,
    size: i32,
    count: Count,
    // when true, ymin/ymax are read as DEPTH BELOW the local surface (so the blob tracks
    // the terrain instead of a fixed absolute Y). Real-world surfaces vary a lot and, at
    // small scale, sit far below vanilla's ~y64 — an absolute 0..60 range lands mostly
    // above ground in a valley and gets pruned to nothing. Stone variants use this so
    // diorite/granite/andesite/dirt fill the rock under every column like vanilla.
    rel: bool,
    dist: Dist,
    ymin: i32,
    ymax: i32,
    discard: f32,
}

// Vanilla overworld ores (UNDERGROUND_ORES order; later entries may overwrite earlier rock variants).
fn ore_table() -> Vec<Ore> {
    use Count::*;
    use Dist::*;
    // absolute-Y ore (metal/gem + gravel): ymin/ymax are vanilla Y.
    let o = |stone, deep, size, count, dist, ymin, ymax, discard| Ore {
        stone,
        deep,
        size,
        count,
        dist,
        ymin,
        ymax,
        discard,
        rel: false,
    };
    // surface-relative variant (stone families): ymin/ymax are DEPTH below the local surface,
    // so the blob always lands in the rock under the column regardless of terrain height.
    let rel = |block, size, count, dist, dmin, dmax| Ore {
        stone: block,
        deep: block,
        size,
        count,
        dist,
        ymin: dmin,
        ymax: dmax,
        discard: 0.0,
        rel: true,
    };
    vec![
        // Stone variation first (so metal ores can overwrite). Vanilla places diorite/granite/
        // andesite as size-64 blobs a couple per chunk plus a rarer upper one; we place them
        // surface-RELATIVE (2..72 blocks down) so they fill the rock at any terrain height, with
        // two LARGER tiers (size 112 rarity 1/3, size 160 rarity 1/8) for broad sweeping patches.
        // REPLACEABLE masks to bare rock only, so ores/buildings/decor are never overwritten.
        rel(ANDESITE, 64, N(3), Uniform, 2, 72),
        rel(ANDESITE, 112, Rarity(3), Uniform, 2, 80),
        rel(ANDESITE, 160, Rarity(8), Uniform, 2, 80),
        rel(DIORITE, 64, N(3), Uniform, 2, 72),
        rel(DIORITE, 112, Rarity(3), Uniform, 2, 80),
        rel(DIORITE, 160, Rarity(8), Uniform, 2, 80),
        rel(GRANITE, 64, N(3), Uniform, 2, 72),
        rel(GRANITE, 112, Rarity(3), Uniform, 2, 80),
        rel(GRANITE, 160, Rarity(8), Uniform, 2, 80),
        // dirt/coarse-dirt/gravel pockets, also surface-relative so they pepper the shallow rock.
        rel(DIRT, 33, N(7), Uniform, 2, 90),
        rel(GRAVEL, 33, N(10), Uniform, 2, 110),
        // tuff stays deep (absolute), pairs with the deepslate zone.
        o(TUFF, TUFF, 64, N(2), Uniform, -64, 0, 0.0),
        o(TUFF, TUFF, 112, Rarity(3), Uniform, -64, 0, 0.0),
        o(TUFF, TUFF, 160, Rarity(8), Uniform, -64, 0, 0.0),
        o(GRAVEL, GRAVEL, 33, N(6), Uniform, -64, 0, 0.0),
        // metal/gem ores
        o(
            COAL_ORE,
            DEEPSLATE_COAL_ORE,
            17,
            N(30),
            Uniform,
            136,
            256,
            0.0,
        ),
        o(
            COAL_ORE,
            DEEPSLATE_COAL_ORE,
            17,
            N(20),
            Trapezoid,
            0,
            192,
            0.5,
        ),
        o(
            IRON_ORE,
            DEEPSLATE_IRON_ORE,
            9,
            N(90),
            Trapezoid,
            80,
            256,
            0.0,
        ),
        o(
            IRON_ORE,
            DEEPSLATE_IRON_ORE,
            9,
            N(10),
            Trapezoid,
            -24,
            56,
            0.0,
        ),
        o(
            IRON_ORE,
            DEEPSLATE_IRON_ORE,
            4,
            N(10),
            Uniform,
            -64,
            72,
            0.0,
        ),
        o(
            COPPER_ORE,
            DEEPSLATE_COPPER_ORE,
            10,
            N(16),
            Trapezoid,
            -16,
            112,
            0.0,
        ),
        o(
            COPPER_ORE,
            DEEPSLATE_COPPER_ORE,
            20,
            N(16),
            Trapezoid,
            -16,
            112,
            0.0,
        ),
        o(
            GOLD_ORE,
            DEEPSLATE_GOLD_ORE,
            9,
            N(4),
            Trapezoid,
            -64,
            32,
            0.5,
        ),
        o(
            REDSTONE_ORE,
            DEEPSLATE_REDSTONE_ORE,
            8,
            N(4),
            Uniform,
            -64,
            15,
            0.0,
        ),
        o(
            REDSTONE_ORE,
            DEEPSLATE_REDSTONE_ORE,
            8,
            N(8),
            Trapezoid,
            -96,
            -32,
            0.0,
        ),
        o(
            LAPIS_ORE,
            DEEPSLATE_LAPIS_ORE,
            7,
            N(2),
            Trapezoid,
            -32,
            32,
            0.0,
        ),
        o(
            LAPIS_ORE,
            DEEPSLATE_LAPIS_ORE,
            7,
            N(4),
            Uniform,
            -64,
            64,
            1.0,
        ),
        o(
            DIAMOND_ORE,
            DEEPSLATE_DIAMOND_ORE,
            4,
            N(7),
            Trapezoid,
            -64,
            16,
            0.5,
        ),
        o(
            DIAMOND_ORE,
            DEEPSLATE_DIAMOND_ORE,
            8,
            N(2),
            Uniform,
            -64,
            -4,
            0.5,
        ),
        o(
            DIAMOND_ORE,
            DEEPSLATE_DIAMOND_ORE,
            12,
            Rarity(9),
            Trapezoid,
            -64,
            16,
            0.7,
        ),
        o(
            DIAMOND_ORE,
            DEEPSLATE_DIAMOND_ORE,
            8,
            N(4),
            Trapezoid,
            -64,
            16,
            1.0,
        ),
    ]
}

#[inline]
fn chunk_rng(seed: i64, cx: i32, cz: i32) -> XoroRandom {
    let s = seed
        ^ (cx as i64).wrapping_mul(0x1F1F_3C71)
        ^ (cz as i64).wrapping_mul(0x9E37_79B1)
        ^ 0x0EE5_0EE5_i64; // distinct ore salt
    XoroRandom::from_seed(s)
}

#[inline]
fn sample_y(dist: Dist, lo: i32, hi: i32, r: &mut XoroRandom) -> i32 {
    let range = hi - lo;
    if range <= 0 {
        return lo;
    }
    match dist {
        Dist::Uniform => lo + r.next_int(range + 1),
        // trapezoid (plateau 0) = symmetric triangle = average of two uniforms
        Dist::Trapezoid => lo + (r.next_int(range + 1) + r.next_int(range + 1)) / 2,
    }
}

/// Place all vanilla ores across the bbox (+1 chunk margin so edge blobs complete). Sequential
/// (needs world read/write for discard-on-air).
pub fn place_ores(
    editor: &mut WorldEditor,
    seed: i64,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) {
    let table = ore_table();
    let shift = super::y_shift();
    let cx0 = min_x.div_euclid(16) - 1;
    let cx1 = max_x.div_euclid(16) + 1;
    let cz0 = min_z.div_euclid(16) - 1;
    let cz1 = max_z.div_euclid(16) + 1;
    for cx in cx0..=cx1 {
        for cz in cz0..=cz1 {
            let mut r = chunk_rng(seed, cx, cz);
            for ore in &table {
                let attempts = match ore.count {
                    Count::N(n) => n,
                    Count::Rarity(c) => {
                        if r.next_int(c) == 0 {
                            1
                        } else {
                            0
                        }
                    }
                };
                for _ in 0..attempts {
                    let x = cx * 16 + r.next_int(16);
                    let z = cz * 16 + r.next_int(16);
                    let gl = editor.get_ground_level(x, z);
                    // surface-relative variants: center Y = surface − depth (always in the rock).
                    // absolute ores: sampled at vanilla Y, translated onto this world's floor,
                    // then pruned if above the local surface.
                    let y = if ore.rel {
                        gl - sample_y(ore.dist, ore.ymin, ore.ymax, &mut r)
                    } else {
                        sample_y(ore.dist, ore.ymin, ore.ymax, &mut r) + shift
                    };
                    if y > gl - 2 {
                        continue;
                    }
                    // skip blobs centered in a CAVE (air) — otherwise the sphere's rock rim paints a
                    // floating disc/shell across the cave interior.
                    if !editor.block_exists_absolute(x, y, z) {
                        continue;
                    }
                    place_blob(editor, x, y, z, ore, &mut r);
                }
            }
        }
    }
}

/// Count of the 6 face-neighbors that are air (no solid block).
fn air_neighbors(editor: &WorldEditor, x: i32, y: i32, z: i32) -> u32 {
    let n = [
        (x + 1, y, z),
        (x - 1, y, z),
        (x, y + 1, z),
        (x, y - 1, z),
        (x, y, z + 1),
        (x, y, z - 1),
    ];
    n.iter()
        .filter(|&&(a, b, c)| !editor.block_exists_absolute(a, b, c))
        .count() as u32
}

fn place_blob(editor: &mut WorldEditor, cx: i32, cy: i32, cz: i32, ore: &Ore, r: &mut XoroRandom) {
    let f = r.next_float() as f64 * PI;
    let half_len = ore.size as f64 / 8.0;
    let (ax, az) = (f.sin(), f.cos());
    let mut placed = 0;
    for i in 0..ore.size {
        let t = i as f64 / ore.size as f64;
        let px = cx as f64 + ax * half_len * (t * 2.0 - 1.0);
        let pz = cz as f64 + az * half_len * (t * 2.0 - 1.0);
        let py = cy + r.next_int(3) - 2;
        let rad = ((PI * t).sin() + 1.0) * (ore.size as f64 / 16.0) * 0.5 + 0.5;
        let ri = rad.ceil() as i32;
        let r2 = rad * rad;
        for dx in -ri..=ri {
            for dy in -ri..=ri {
                for dz in -ri..=ri {
                    if (dx * dx + dy * dy + dz * dz) as f64 > r2 {
                        continue;
                    }
                    let bx = px.round() as i32 + dx;
                    let by = py + dy;
                    let bz = pz.round() as i32 + dz;
                    // CRITICAL: never write into cave air. `set_block_absolute` treats an empty (air)
                    // cell as ALWAYS insertable — the REPLACEABLE whitelist only restricts replacing
                    // *existing* blocks — so without this guard the blob paints ore/dirt/granite
                    // straight into the cave as floating discs (and, since an air cell isn't
                    // deepslate, host-matching picks the wrong stone variant before placing it into
                    // the air). Require solid rock here.
                    if !editor.block_exists_absolute(bx, by, bz) {
                        continue;
                    }
                    let air = air_neighbors(editor, bx, by, bz);
                    // Mask to the rock: skip wall fins/spikes/protrusions (>=3 air faces) so the blob sits
                    // flush in the surface, never sticking out into the cave. Flat walls/edges (<=2) keep
                    // the vanilla "ore in the wall" look.
                    if air >= 3 {
                        continue;
                    }
                    // discard-on-air-exposure: buried variants skip near any cave wall.
                    if ore.discard > 0.0 && air >= 1 && r.next_float() < ore.discard {
                        continue;
                    }
                    // match the variant to the ACTUAL host rock (the stone→deepslate transition is
                    // not at y=0), so we never get deepslate ore in stone or vice versa.
                    let host_deep = editor.check_for_block_absolute(
                        bx,
                        by,
                        bz,
                        Some(&[DEEPSLATE, COBBLED_DEEPSLATE]),
                        None,
                    );
                    let blk = if host_deep { ore.deep } else { ore.stone };
                    editor.set_block_absolute(blk, bx, by, bz, Some(REPLACEABLE), None);
                    placed += 1;
                    if placed >= ore.size {
                        return;
                    }
                }
            }
        }
    }
}
