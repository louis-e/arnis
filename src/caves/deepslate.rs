//! Stone→deepslate transition for the filled columns under `--caves`: solid deepslate for the
//! bottom ~30% of the underground band, then a short noisy interleave up to plain stone — the
//! vanilla-style deepslate line, wobbled by a smooth low-frequency noise so it reads as a coherent
//! wave instead of per-voxel speckle. Runs on the filled ground BEFORE the cave carve (the cave
//! engine's stone/deepslate host-matching depends on this line existing).

use crate::block_definitions::*;
use crate::world_editor::{terrain_floor_y, WorldEditor};

/// Bottom of the underground band the fraction is measured over, above the bedrock plane.
const BAND_FLOOR_OFFSET: i32 = 5;
/// The band's ceiling sits this many blocks below the surface.
const BAND_CAP: i32 = 10;
const DEEPSLATE_FRACTION: i32 = 30; // bottom % of the band that is solid deepslate
const DEEPSLATE_BAND: i32 = 2; // ± wobble of the solid-deepslate top line
const DEEPSLATE_TRANSITION_PCT: i32 = 5; // top % of the band = noisy deepslate↔stone interleave
const DEEPSLATE_TRANSITION_MIN: i32 = 3; // ...at least this many blocks tall

pub(super) fn apply_region(
    editor: &mut WorldEditor,
    iter_min_x: i32,
    iter_max_x: i32,
    iter_min_z: i32,
    iter_max_z: i32,
) {
    let seed = 0xDEE9_5147;
    let floor = terrain_floor_y();
    let band_floor = floor + BAND_FLOOR_OFFSET;
    for x in iter_min_x..=iter_max_x {
        for z in iter_min_z..=iter_max_z {
            let surf = editor.get_ground_level(x, z);
            let ceiling = surf - BAND_CAP;
            let band = ceiling - band_floor;
            if band < 8 {
                continue;
            }
            let thresh = band_floor + (band * DEEPSLATE_FRACTION) / 100;
            let wob =
                ((vnoise3(x, 0, z, 24, seed) - 0.5) * 2.0 * DEEPSLATE_BAND as f64).round() as i32;
            let line = thresh + wob;
            let transition =
                ((band * DEEPSLATE_TRANSITION_PCT) / 100).max(DEEPSLATE_TRANSITION_MIN);
            for y in (floor + 1)..=(line + transition) {
                let make = if y <= line {
                    true
                } else {
                    let frac = ((line + transition - y) as u64 * 100) / (transition as u64).max(1);
                    coord_hash3(x, y, z, seed ^ 0x6A6A) % 100 < frac
                };
                // BOTH conditions required: the check GUARD stops writes into EMPTY/void cells
                // (set_block inserts unconditionally into absent cells → floating deepslate cubes
                // in cave air without it), and the STONE write whitelist makes the write an
                // overwrite (a None/None write is insert-if-absent and would no-op on stone).
                if make && editor.check_for_block_absolute(x, y, z, Some(&[STONE]), None) {
                    editor.set_block_absolute(DEEPSLATE, x, y, z, Some(&[STONE]), None);
                }
            }
        }
    }
}

// 3D value noise over GLOBAL coordinates (seam-safe, no noise crate).

fn coord_hash3(x: i32, y: i32, z: i32, seed: u64) -> u64 {
    // ORIGIN-SAFE: each coordinate is OFFSET before the multiply so a zero coordinate still
    // perturbs the hash (a plain `(v as u64)*k` collapses along the x=0/y=0/z=0 planes and drew a
    // visible artifact cross at world origin).
    let mix = |v: i32, k: u64| -> u64 {
        let mut h = (v as u32 as u64)
            .wrapping_add(0x9E37_79B9_7F4A_7C15)
            .wrapping_mul(k);
        h ^= h >> 29;
        h
    };
    let mut h = seed
        ^ mix(x, 0x9E37_79B9_7F4A_7C15)
        ^ mix(y, 0xC2B2_AE3D_27D4_EB4F)
        ^ mix(z, 0x1656_67B1_9E37_79F9);
    h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^ (h >> 29)
}

/// Trilinear value noise in [0,1) on a global lattice of period `period` blocks.
fn vnoise3(x: i32, y: i32, z: i32, period: i32, seed: u64) -> f64 {
    let s = period.max(1);
    let x0 = x.div_euclid(s);
    let y0 = y.div_euclid(s);
    let z0 = z.div_euclid(s);
    let fx = (x - x0 * s) as f64 / s as f64;
    let fy = (y - y0 * s) as f64 / s as f64;
    let fz = (z - z0 * s) as f64 / s as f64;
    let sm = |t: f64| t * t * (3.0 - 2.0 * t);
    let (ux, uy, uz) = (sm(fx), sm(fy), sm(fz));
    let corner = |cx: i32, cy: i32, cz: i32| -> f64 {
        (coord_hash3(cx, cy, cz, seed) % 100_000) as f64 / 100_000.0
    };
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let c000 = corner(x0, y0, z0);
    let c100 = corner(x0 + 1, y0, z0);
    let c010 = corner(x0, y0 + 1, z0);
    let c110 = corner(x0 + 1, y0 + 1, z0);
    let c001 = corner(x0, y0, z0 + 1);
    let c101 = corner(x0 + 1, y0, z0 + 1);
    let c011 = corner(x0, y0 + 1, z0 + 1);
    let c111 = corner(x0 + 1, y0 + 1, z0 + 1);
    let x00 = lerp(c000, c100, ux);
    let x10 = lerp(c010, c110, ux);
    let x01 = lerp(c001, c101, ux);
    let x11 = lerp(c011, c111, ux);
    let yy0 = lerp(x00, x10, uy);
    let yy1 = lerp(x01, x11, uy);
    lerp(yy0, yy1, uz)
}
