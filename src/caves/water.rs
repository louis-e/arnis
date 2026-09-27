//! Dedicated water-cave generation — pools and rivers get their OWN fresh carve into solid rock,
//! rather than reusing the dry noise-cave shape and deciding per-cell which parts to flood (that
//! approach produces patchy "blob"/curtain water). Two features:
//!   POOL CAVES — a wide multi-lobe (non-circular) room carved fresh into rock; water fills the
//!                bottom HALF, air the top half. REJECTED entirely if its footprint would intersect
//!                the existing dry cave network — a half-filled room punching into a bigger open
//!                cavern reads as floating/unsupported water, so pools only ever stand alone.
//!   RIVERS     — a long, winding, snake-like, net-downhill channel carved fresh into rock and
//!                filled with water (real `schedule_fluid_tick`-ed source blocks, so Minecraft's own
//!                physics settles/streams them). May split up to twice (source → 2 streams, then one
//!                of those → 2 again = max 3 streams from one source). Contact with the existing dry
//!                cave network is DIRECTIONAL, like a real stream: while DESCENDING it breaches —
//!                opens the lip and the ticked water pours down into the cave; while running level
//!                (or the contact is beside/above it) it stays SEALED — the walk stops without
//!                opening the wall, so caves never get holes punched sideways/up into their ceilings.
//!
//! Both are PLANNED against [`CaveShape`] rather than the tile's cave-air set, over every origin
//! that can reach the region, and stop only at the world's bbox. A pool or river crossing a tile
//! edge therefore comes out the same from both tiles, and [`apply`] writes each tile's own part.
use super::decoration::Decor;
use super::rng::XoroRandom;
use super::shape::{CaveShape, Rect, FEATURE_REACH};
use super::{pack, unpack, vy};
use crate::block_definitions::*;
use crate::world_editor::WorldEditor;
// Fnv, not std: std seeds its hasher randomly per process, so iterating one to
// apply world edits gives a different write order -- and a different world --
// every run. FNV is deterministic. See the note in caves/mod.rs.
use fnv::FnvHashMap as HashMap;
use fnv::FnvHashSet as HashSet;
use std::f64::consts::PI;

/// Every block the pools and rivers carve, and the ones left holding water.
#[derive(Default)]
pub(super) struct WaterPlan {
    pub carved: HashSet<i64>,
    pub water: HashSet<i64>,
}

impl WaterPlan {
    /// Whether a block stands at the cell once the plan is applied: rock the carve left alone
    /// (bedrock included, the void below it not), or planned water.
    pub(super) fn solid(&self, shape: &CaveShape, x: i32, y: i32, z: i32) -> bool {
        let p = pack(x, y, z);
        if self.water.contains(&p) {
            return true;
        }
        y >= shape.floor()
            && y <= shape.surf(x, z)
            && !shape.is_cave(x, y, z)
            && !self.carved.contains(&p)
    }
}

/// Plan the pools and rivers of every origin chunk that can reach `region`. Features are
/// planned in global chunk order, so any two tiles agree on every feature they share.
pub(super) fn plan(shape: &CaveShape, decor: &Decor, seed: i64, region: Rect) -> WaterPlan {
    let mut plan = WaterPlan::default();
    let world = shape.world();
    let (cx0, cx1, cz0, cz1) = region.grow(FEATURE_REACH).clip(world).chunks();

    for cx in cx0..=cx1 {
        for cz in cz0..=cz1 {
            // POOL CAVE: rare, one roll per chunk.
            let mut rp = chunk_rng(seed, cx, cz, 0xA044_A044);
            if rp.next_int(30) == 0 {
                plan_pool(shape, decor, &mut rp, cx, cz, &mut plan);
            }
            // RIVER: independent roll.
            let mut rr = chunk_rng(seed, cx, cz, 0xA045_A045);
            if rr.next_int(18) == 0 {
                let ox = cx * 16 + rr.next_int(16);
                let oz = cz * 16 + rr.next_int(16);
                if world.contains(ox, oz) {
                    let top = shape.top(ox, oz);
                    // start HIGH in the band so the long descent has room — a river is "from
                    // somewhere up, going down".
                    let lo = vy(-20);
                    let hi = (top - 8).min(vy(38));
                    if hi > lo
                        // never START inside the existing cave network — a source that spawns in an
                        // open cavern would read as a hole in its ceiling; stay sealed instead.
                        && !shape.is_cave(ox, lo + (hi - lo) / 2, oz)
                    {
                        let oy = lo + rr.next_int(hi - lo + 1);
                        if !shape.is_cave(ox, oy, oz) {
                            let yaw = rr.next_float() as f64 * 2.0 * PI;
                            let steps = 26 + rr.next_int(27); // 26..52 — long
                            let river_seed = rr.next_long();
                            walk_river(
                                shape, &mut plan, river_seed, ox as f64, oy as f64, oz as f64, yaw,
                                steps, 2,
                            );
                        }
                    }
                }
            }
        }
    }
    // UNDERCUT CLEANUP: a river planned later can tunnel UNDER water another river/pool already
    // placed, leaving a source hanging over air (reads as un-updated/floating water and forces the
    // sealer to jam a rock plug under it). Remove any source whose support was carved away — its
    // on-rock neighbors keep the stream alive at runtime.
    let undercut: Vec<i64> = plan
        .water
        .iter()
        .copied()
        .filter(|&p| {
            let (x, y, z) = unpack(p);
            !plan.solid(shape, x, y - 1, z)
        })
        .collect();
    for p in undercut {
        plan.water.remove(&p);
    }
    plan
}

