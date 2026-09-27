//! Cave worldgen (`--caves`): a from-scratch Rust port of Minecraft 1.21.8 cave generation
//! (clean-room from the decompiled mojmap source and worldgen JSON; the noise math is
//! bit-identical, validated by a Java value-parity harness), carved directly into the solid
//! `--fillground` columns. Terrain-decoupled: below the surface, vanilla caves are a pure 3D
//! position-noise function, so per-tile carving is automatically seamless.
//!
//! THE PIPELINE (carve_region, in order):
//!   0. Deepslate (`deepslate.rs`) — the stone→deepslate transition the ore and rim passes match.
//!   1. Surface heightmap — every pass respects `surf − TOP_GATE` (the roof seal; caves never
//!      breach the surface or expose building foundations).
//!   2. NOISE CAVES — the vanilla density field (cheese caverns + spaghetti tunnels + entrance
//!      pockets + pillars), sampled at 4×8×4 cell corners and trilerped like vanilla, plus
//!      per-block thin "noodle" worms (the connectors between cave systems). Carve where ≤ 0.
//!      Shape knobs (depth-tapered shrinks that keep shallow caves modest and let deep ones open
//!      up) live in `density.rs`.
//!   3. CARVERS — vanilla's random-walk tunnels + rare ravines (`carver.rs`), per-chunk seeded so
//!      tiles agree.
//!   4. DESPECKLE + MICRO-CAVE PRUNE — remove floating rock spikes, then refill any isolated cave
//!      pocket under 48 blocks (tile-edge pockets kept: the neighbor tile carves its half).
//!   5. WATER FEATURES (`water.rs`) — pool caves (multi-lobe rooms, bottom half water; big "grand"
//!      and coral-reef variants) and snake rivers (long, meandering, downhill, up to 3 streams per
//!      source, breach INTO caves only while descending). All real ticked source blocks, all
//!      support-checked; water and lava may never touch (stone barrier + placement guards).
//!   6. DEEP LAVA SEA — everything in the bottom 10 blocks floods with contained, supported lava;
//!      rock faces touching it get obsidian/magma rims.
//!   7. FORMATIONS (`schems.rs`) — curated .schem assets (ice spikes, dripstone columns, amethyst
//!      clusters, clay basins…) from an optional cave asset pack, stamped on cave floors/ceilings,
//!      themed by biome zone, clipped safely against walls.
//!   8. ORES (`ores.rs`) — the vanilla ore table + stone-variant patches (three size tiers),
//!      masked to bare rock so they never bleed into caves or over other features.
//!   9. DECORATION (`decoration.rs`) — 8 biome themes in noise blotches (~half the underground;
//!      lush, dripstone, deep dark, mushroom, ice [mountains only], amethyst, volcanic [bottom of
//!      world], coral [in water pools]) with buffer strips of plain rock between them, plus glow
//!      lichen and rare amethyst geodes everywhere.
//!
//! Every depth band is vanilla's, measured from vanilla's -64 floor and translated onto this
//! world's bedrock plane (`vy`). With the default ground level that plane IS -64, so the layout
//! is vanilla's exactly; a raised `--ground-level` raises the plane and the caves with it.
//!
//! Everything is a pure function of (seed, position) — deterministic and seam-safe across tiles
//! by construction.
//!
//! Ported from the cave engine Teddy563 wrote for the Meld fork of Arnis
//! (https://github.com/Teddy563/arnis).

mod carver;
pub mod decoration;
mod deepslate;
mod density;
mod noise;
mod ores;
mod rng;
mod schems;
mod water;
pub mod zone_map;

use crate::args::Args;
use crate::block_definitions::*;
use crate::coordinate_system::cartesian::XZBBox;
use crate::world_editor::{terrain_floor_y, WorldEditor};
use decoration::{BiomeAmounts, Decor};
use density::CaveGen;
use rayon::prelude::*;
// FnvHashSet, not std HashSet: std seeds its hasher randomly PER PROCESS, so
// iterating one yields a different order every run. These sets are iterated to apply
// world edits (despeckle, prune, decoration), and where those edits interact the write
// ORDER decides the result -- which made a cave render unreproducible from one run to
// the next. FNV hashes deterministically, so the same insertions iterate the same way
// every time.
use fnv::FnvHashSet as HashSet;

