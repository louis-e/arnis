//! Clean-room port of the vanilla 1.21.8 random-walk carvers (`minecraft:cave`,
//! `cave_extra_underground`, `canyon`) — the winding round tunnels + ravines that the noise field
//! alone doesn't make.
//!
//! Terrain-decoupled + vanilla-LOOK (not seed-exact): each origin chunk gets a deterministic,
//! seam-stable RNG (same (cx,cz) → same result across tiles), and we reproduce vanilla's exact DRAW
//! ORDER + geometry. Carve positions are produced as pure geometry (parallel) in vanilla
//! coordinates, translated onto this world's floor, then applied by the caller against the real
//! world (rock-only, below the surface seal).

use super::rng::XoroRandom;
use rayon::prelude::*;
use std::f64::consts::PI;

const CAVE_RANGE_CHUNKS: i32 = 8; // origin-chunk margin (vanilla reach = getRange*2-1 = 7)
const BRANCH_BUDGET: i32 = 112; // SectionPos.sectionToBlockCoord(range*2-1)
const MIN_Y_CARVE: i32 = super::VANILLA_FLOOR;

/// Carve config (the two cave carvers + the canyon).
struct Cfg {
    salt: i64,
    probability: f32,
    y_min: i32,
    y_max: i32,
}

/// Produce all carver AIR positions over the block rect, deduped. Pure (no world access) so it runs
/// in parallel; the caller filters to rock below the surface seal. The walk runs in vanilla
/// coordinates and the result is translated by [`super::y_shift`].
pub fn carve_positions(
    seed: i64,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) -> Vec<(i32, i32, i32)> {
    let caves = [
        Cfg {
            salt: 0x0CA5_0CA5,
            probability: 0.075,
            y_min: -56,
            y_max: 180,
        }, // cave
        Cfg {
            salt: 0xE547_E547,
            probability: 0.025,
            y_min: -56,
            y_max: 47,
        }, // cave_extra_underground
    ];
    let cx0 = min_x.div_euclid(16) - CAVE_RANGE_CHUNKS;
    let cx1 = max_x.div_euclid(16) + CAVE_RANGE_CHUNKS;
    let cz0 = min_z.div_euclid(16) - CAVE_RANGE_CHUNKS;
    let cz1 = max_z.div_euclid(16) + CAVE_RANGE_CHUNKS;

    let chunks: Vec<(i32, i32)> = (cx0..=cx1)
        .flat_map(|cx| (cz0..=cz1).map(move |cz| (cx, cz)))
        .collect();

    let mut out: Vec<(i32, i32, i32)> = chunks
        .par_iter()
        .flat_map_iter(|&(cx, cz)| {
            let mut pts: Vec<(i32, i32, i32)> = Vec::new();
            for cfg in &caves {
                cave_chunk(seed, cfg, cx, cz, min_x, max_x, min_z, max_z, &mut pts);
            }
            canyon_chunk(seed, cx, cz, min_x, max_x, min_z, max_z, &mut pts);
            pts
        })
        .collect();
    let shift = super::y_shift();
    for p in &mut out {
        p.1 += shift;
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[inline]
fn chunk_rng(seed: i64, cx: i32, cz: i32, salt: i64) -> XoroRandom {
    // deterministic, seam-stable per (cx,cz): same chunk → same carving across tiles.
    let s = seed
        ^ (cx as i64).wrapping_mul(341873128712)
        ^ (cz as i64).wrapping_mul(132897987541)
        ^ salt;
    XoroRandom::from_seed(s)
}

// ---- cave + cave_extra (round tunnels) ----
#[allow(clippy::too_many_arguments)]
fn cave_chunk(
    seed: i64,
    cfg: &Cfg,
    cx: i32,
    cz: i32,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    out: &mut Vec<(i32, i32, i32)>,
) {
    let mut r = chunk_rng(seed, cx, cz, cfg.salt);
    if r.next_float() > cfg.probability {
        return;
    }
    // triple-nested nextInt → vanilla's skewed origin count (usually 0-2, rarely a cluster).
    // Evaluated inner→outer (Rust can't double-borrow in one expression; same order as vanilla).
    let a = r.next_int(15) + 1;
    let b = r.next_int(a) + 1;
    let n = r.next_int(b);
    for _ in 0..n {
        let ox = cx * 16 + r.next_int(16);
        let oy = cfg.y_min + r.next_int(cfg.y_max - cfg.y_min + 1);
        let oz = cz * 16 + r.next_int(16);
        let h_mult = 0.7 + r.next_float() as f64 * 0.7; // 0.7..1.4
        let v_mult = 0.8 + r.next_float() as f64 * 0.5; // 0.8..1.3
        let floor_level = -1.0 + r.next_float() as f64 * 0.6; // -1.0..-0.4

        let mut tunnels = 1;
        if r.next_int(4) == 0 {
            // a room: one fat blob
            let y_scale = 0.1 + r.next_float() as f64 * 0.8;
            let f = 1.0 + r.next_float() as f64 * 2.0; // room radius 1..3 (vanilla rolls 1..7; capped to keep rooms moderate)
            let d = 1.5 + f;
            carve_ellipsoid(
                ox as f64 + 1.0,
                oy as f64,
                oz as f64,
                d,
                d * y_scale,
                floor_level,
                cx,
                cz,
                min_x,
                max_x,
                min_z,
                max_z,
                out,
            );
            tunnels += r.next_int(4);
        }
        for _ in 0..tunnels {
            let yaw = r.next_float() as f64 * 2.0 * PI;
            let pitch = (r.next_float() as f64 - 0.5) / 4.0;
            let thickness = get_thickness(&mut r);
            let branch_count = BRANCH_BUDGET - r.next_int(BRANCH_BUDGET / 4);
            let tseed = r.next_long();
            create_tunnel(
                tseed,
                ox as f64,
                oy as f64,
                oz as f64,
                h_mult,
                v_mult,
                thickness,
                yaw,
                pitch,
                0,
                branch_count,
                1.0,
                floor_level,
                cx,
                cz,
                min_x,
                max_x,
                min_z,
                max_z,
                out,
            );
        }
    }
}

fn get_thickness(r: &mut XoroRandom) -> f64 {
    let mut f = r.next_float() as f64 * 2.0 + r.next_float() as f64;
    if r.next_int(10) == 0 {
        f *= r.next_float() as f64 * r.next_float() as f64 * 3.0 + 1.0;
    }
    // Cap the rare thickness spike: vanilla's `*= rand*rand*3+1` can reach ~12 → tunnel ellipsoids
    // of d=1.5+thickness ~13.5 radius, i.e. giant spherical blobs mid-tunnel. Capping at 3.0 keeps
    // tunnels moderate (d ~4.5 max radius) while leaving topology/connectivity unchanged.
    f.min(3.0)
}

#[allow(clippy::too_many_arguments)]
fn create_tunnel(
    tseed: i64,
    mut x: f64,
    mut y: f64,
    mut z: f64,
    h_mult: f64,
    v_mult: f64,
    thickness: f64,
    mut yaw: f64,
    mut pitch: f64,
    branch_index: i32,
    branch_count: i32,
    horiz_vert_ratio: f64,
    floor_level: f64,
    cx: i32,
    cz: i32,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    out: &mut Vec<(i32, i32, i32)>,
) {
    let mut r = XoroRandom::from_seed(tseed);
    let branch_point = r.next_int(branch_count / 2) + branch_count / 4;
    let steep = r.next_int(6) == 0;
    let mut yaw_delta = 0.0f64;
    let mut pitch_delta = 0.0f64;

    for step in branch_index..branch_count {
        let d = 1.5 + (PI * step as f64 / branch_count as f64).sin() * thickness;
        let d1 = d * horiz_vert_ratio;
        let cos_pitch = pitch.cos();
        x += yaw.cos() * cos_pitch;
        y += pitch.sin();
        z += yaw.sin() * cos_pitch;
        pitch *= if steep { 0.92 } else { 0.7 };
        pitch += pitch_delta * 0.1;
        yaw += yaw_delta * 0.1;
        pitch_delta *= 0.9;
        yaw_delta *= 0.75;
        pitch_delta +=
            (r.next_float() as f64 - r.next_float() as f64) * r.next_float() as f64 * 2.0;
        yaw_delta += (r.next_float() as f64 - r.next_float() as f64) * r.next_float() as f64 * 4.0;

        if step == branch_point && thickness > 1.0 {
            let t1 = r.next_float() as f64 * 0.5 + 0.5;
            let s1 = r.next_long();
            let t2 = r.next_float() as f64 * 0.5 + 0.5;
            let s2 = r.next_long();
            create_tunnel(
                s1,
                x,
                y,
                z,
                h_mult,
                v_mult,
                t1,
                yaw - PI / 2.0,
                pitch / 3.0,
                step,
                branch_count,
                1.0,
                floor_level,
                cx,
                cz,
                min_x,
                max_x,
                min_z,
                max_z,
                out,
            );
            create_tunnel(
                s2,
                x,
                y,
                z,
                h_mult,
                v_mult,
                t2,
                yaw + PI / 2.0,
                pitch / 3.0,
                step,
                branch_count,
                1.0,
                floor_level,
                cx,
                cz,
                min_x,
                max_x,
                min_z,
                max_z,
                out,
            );
            return;
        }
        if r.next_int(4) != 0 {
            if !can_reach(cx, cz, x, z, step, branch_count, thickness) {
                return;
            }
            carve_ellipsoid(
                x,
                y,
                z,
                d * h_mult,
                d1 * v_mult,
                floor_level,
                cx,
                cz,
                min_x,
                max_x,
                min_z,
                max_z,
                out,
            );
        }
    }
}

fn can_reach(
    cx: i32,
    cz: i32,
    x: f64,
    z: f64,
    branch_index: i32,
    branch_count: i32,
    width: f64,
) -> bool {
    let mid_x = (cx * 16 + 8) as f64;
    let mid_z = (cz * 16 + 8) as f64;
    let dx = x - mid_x;
    let dz = z - mid_z;
    let remaining = (branch_count - branch_index) as f64;
    let reach = width + 2.0 + 16.0;
    dx * dx + dz * dz - remaining * remaining <= reach * reach
}

#[allow(clippy::too_many_arguments)]
fn carve_ellipsoid(
    x: f64,
    y: f64,
    z: f64,
    horiz_radius: f64,
    vert_radius: f64,
    floor_level: f64,
    _cx: i32,
    _cz: i32,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    out: &mut Vec<(i32, i32, i32)>,
) {
    let min_bx = (x - horiz_radius).floor() as i32;
    let max_bx = (x + horiz_radius).floor() as i32;
    let min_by = ((y - vert_radius).floor() as i32 - 1).max(MIN_Y_CARVE + 1);
    let max_by = (y + vert_radius).floor() as i32 + 1;
    let min_bz = (z - horiz_radius).floor() as i32;
    let max_bz = (z + horiz_radius).floor() as i32;
    for bx in min_bx..=max_bx {
        if bx < min_x || bx > max_x {
            continue;
        }
        let ndx = (bx as f64 + 0.5 - x) / horiz_radius;
        if ndx * ndx >= 1.0 {
            continue;
        }
        for bz in min_bz..=max_bz {
            if bz < min_z || bz > max_z {
                continue;
            }
            let ndz = (bz as f64 + 0.5 - z) / horiz_radius;
            if ndx * ndx + ndz * ndz >= 1.0 {
                continue;
            }
            let mut by = max_by;
            while by > min_by {
                let ndy = (by as f64 - 0.5 - y) / vert_radius;
                // Do not add per-block jitter to this boundary either (see the matching note on the
                // noise-carve threshold in mod.rs): it reads as grainy noise on every tunnel wall.
                // Keep carver walls as clean ellipsoid sweeps.
                if ndy > floor_level && ndx * ndx + ndy * ndy + ndz * ndz < 1.0 {
                    out.push((bx, by, bz));
                }
                by -= 1;
            }
        }
    }
}

// ---- canyon (ravine) ----
#[allow(clippy::too_many_arguments)]
fn canyon_chunk(
    seed: i64,
    cx: i32,
    cz: i32,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    out: &mut Vec<(i32, i32, i32)>,
) {
    let mut r = chunk_rng(seed, cx, cz, 0x4A1E_4A1E);
    if r.next_float() > 0.008 {
        return; // ravines are rare (probability 0.01)
    }
    let ox = (cx * 16 + r.next_int(16)) as f64;
    let oy = (-54 + r.next_int(64)) as f64; // y ~ [-54, 10) band (uniform-ish, vanilla y -64..40)
    let oz = (cz * 16 + r.next_int(16)) as f64;
    let h_mult = 0.7 + r.next_float() as f64 * 0.7;
    let v_mult = 0.8 + r.next_float() as f64 * 0.5;
    // Cap the ravine girth: vanilla's 2*(r*2+r+1) reaches 8, and combined with the per-step width
    // cache (w^2 up to 4) and the vertical stretch below, a single ravine could otherwise carve a
    // smooth ~53-wide x ~74-tall oval — a giant egg-shaped cavern far out of scale with the rest of
    // the network (the noise field itself tops out around ~29-tall runs).
    let thickness = (2.0 * (r.next_float() as f64 * 2.0 + r.next_float() as f64 + 1.0)).min(5.0);
    let yaw = r.next_float() as f64 * 2.0 * PI;
    let pitch = (r.next_float() as f64 - 0.5) / 8.0; // shallow
    let branch_count = BRANCH_BUDGET - r.next_int(BRANCH_BUDGET / 4);
    let tseed = r.next_long();
    let floor_level = -1.0 + r.next_float() as f64 * 0.6;

    // width-over-length cache (vanilla precomputes a per-step widen factor)
    let mut tr = XoroRandom::from_seed(tseed);
    let mut widths = [1.0f64; BRANCH_BUDGET as usize];
    let mut w = 1.0;
    for (wi, width) in widths.iter_mut().enumerate().take(branch_count as usize) {
        if wi == 0 || tr.next_int(3) == 0 {
            w = 1.0 + tr.next_float() as f64 * tr.next_float() as f64;
        }
        *width = (w * w).min(1.8); // widen factor capped (sqrt -> <=1.35x) — see thickness cap
    }

    let mut x = ox;
    let mut y = oy;
    let mut z = oz;
    let mut yaw = yaw;
    let mut pitch = pitch;
    let mut yaw_delta = 0.0f64;
    let mut pitch_delta = 0.0f64;
    for step in 0..branch_count {
        let half = 1.5 + (PI * step as f64 / branch_count as f64).sin() * thickness;
        let vr = half * v_mult;
        let hr = half * h_mult;
        let cos_pitch = pitch.cos();
        x += yaw.cos() * cos_pitch;
        y += pitch.sin();
        z += yaw.sin() * cos_pitch;
        pitch *= 0.7;
        pitch += pitch_delta * 0.05;
        yaw += yaw_delta * 0.05;
        pitch_delta *= 0.8;
        yaw_delta *= 0.5;
        pitch_delta +=
            (tr.next_float() as f64 - tr.next_float() as f64) * tr.next_float() as f64 * 2.0;
        yaw_delta +=
            (tr.next_float() as f64 - tr.next_float() as f64) * tr.next_float() as f64 * 4.0;
        if tr.next_int(4) != 0 {
            if !can_reach(cx, cz, x, z, step, branch_count, thickness) {
                return;
            }
            // ravine cross-section: narrow + tall, widened by the per-step width factor. Vertical
            // stretch reduced from vanilla's 3.0: keeps the tall-ravine identity (~2x taller than
            // wide) without 70-block-tall smooth ovals (max height ~30 at the rarest roll).
            let wfac = widths[step as usize];
            carve_ellipsoid(
                x,
                y,
                z,
                hr * wfac.sqrt(),
                vr * 2.2,
                floor_level,
                cx,
                cz,
                min_x,
                max_x,
                min_z,
                max_z,
                out,
            );
        }
    }
}