/// Write the part of the plan inside `region`. Returns the cells carved to AIR there (merge into
/// the caller's cave-air set) and the cells filled with WATER (for the later passes to avoid).
pub(super) fn apply(
    editor: &mut WorldEditor,
    plan: &WaterPlan,
    region: Rect,
    cave_host: &[Block],
) -> (HashSet<i64>, HashSet<i64>) {
    let mut carved: HashSet<i64> = HashSet::default();
    let mut water: HashSet<i64> = HashSet::default();
    for &p in &plan.carved {
        let (x, y, z) = unpack(p);
        if region.contains(x, z) {
            editor.set_block_absolute(AIR, x, y, z, Some(cave_host), None);
            carved.insert(p);
        }
    }
    for &p in &plan.water {
        let (x, y, z) = unpack(p);
        if region.contains(x, z) {
            editor.set_block_absolute(WATER, x, y, z, Some(&[AIR]), None);
            editor.schedule_fluid_tick(WATER, x, y, z);
            water.insert(p);
        }
    }
    (carved, water)
}

#[inline]
fn chunk_rng(seed: i64, cx: i32, cz: i32, salt: i64) -> XoroRandom {
    let s = seed
        ^ (cx as i64).wrapping_mul(341873128712)
        ^ (cz as i64).wrapping_mul(132897987541)
        ^ salt;
    XoroRandom::from_seed(s)
}