/// World seed for every cave pass. Fixed, so the same area always gets the same caves.
const SEED: i64 = 0xCA7E_CA7E;
/// Vanilla's world floor; every depth constant in the cave passes is written against it.
pub(crate) const VANILLA_FLOOR: i32 = -64;
/// Carve only this many blocks below the column's surface (the roof seal — keeps caves from breaching
/// the surface / exposing grass).
const TOP_GATE: i32 = 6;
/// Global carve-density knob: carve the noise body only where combined density ≤ this (vanilla = 0.0).
/// NOTE kept at 0.0: the `squeeze` clusters density extremely tightly near 0, so even tiny negatives
/// are a cliff (−0.013 → −52% volume AND shatters connectivity 62%→15%). Room-size reduction is done
/// via density::CHEESE_SHRINK instead (shrinks the broad cheese rooms while keeping them as hubs).
const CARVE_THRESHOLD: f64 = 0.0;
/// Rock the carve is allowed to replace (never bedrock, water, buildings, ores, plants).
const CAVE_HOST: &[Block] = &[
    STONE,
    DEEPSLATE,
    TUFF,
    COBBLED_DEEPSLATE,
    GRAVEL,
    DIRT,
    ANDESITE,
    GRANITE,
    DIORITE,
];

/// How far this world's bedrock plane sits above vanilla's.
#[inline]
pub(crate) fn y_shift() -> i32 {
    terrain_floor_y() - VANILLA_FLOOR
}

/// A vanilla Y, translated into this world.
#[inline]
pub(crate) fn vy(y: i32) -> i32 {
    y + y_shift()
}

/// The `--cave-biomes` amounts for this run. `validate_args` has already rejected a bad list,
/// so a parse failure here can only come from a caller that skipped it.
fn biome_amounts(args: &Args) -> BiomeAmounts {
    match args.cave_biomes.as_deref().map(BiomeAmounts::parse) {
        Some(Ok(amounts)) => amounts,
        Some(Err(e)) => {
            eprintln!("Warning: --cave-biomes ignored ({e}); using defaults");
            BiomeAmounts::default()
        }
        None => BiomeAmounts::default(),
    }
}

/// Carve caves into the solid fillground across the whole bbox.
pub fn carve(editor: &mut WorldEditor, args: &Args, xzbbox: &XZBBox) {
    carve_region(
        editor,
        args,
        xzbbox.min_x(),
        xzbbox.max_x(),
        xzbbox.min_z(),
        xzbbox.max_z(),
    );
}