/// wide multi-lobe (non-circular) room carved fresh into rock; the room's own bottom half fills
/// with water, top half stays air. REJECTED (no-op) if its footprint would touch the existing dry
/// cave network — kept strictly standalone so the half-fill level never looks stranded inside a
/// bigger open void.
fn plan_pool(
    shape: &CaveShape,
    decor: &Decor,
    r: &mut XoroRandom,
    cx: i32,
    cz: i32,
    plan: &mut WaterPlan,
) {
    let world = shape.world();
    let gx = cx * 16 + r.next_int(16);
    let gz = cz * 16 + r.next_int(16);
    if !world.contains(gx, gz) {
        return;
    }
    let top = shape.top(gx, gz);
    // mid-depth band: clear of both the near-surface roof and the deep-lava floor (vanilla y<-54).
    let lo = vy(-48);
    let hi = (top - 10).min(vy(30));
    if hi <= lo {
        return;
    }
    let gy = lo + r.next_int(hi - lo + 1);

    // CORAL ROOMS: a pool landing in a coral blotch becomes a proper flooded reef CAVE — bigger
    // footprint, more lobes, and flooded to ~3/4 of its height instead of half (decoration then
    // grows the reef in it). This is the dedicated "underwater coral cavern" feature.
    let coral = decor.coral_zone(gx, gz);

    // 2-4 overlapping lobes (3-5 for coral rooms), each its own (independent x/z radii →
    // non-circular) mini-ellipsoid, offset from the shared center — a wide, lumpy, decidedly
    // non-circular footprint. Vertical radius stays modest: pools read wide-and-flat, never tall.
    let n_lobes = if coral {
        3 + r.next_int(3)
    } else {
        2 + r.next_int(3)
    };
    struct Lobe {
        ox: i32,
        oz: i32,
        rx: f64,
        rz: f64,
        rv: f64,
    }
    let (r_lo, r_span, v_lo, v_span, o_span) = if coral {
        (6.0, 4.0, 3.0, 1.5, 9) // coral rooms: radii 6..10, height 3..4.5, lobes spread ±4
    } else {
        (5.0, 6.0, 2.5, 1.5, 7) // pools: radii 5..11, height 2.5..4, lobes spread ±3
    };
    // GRAND pools (~1 in 4): one axis stretched ~1.7x — long lake-like galleries instead of
    // another round-ish pond. Composes with coral (a grand coral room = a long flooded reef).
    let grand = r.next_int(4) == 0;
    let stretch_x = r.next_int(2) == 0;
    let lobes: Vec<Lobe> = (0..n_lobes)
        .map(|_| {
            let mut rx = r_lo + r.next_float() as f64 * r_span;
            let mut rz = r_lo + r.next_float() as f64 * r_span; // independent of rx → not circular
            if grand {
                if stretch_x {
                    rx *= 1.45;
                } else {
                    rz *= 1.45;
                }
            }
            Lobe {
                ox: r.next_int(o_span) - o_span / 2,
                oz: r.next_int(o_span) - o_span / 2,
                rx,
                rz,
                rv: v_lo + r.next_float() as f64 * v_span, // flat, not tall
            }
        })
        .collect();
    let ri_h = lobes
        .iter()
        .map(|l| l.rx.max(l.rz))
        .fold(0.0, f64::max)
        .ceil() as i32
        + 4;
    let ri_v = lobes.iter().map(|l| l.rv).fold(0.0, f64::max).ceil() as i32;

    let mut cells: HashSet<(i32, i32, i32)> = HashSet::default();
    for dx in -ri_h..=ri_h {
        for dz in -ri_h..=ri_h {
            let (px, pz) = (gx + dx, gz + dz);
            if !world.contains(px, pz) {
                continue;
            }
            let ptop = shape.top(px, pz);
            for dy in -ri_v..=ri_v {
                let py = gy + dy;
                if py > ptop {
                    continue; // respect the per-column roof seal
                }
                let inside = lobes.iter().any(|l| {
                    let ndx = (dx - l.ox) as f64 / l.rx;
                    let ndz = (dz - l.oz) as f64 / l.rz;
                    let ndy = dy as f64 / l.rv;
                    ndx * ndx + ndy * ndy + ndz * ndz <= 1.0
                });
                if inside {
                    cells.insert((px, py, pz));
                }
            }
        }
    }
    if cells.len() < 40 {
        return; // clipped too hard by the roof/bbox — skip rather than leave a tiny stub
    }
    // REJECT entirely if this footprint would touch the existing dry cave network — a pool must
    // stand fully alone so its "bottom half water" level never looks stranded in a bigger void.
    if cells.iter().any(|&(x, y, z)| shape.is_cave(x, y, z)) {
        return;
    }

    for &(px, py, pz) in &cells {
        plan.carved.insert(pack(px, py, pz));
    }
    // "half water": bottom half of whatever actually got carved (not the nominal radius — a room
    // clipped by the roof/bbox still reads as half-and-half of its real extent). Coral rooms flood
    // to ~3/4 of their height — a properly underwater cave with a slim air pocket at the top.
    let y_lo = cells.iter().map(|&(_, y, _)| y).min().unwrap();
    let y_hi = cells.iter().map(|&(_, y, _)| y).max().unwrap();
    let mid = if coral {
        y_lo + (y_hi - y_lo) * 3 / 4
    } else {
        (y_lo + y_hi) / 2
    };
    // bottom-up SUPPORT-aware fill: a cell gets water only on solid rock or on water already placed
    // — the union of offset lobes can overhang itself, and naively filling every cell below the
    // waterline strands water over the inter-lobe air gaps (visible floating-water bubbles).
    let mut fill: Vec<(i32, i32, i32)> = cells
        .iter()
        .copied()
        .filter(|&(_, y, _)| y <= mid)
        .collect();
    fill.sort_unstable_by_key(|&(_, y, _)| y);
    for &(px, py, pz) in &fill {
        if plan.solid(shape, px, py - 1, pz) {
            plan.water.insert(pack(px, py, pz));
        }
    }
}

/// walk a winding snake-like channel, net-downhill, carving fresh into rock and filling with water
/// (real ticked source blocks). Contact with the existing dry cave network is DIRECTIONAL:
///   - DESCENDING contact (the step that hit was moving down, or the cave air sits below the
///     channel) → BREACH: this step's rim is still carved (existing-air cells skipped, so only the
///     lip opens) and the walk stops — the ticked water at the lip pours down into the cave.
///   - LEVEL/UPWARD contact → SEALED: stop without carving this step at all; the wall stays intact,
///     so no sideways/ceiling holes into existing caves.
///
/// `splits_left` allows up to 2 split events (source → 2 streams, one of which may → 2 again
/// = max 3 streams total from one source).
#[allow(clippy::too_many_arguments)]
fn walk_river(
    shape: &CaveShape,
    plan: &mut WaterPlan,
    seed: i64,
    ox: f64,
    oy: f64,
    oz: f64,
    start_yaw: f64,
    steps: i32,
    splits_left: i32,
) {
    let world = shape.world();
    let mut r = XoroRandom::from_seed(seed);
    let mut yaw = start_yaw;
    let (mut x, mut y, mut z) = (ox, oy, oz);
    let thickness = 1.6 + r.next_float() as f64 * 1.2; // wider: ~1.6..2.8
    let branch_step = steps / 3 + r.next_int((steps / 3).max(1));
    // snake meander: a low-frequency sine sway on top of the random drift gives the channel a
    // regular S-curve rhythm instead of a pure drunkard's walk.
    let sway_phase = r.next_float() as f64 * 2.0 * PI;
    let sway_amp = 0.18 + r.next_float() as f64 * 0.14; // 0.18..0.32 rad

    let mut by_col: HashMap<(i32, i32), i32> = HashMap::default(); // (x,z) -> lowest carved y
    let level_out = f64::from(vy(-50));

    for step in 0..steps {
        yaw += (r.next_float() as f64 - 0.5) * 0.6
            + (step as f64 * 0.35 + sway_phase).sin() * sway_amp;
        x += yaw.cos();
        z += yaw.sin();
        // net-downhill: descend most steps, never climb — a real stream flows down. Levels out at
        // vanilla y=-50: stays clear of the unconditional lava sea below -54 (a stream running into
        // that band would demand water-lava barriers everywhere).
        let descended = r.next_int(4) != 0 && y > level_out;
        if descended {
            y -= 1.0;
        }

        let (cxp, cyp, czp) = (x.round() as i32, y.round() as i32, z.round() as i32);
        if !world.contains(cxp, czp) {
            break;
        }
        // DIRECTIONAL contact rule with the existing dry cave network.
        let contact_here = shape.is_cave(cxp, cyp, czp);
        let contact_below = shape.is_cave(cxp, cyp - 1, czp) || shape.is_cave(cxp, cyp - 2, czp);
        if contact_here && !(descended || contact_below) {
            break; // SEALED: level/upward contact — stop without opening the wall.
        }
        let breach = contact_here || contact_below;

        let ptop = shape.top(cxp, czp);
        let ri = thickness.ceil() as i32;
        for dx in -ri..=ri {
            for dz in -ri..=ri {
                if ((dx * dx + dz * dz) as f64) > thickness * thickness {
                    continue;
                }
                let (px, pz) = (cxp + dx, czp + dz);
                if !world.contains(px, pz) {
                    continue;
                }
                for dy in 0..=1 {
                    let py = cyp - dy;
                    if py > ptop {
                        continue;
                    }
                    if shape.is_cave(px, py, pz) {
                        continue; // never carve the existing cave's own cells
                    }
                    plan.carved.insert(pack(px, py, pz));
                    by_col
                        .entry((px, pz))
                        .and_modify(|e| {
                            if py < *e {
                                *e = py;
                            }
                        })
                        .or_insert(py);
                }
            }
        }
        if breach {
            break; // BREACH: lip carved (above), ticked water will pour down into the cave.
        }

        // SPLIT: fork into 2 diverging sub-streams. The source may split once (→2), and exactly one
        // of those streams may split once more (→3 total) — never more than 3 streams per source.
        if splits_left > 0 && step == branch_step && steps - step > 8 {
            let remaining = steps - step;
            for i in 0..2 {
                let spread = 0.5 + r.next_float() as f64 * 0.5; // radians of divergence
                let fork_yaw = if i == 0 { yaw - spread } else { yaw + spread };
                // only the FIRST fork inherits the remaining split budget — caps total streams at 3.
                let child_splits = if i == 0 { splits_left - 1 } else { 0 };
                let fork_seed = r.next_long();
                walk_river(
                    shape,
                    plan,
                    fork_seed,
                    x,
                    y,
                    z,
                    fork_yaw,
                    remaining,
                    child_splits,
                );
            }
            break; // the parent walk hands off to its forks and stops
        }
    }
    if by_col.len() < 4 {
        return; // too short after bbox/roof/cut clipping — not worth a river that's basically a puddle
    }
    for (&(px, pz), &py) in &by_col {
        // never place a source over air (a breach lip over a cave/pool): the neighboring on-rock
        // source flows over the edge at runtime — a real waterfall — and the floating-fluid sealer
        // has nothing to plug under the river mouth.
        if plan.solid(shape, px, py - 1, pz) {
            plan.water.insert(pack(px, py, pz));
        }
    }
}