/// Carve over an explicit block-coordinate rect (per-tile callers pass strict tile bounds). Noise is
/// a pure position-fn, so per-tile carving is seamless (same coords → same density).
pub fn carve_region(
    editor: &mut WorldEditor,
    args: &Args,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) {
    // The ore variants and the lava rims match the host rock, so the deepslate line goes first.
    deepslate::apply_region(editor, min_x, max_x, min_z, max_z);

    let seed = SEED;
    let floor = terrain_floor_y();
    let gen = CaveGen::new(seed);
    let decor = Decor::new(seed, biome_amounts(args));
    // resolve + load the cave asset pack once (explicit flag dir, else exe-adjacent `cave-pack/`).
    schems::init_pack(args.cave_asset_pack.as_deref());

    // 1) surface heightmap (sequential — get_ground_level borrows &editor).
    let h = (max_z - min_z + 1) as usize;
    let w = (max_x - min_x + 1) as usize;
    let mut surf = vec![0i32; w * h];
    let mut max_surf = floor;
    for (ix, sx) in (min_x..=max_x).enumerate() {
        for (iz, sz) in (min_z..=max_z).enumerate() {
            let s = editor.get_ground_level(sx, sz);
            surf[ix * h + iz] = s;
            max_surf = max_surf.max(s);
        }
    }
    let surf = &surf;

    // 2) cave mask via vanilla CELL INTERPOLATION (4×8×4 cells: sample the combined cheese/spaghetti/
    //    entrances/pillars density at 8 cell corners, trilerp per block — gives vanilla-sized smooth
    //    rooms instead of per-block-fat blobs). noodle is min'd in per-block at full res. Cells are on
    //    GLOBAL boundaries so tiles/regions stay seamless. ~16× fewer density evals than per-block.
    //    The floor sits on a section boundary, so cells line up with vanilla's after translation.
    const CW: i32 = 4;
    const CH: i32 = 8;
    let cx0 = min_x.div_euclid(CW);
    let cx1 = max_x.div_euclid(CW);
    let cz0 = min_z.div_euclid(CW);
    let cz1 = max_z.div_euclid(CW);
    let cy_lo = (floor + 1).div_euclid(CH);
    let cy_hi = (max_surf - TOP_GATE).div_euclid(CH);

    let cell_cols: Vec<(i32, i32)> = (cx0..=cx1)
        .flat_map(|cx| (cz0..=cz1).map(move |cz| (cx, cz)))
        .collect();

    let carved: Vec<(i32, i32, i32)> = cell_cols
        .par_iter()
        .flat_map_iter(|&(cx, cz)| {
            let mut out: Vec<(i32, i32, i32)> = Vec::new();
            let (wx0, wx1) = (cx * CW, cx * CW + CW);
            let (wz0, wz1) = (cz * CW, cz * CW + CW);
            // A cell's TOP corner plane is the next cell's BOTTOM plane: same four
            // coordinates, so the same four values. Carrying it up the column halves the
            // density evaluations - the dominant cost of cave generation - and cannot
            // change a result, because it reuses values instead of recomputing them.
            let mut b00 = gen.combined_density(wx0, cy_lo * CH, wz0);
            let mut b10 = gen.combined_density(wx1, cy_lo * CH, wz0);
            let mut b01 = gen.combined_density(wx0, cy_lo * CH, wz1);
            let mut b11 = gen.combined_density(wx1, cy_lo * CH, wz1);
            for cy in cy_lo..=cy_hi {
                let (wy0, wy1) = (cy * CH, cy * CH + CH);
                // 8 corners of the combined density (cheese/spaghetti/entrances/pillars + slides+squeeze).
                // The wy0 plane was computed as the previous cell's wy1 plane.
                let (n000, n100, n001, n101) = (b00, b10, b01, b11);
                let n010 = gen.combined_density(wx0, wy1, wz0);
                let n110 = gen.combined_density(wx1, wy1, wz0);
                let n011 = gen.combined_density(wx0, wy1, wz1);
                let n111 = gen.combined_density(wx1, wy1, wz1);
                b00 = n010;
                b10 = n110;
                b01 = n011;
                b11 = n111;
                let by_lo = wy0.max(floor + 1);
                let by_hi = (wy1 - 1).min(max_surf - TOP_GATE);
                for by in by_lo..=by_hi {
                    let fy = (by - wy0) as f64 / CH as f64;
                    let xz00 = lerp(fy, n000, n010);
                    let xz10 = lerp(fy, n100, n110);
                    let xz01 = lerp(fy, n001, n011);
                    let xz11 = lerp(fy, n101, n111);
                    for bx in wx0.max(min_x)..wx1.min(max_x + 1) {
                        let fx = (bx - wx0) as f64 / CW as f64;
                        let z0v = lerp(fx, xz00, xz10);
                        let z1v = lerp(fx, xz01, xz11);
                        let ix = (bx - min_x) as usize;
                        for bz in wz0.max(min_z)..wz1.min(max_z + 1) {
                            let top = surf[ix * h + (bz - min_z) as usize] - TOP_GATE;
                            if by > top {
                                continue;
                            }
                            let fz = (bz - wz0) as f64 / CW as f64;
                            let combined = lerp(fz, z0v, z1v);
                            // Do NOT add per-block jitter to this threshold: the `squeeze`
                            // clusters density so tightly near 0 that even a mean-zero ±0.005
                            // perturbation flips a huge number of cells randomly, producing grainy
                            // salt-and-pepper walls everywhere. Terracing on big caverns is a
                            // separate problem needing a coherent isosurface warp, not noise here.
                            // Carve iff min(combined, noodle) <= 0; noodle is only evaluated when
                            // combined stays solid. Noodle keeps its 0 gate — the thin worms are the
                            // CONNECTORS between cave systems, and trimming them fragments the
                            // network (connectivity collapses to ~29%). Cave size is cut via the
                            // cheese/carver-room shrinks instead, which preserve connectivity.
                            let carve = combined <= CARVE_THRESHOLD
                                || gen.noodle_density(bx, by, bz) <= 0.0;
                            if carve {
                                out.push((bx, by, bz));
                            }
                        }
                    }
                }
            }
            out
        })
        .collect();

    // 3) apply caves (noise + carvers) into rock, tracking every cave-air cell for the despeckle.
    let mut air: HashSet<i64> = HashSet::default();
    for (bx, by, bz) in carved {
        editor.set_block_absolute(AIR, bx, by, bz, Some(CAVE_HOST), None);
        air.insert(pack(bx, by, bz));
    }
    // random-walk CARVERS (winding round tunnels + ravines) — vanilla's other cave system. Pure
    // geometry in parallel, applied below the surface seal into rock only.
    let carver_pts = carver::carve_positions(seed, min_x, max_x, min_z, max_z);
    for (bx, by, bz) in carver_pts {
        if bx < min_x || bx > max_x || bz < min_z || bz > max_z || by < floor + 1 {
            continue;
        }
        let top = surf[(bx - min_x) as usize * h + (bz - min_z) as usize] - TOP_GATE;
        if by > top {
            continue;
        }
        editor.set_block_absolute(AIR, bx, by, bz, Some(CAVE_HOST), None);
        air.insert(pack(bx, by, bz));
    }

    // 4) DESPECKLE: remove floating/spike rock (a solid cell with ≥5 of 6 neighbors = cave air).
    //    Kills the thin rock islands that ore would cling to (the "floating ore" look) and tidies the
    //    holey overlap between noise caves + carvers. 2 passes (islands, then the spikes they expose).
    for _ in 0..2 {
        let mut cand: HashSet<i64> = HashSet::default();
        let mut remove: Vec<(i32, i32, i32)> = Vec::new();
        for &a in &air {
            let (px, py, pz) = unpack(a);
            for (nx, ny, nz) in neighbours(px, py, pz) {
                let np = pack(nx, ny, nz);
                if air.contains(&np) || !cand.insert(np) {
                    continue;
                }
                let airn = neighbours(nx, ny, nz)
                    .iter()
                    .filter(|&&(mx, my, mz)| air.contains(&pack(mx, my, mz)))
                    .count();
                if airn >= 5 {
                    remove.push((nx, ny, nz));
                }
            }
        }
        if remove.is_empty() {
            break;
        }
        for (x, y, z) in remove {
            editor.set_block_absolute(AIR, x, y, z, Some(CAVE_HOST), None);
            air.insert(pack(x, y, z));
        }
    }

    // 4.4) PRUNE MICRO-CAVES: the noise field's carve fringe leaves isolated pockets of just a few
    //    blocks — meaningless "caves" that pockmark cliff faces and read as random holes.
    //    Refill any connected component smaller than 48 cells with rock. Components that
    //    touch the tile boundary are KEPT even when small — they may continue in the neighbor tile,
    //    and refilling only our half would carve a visible seam (the neighbor still carves its side).
    {
        let mut seen: HashSet<i64> = HashSet::default();
        let mut refill: Vec<i64> = Vec::new();
        for &start in &air {
            if seen.contains(&start) {
                continue;
            }
            let mut comp: Vec<i64> = vec![start];
            let mut stack: Vec<i64> = vec![start];
            seen.insert(start);
            let mut touches_edge = false;
            while let Some(p) = stack.pop() {
                let (x, y, z) = unpack(p);
                if x <= min_x || x >= max_x || z <= min_z || z >= max_z {
                    touches_edge = true;
                }
                for (nx, ny, nz) in neighbours(x, y, z) {
                    let np = pack(nx, ny, nz);
                    if air.contains(&np) && seen.insert(np) {
                        stack.push(np);
                        comp.push(np);
                    }
                }
            }
            if comp.len() < 48 && !touches_edge {
                refill.extend(comp);
            }
        }
        for &p in &refill {
            let (x, y, z) = unpack(p);
            let rock = if y < vy(0) { DEEPSLATE } else { STONE };
            editor.set_block_absolute(rock, x, y, z, Some(&[AIR]), None);
            air.remove(&p);
        }
    }

    // 4.5) WATER FEATURES — pools + snake rivers get their OWN fresh carve into solid rock (see
    //    water.rs), instead of reusing the dry cave shape and deciding per-cell which parts to flood
    //    (which produces patchy "blob"/curtain water). Merge their carved cells into `air` so ores/
    //    geodes/decoration treat them consistently with the rest of the cave network. `water_cells`
    //    is PURE water (no lava) — used below to tell a genuine water/lava boundary apart from lava
    //    simply touching more lava (which must NOT trigger the barrier).
    let (feat_carved, water_cells) = water::carve_water_features(
        editor, &air, &decor, seed, min_x, max_x, min_z, max_z, surf, h, CAVE_HOST, TOP_GATE,
    );
    air.extend(feat_carved);
    // union of every placed fluid cell (water + lava) — for the later passes' "don't place near
    // fluid" exclusions only; NOT used for the water/lava barrier check (that needs the type
    // distinction).
    let mut basin_fluid: HashSet<i64> = water_cells.clone();

    // 4.6) DEEP LAVA FLOOR — the unconditional lava sea in the bottom 10 blocks (vanilla: below
    //    y=-54, a flat bottom-of-the-world plane). This is the ONLY carve-time source of lava in the
    //    cave system — there are deliberately no scattered mid-depth lava lakes. Containment (skip
    //    cells touching open terrain) + an ascending-y support sweep keep it from floating or leaking.
    {
        let lava_level = vy(-54);
        let is_open = |ed: &WorldEditor, x: i32, y: i32, z: i32| {
            // A neighbour outside this tile's bbox belongs to the ADJACENT tile, which carves AND
            // fills this same deep column identically (the carve is a pure position fn). Reading
            // it here returns "not placed", which would make the lava sea treat every seam-edge
            // column as touching open terrain and skip it -> a 2-wide air trench along every tile
            // boundary. Never treat an out-of-bbox neighbour as open: decide the edge column from
            // in-bbox neighbours only, exactly as a single whole-bbox pass would.
            if x < min_x || x > max_x || z < min_z || z > max_z {
                return false;
            }
            !ed.block_exists_absolute(x, y, z) && !air.contains(&pack(x, y, z))
        };
        let mut cand: HashSet<i64> = HashSet::default();
        for &a in &air {
            let (x, y, z) = unpack(a);
            if x < min_x || x > max_x || z < min_z || z > max_z || y >= lava_level {
                continue;
            }
            if neighbours(x, y, z)
                .iter()
                .any(|&(nx, ny, nz)| is_open(editor, nx, ny, nz))
            {
                continue;
            }
            cand.insert(a);
        }
        let mut order: Vec<i64> = cand.iter().copied().collect();
        order.sort_unstable_by_key(|&p| unpack(p).1);
        let mut supported: HashSet<i64> = HashSet::default();
        for &a in &order {
            let (x, y, z) = unpack(a);
            if editor.block_exists_absolute(x, y - 1, z) || supported.contains(&pack(x, y - 1, z)) {
                supported.insert(a);
            }
        }
        for &a in &supported {
            let (x, y, z) = unpack(a);
            // stone seam if this deep-lava cell happens to touch a pool/river WATER cell specifically
            // (rare — pools/rivers stay well above the sea, this is only a safety net). Checked
            // against `water_cells` (pure water), NOT `basin_fluid` — lava touching more lava (the
            // normal case in a deep sea) must never trigger this.
            let touches_water = neighbours(x, y, z)
                .iter()
                .any(|&(nx, ny, nz)| water_cells.contains(&pack(nx, ny, nz)));
            if touches_water {
                let barrier = if y < vy(0) { DEEPSLATE } else { STONE };
                editor.set_block_absolute(barrier, x, y, z, Some(&[AIR]), None);
                continue;
            }
            editor.set_block_absolute(LAVA, x, y, z, Some(&[AIR]), None);
            basin_fluid.insert(a);
        }
        // VOLCANIC RIM: rock faces touching the lava sea become obsidian (with magma accents).
        // Deterministic per-cell hash so tiles agree; masked to ROCK-family blocks so
        // ores/bedrock/buildings are never converted.
        let rim_rock: &[Block] = &[
            STONE,
            DEEPSLATE,
            TUFF,
            COBBLED_DEEPSLATE,
            GRANITE,
            DIORITE,
            ANDESITE,
        ];
        for &a in &supported {
            let (x, y, z) = unpack(a);
            for (nx, ny, nz) in neighbours(x, y, z) {
                let np = pack(nx, ny, nz);
                if supported.contains(&np) || air.contains(&np) {
                    continue;
                }
                let hv = (np as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40;
                if hv % 100 < 45 {
                    editor.set_block_absolute(OBSIDIAN, nx, ny, nz, Some(rim_rock), None);
                } else if hv % 100 < 62 {
                    editor.set_block_absolute(MAGMA_BLOCK, nx, ny, nz, Some(rim_rock), None);
                }
            }
        }
    }

    // 4.7) CAVE ASSET FORMATIONS — stamp curated schematics (ice spikes, dripstone columns, amethyst
    //    clusters, snow piles, fluid pockets, dripleaf…) from the cave pack into the finished cave
    //    network, themed by the same biome zones decoration uses. Runs BEFORE ores/decoration so ore
    //    blobs don't spawn inside formations and AIR-only decoration skips formation blocks.
    schems::stamp_region(
        editor,
        &air,
        &decor,
        seed,
        min_x,
        max_x,
        min_z,
        max_z,
        surf,
        h,
        TOP_GATE,
        &mut basin_fluid,
    );

    // NOTE: floating-water sealing is NOT done here — the land-cover water-depth carve runs AFTER
    // this cave pass, so a seal here can't see that water. The caller invokes
    // `seal_floating_fluid_region` after all water generation instead.

    // 5) ORES — vanilla blob ores (+ stone variants), placed into the now-clean rock so
    //    discard-on-air-exposure leaves clean cave walls. deepslate variant matches host rock.
    ores::place_ores(editor, seed, min_x, max_x, min_z, max_z);

    // 6) DECORATION — the biome themes (lush moss + cave-vines, dripstone, sculk, mushroom, ice,
    //    amethyst, volcanic, coral reefs in pools), glow lichen on all surfaces, and rare amethyst
    //    geodes. Biome patches via low-freq noise. No springs/drips: water exists only as the pool/
    //    river features placed above.
    decoration::decorate(
        editor,
        &decor,
        &air,
        &basin_fluid,
        &water_cells,
        min_x,
        max_x,
        min_z,
        max_z,
    );
}

/// Seal floating water/lava: a fluid block with cave air directly below has no support and looks like
/// it floats (the land-cover water-depth carve can undercut surface water over a cave). Re-fill that
/// air cell with rock so every fluid column keeps a bed. MUST be called AFTER all water generation.
/// Per-column scan from the surface down — cheap; no-ops on properly supported cave fluid (its cell
/// below is always rock or more fluid).
pub fn seal_floating_fluid_region(
    editor: &mut WorldEditor,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) {
    let floor = terrain_floor_y();
    // Scan in parallel, then apply. The scan is the expensive half - a full-depth probe
    // of every column - while the writes are rare (only genuine floaters) and must stay
    // on one thread because the world is a hash map behind &mut. Columns never read or
    // write outside their own (x, z), so splitting by column is safe, and rayon's indexed
    // collect preserves input order, which keeps the applied writes in the same sequence
    // as a serial scan.
    let plugs: Vec<(i32, i32, i32)> = (min_x..=max_x)
        .into_par_iter()
        .flat_map_iter(|x| {
            let mut found: Vec<(i32, i32, i32)> = Vec::new();
            for z in min_z..=max_z {
                let top = editor.get_ground_level(x, z) + 2;
                for y in (floor + 1..=top).rev() {
                    if editor.check_for_block_absolute(x, y, z, Some(&[WATER, LAVA]), None)
                        && !editor.block_exists_absolute(x, y - 1, z)
                    {
                        found.push((x, y, z));
                    }
                }
            }
            found
        })
        .collect();

    for (x, y, z) in plugs {
        // Do not add a "waterfall exemption" here (skipping the plug when more fluid
        // sits a few blocks lower): it legalizes water-over-air-over-water gaps inside
        // multi-lobe pools (hundreds of visible floaters per region). The river→pool
        // merge is solved at the SOURCE instead: rivers never place a source block over
        // air (water.rs), so there is nothing here to plug at a river mouth — the last
        // on-rock source flows over the edge at runtime, a real waterfall.
        let rock = if y < vy(1) { DEEPSLATE } else { STONE };
        editor.set_block_absolute(rock, x, y - 1, z, Some(&[AIR]), None);
    }
}

/// The six face neighbours of a block.
#[inline]
fn neighbours(x: i32, y: i32, z: i32) -> [(i32, i32, i32); 6] {
    [
        (x + 1, y, z),
        (x - 1, y, z),
        (x, y + 1, z),
        (x, y - 1, z),
        (x, y, z + 1),
        (x, y, z - 1),
    ]
}

/// Offset that makes every legal Y non-negative in the packed key (the tall floor is -2032).
const PACK_Y_OFFSET: i64 = 2048;

/// Pack/unpack a block coord into an i64 for the cave-air sets (offset so negatives are safe;
/// supports |x|,|z| < 2^23 and y in [-2048, 2047]).
#[inline]
fn pack(x: i32, y: i32, z: i32) -> i64 {
    (((x as i64 + (1 << 23)) & 0xFF_FFFF) << 36)
        | (((z as i64 + (1 << 23)) & 0xFF_FFFF) << 12)
        | ((y as i64 + PACK_Y_OFFSET) & 0xFFF)
}
#[inline]
fn unpack(p: i64) -> (i32, i32, i32) {
    let x = ((p >> 36) & 0xFF_FFFF) as i32 - (1 << 23);
    let z = ((p >> 12) & 0xFF_FFFF) as i32 - (1 << 23);
    let y = ((p & 0xFFF) - PACK_Y_OFFSET) as i32;
    (x, y, z)
}

#[inline]
fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cave passes iterate these sets and apply world edits in whatever order they
    /// come out, so the iteration order IS part of the world. std's HashSet seeds its
    /// hasher randomly per process, which would make the same cave render come out
    /// different every run; FNV hashes deterministically.
    ///
    /// This asserts the property directly rather than the type, so swapping the alias
    /// back to a randomly-seeded hasher fails here instead of silently making renders
    /// unreproducible.
    #[test]
    fn cave_sets_iterate_in_a_stable_order() {
        let build = || {
            let mut set: HashSet<i64> = HashSet::default();
            for i in 0..512i64 {
                set.insert(i.wrapping_mul(0x9E37_79B9).wrapping_add(17));
            }
            set.iter().copied().collect::<Vec<_>>()
        };
        assert_eq!(
            build(),
            build(),
            "the same insertions must iterate identically, or renders stop reproducing"
        );
    }

    /// Every Y a world can hold, including the tall floor and ceiling, survives the key.
    #[test]
    fn pack_round_trips_the_full_height_range() {
        for (x, y, z) in [
            (0, 0, 0),
            (-1, -2032, 7),
            (123_456, 2031, -654_321),
            (-(1 << 22), -64, (1 << 22) - 1),
        ] {
            assert_eq!(unpack(pack(x, y, z)), (x, y, z));
        }
    }

    /// The carve loop reuses each cell's top corner plane as the next cell's bottom
    /// plane instead of recomputing it. That is only sound if the two are the same
    /// four samples, so this walks a column both ways and compares every value.
    #[test]
    fn carried_corner_plane_matches_recomputation() {
        const CW: i32 = 4;
        const CH: i32 = 8;
        let gen = CaveGen::new(1234);
        let (wx0, wx1) = (16, 16 + CW);
        let (wz0, wz1) = (-48, -48 + CW);

        // Naive: every cell computes all eight corners from scratch.
        let mut naive = Vec::new();
        for cy in -8..=2 {
            let (wy0, wy1) = (cy * CH, cy * CH + CH);
            naive.push([
                gen.combined_density(wx0, wy0, wz0),
                gen.combined_density(wx1, wy0, wz0),
                gen.combined_density(wx0, wy0, wz1),
                gen.combined_density(wx1, wy0, wz1),
                gen.combined_density(wx0, wy1, wz0),
                gen.combined_density(wx1, wy1, wz0),
                gen.combined_density(wx0, wy1, wz1),
                gen.combined_density(wx1, wy1, wz1),
            ]);
        }

        // Carried: the bottom plane comes from the previous cell's top plane.
        let mut b00 = gen.combined_density(wx0, -8 * CH, wz0);
        let mut b10 = gen.combined_density(wx1, -8 * CH, wz0);
        let mut b01 = gen.combined_density(wx0, -8 * CH, wz1);
        let mut b11 = gen.combined_density(wx1, -8 * CH, wz1);
        for (i, cy) in (-8..=2).enumerate() {
            let wy1 = cy * CH + CH;
            let (n000, n100, n001, n101) = (b00, b10, b01, b11);
            let n010 = gen.combined_density(wx0, wy1, wz0);
            let n110 = gen.combined_density(wx1, wy1, wz0);
            let n011 = gen.combined_density(wx0, wy1, wz1);
            let n111 = gen.combined_density(wx1, wy1, wz1);
            b00 = n010;
            b10 = n110;
            b01 = n011;
            b11 = n111;

            let carried = [n000, n100, n001, n101, n010, n110, n011, n111];
            assert_eq!(
                carried, naive[i],
                "cell cy={cy}: carried corners differ from recomputed ones"
            );
        }
    }

    /// The density field must be a pure function of its coordinates - the carry is
    /// only valid because asking twice gives the same answer.
    #[test]
    fn combined_density_is_pure() {
        let gen = CaveGen::new(99);
        for (x, y, z) in [(0, -40, 0), (12, -8, -60), (-33, -55, 71)] {
            assert_eq!(gen.combined_density(x, y, z), gen.combined_density(x, y, z));
        }
    }

    const SIDE: i32 = 32;

    fn surface(x: i32, z: i32) -> i32 {
        80 + (x + z) / 8
    }

    /// A filled stone block over the bedrock plane, carved the way a tile is.
    fn carved_block(xzbbox: &XZBBox) -> WorldEditor<'_> {
        use clap::Parser;
        let llbbox =
            crate::coordinate_system::geographic::LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor =
            WorldEditor::new(std::path::PathBuf::from("/dev/null/unused"), xzbbox, llbbox);
        let floor = terrain_floor_y();
        for x in 0..SIDE {
            for z in 0..SIDE {
                let s = surface(x, z);
                editor.register_road_surface_y(x, z, s);
                editor.fill_column_absolute(STONE, x, z, floor + 1, s, false);
                editor.set_block_absolute(BEDROCK, x, floor, z, None, None);
            }
        }
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4", "--caves"]);
        carve_region(&mut editor, &args, 0, SIDE - 1, 0, SIDE - 1);
        editor
    }

    /// Carving the same block twice gives the same world, the caves open up below the
    /// surface without breaching it, lava stays in the bottom band and never meets water.
    #[test]
    fn carving_is_reproducible_and_keeps_its_invariants() {
        use crate::world_editor::{
            set_terrain_floor_y, set_world_bounds, DEFAULT_MAX_Y, DEFAULT_MIN_Y, FLOOR_TEST_LOCK,
        };
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
        set_terrain_floor_y(DEFAULT_MIN_Y + 2);

        let xzbbox = XZBBox::rect_from_min_max(0, 0, SIDE - 1, SIDE - 1).unwrap();
        let world = carved_block(&xzbbox);
        let again = carved_block(&xzbbox);
        assert_eq!(world.content_hash(), again.content_hash());

        let floor = terrain_floor_y();
        let mut cave_air = 0;
        for x in 0..SIDE {
            for z in 0..SIDE {
                let s = surface(x, z);
                assert!(
                    world.block_exists_absolute(x, s, z),
                    "the surface at ({x}, {z}) was breached"
                );
                for y in floor + 1..s {
                    match world.get_block_absolute(x, y, z) {
                        None => cave_air += 1,
                        Some(LAVA) => {
                            assert!(y < vy(-54), "lava above the sea at ({x}, {y}, {z})");
                            for (nx, ny, nz) in neighbours(x, y, z) {
                                assert_ne!(
                                    world.get_block_absolute(nx, ny, nz),
                                    Some(WATER),
                                    "water touches lava at ({x}, {y}, {z})"
                                );
                            }
                        }
                        Some(_) => {}
                    }
                }
            }
        }
        assert!(cave_air > 0, "nothing was carved");
    }
}
