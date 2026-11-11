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
//! Pools, rivers and geodes reach past their origin chunk, so near a tile edge both tiles see
//! them. They are planned against `shape.rs` — the carve as a pure function of position, readable
//! past the region — over every origin that can reach the region, and each tile writes only its
//! own part. No pass writes outside the region: a tile's halo merges into its neighbour's caves.
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
mod shape;
mod water;
pub mod zone_map;

use crate::args::Args;
use crate::block_definitions::*;
use crate::coordinate_system::cartesian::XZBBox;
use crate::world_editor::{terrain_floor_y, WorldEditor};
use decoration::{BiomeAmounts, Decor};
use density::CaveGen;
use rayon::prelude::*;
use shape::{CaveShape, Rect};
use std::sync::atomic::{AtomicI32, Ordering};
// FnvHashSet, not std HashSet: std seeds its hasher randomly PER PROCESS, so
// iterating one yields a different order every run. These sets are iterated to apply
// world edits (despeckle, prune, decoration), and where those edits interact the write
// ORDER decides the result -- which made a cave render unreproducible from one run to
// the next. FNV hashes deterministically, so the same insertions iterate the same way
// every time.
use fnv::FnvHashSet as HashSet;

/// Built-in seed for every cave pass, used unless `--cave-seed` is given. Fixed, so the same
/// area always gets the same caves.
const SEED: i64 = 0xCA7E_CA7E;
/// Vanilla's world floor; every depth constant in the cave passes is written against it.
pub(crate) const VANILLA_FLOOR: i32 = -64;
/// The theme amounts of More Vanilla and More Mix: every theme at this share of its default.
/// Measured by the `style_shares_match` test: More Vanilla is about 30% themed, All Mix (every
/// theme at its default) about 77%, More Mix about 89%. More Mix stays under the 200% top of
/// `--cave-biomes`.
const MORE_VANILLA: f64 = 0.18;
const MORE_MIX: f64 = 1.8;
/// Carve only this many blocks below the column's surface (the roof seal — keeps caves from breaching
/// the surface / exposing grass).
const TOP_GATE: i32 = 6;
/// Global carve-density knob: carve the noise body only where combined density ≤ this (vanilla = 0.0).
/// NOTE kept at 0.0: the `squeeze` clusters density extremely tightly near 0, so even tiny negatives
/// are a cliff (−0.013 → −52% volume AND shatters connectivity 62%→15%). Room-size reduction is done
/// via density::CHEESE_SHRINK instead (shrinks the broad cheese rooms while keeping them as hubs).
const CARVE_THRESHOLD: f64 = 0.0;
/// Noise cells are 4×8×4 blocks, on global boundaries, like vanilla's.
const CELL_W: i32 = 4;
const CELL_H: i32 = 8;
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

/// `--cave-datum-y`, or `i32::MIN` for none. Set from args at the start of every carve.
static DATUM_Y: AtomicI32 = AtomicI32::new(i32::MIN);

fn set_datum_y(y: Option<i32>) {
    DATUM_Y.store(y.unwrap_or(i32::MIN), Ordering::Relaxed);
}

/// Where vanilla's floor sits for the cave passes: `--cave-datum-y` when given, so separate
/// runs share one depth frame, else this world's bedrock plane.
#[inline]
pub(crate) fn datum_y() -> i32 {
    match DATUM_Y.load(Ordering::Relaxed) {
        i32::MIN => terrain_floor_y(),
        y => y,
    }
}

/// Whether `--cave-datum-y` pins the datum for this carve.
#[inline]
pub(crate) fn datum_pinned() -> bool {
    DATUM_Y.load(Ordering::Relaxed) != i32::MIN
}

/// How far the cave datum sits above vanilla's floor.
#[inline]
pub(crate) fn y_shift() -> i32 {
    datum_y() - VANILLA_FLOOR
}

/// A vanilla Y, translated into this world.
#[inline]
pub(crate) fn vy(y: i32) -> i32 {
    y + y_shift()
}

/// This run's cave seed: `--cave-seed` (its bits, so every u64 is a distinct layout), else the
/// built-in one. Independent of any world `--seed`.
pub(crate) fn seed(args: &Args) -> i64 {
    args.cave_seed.map_or(SEED, |s| s as i64)
}

/// `--cave-style`: how much of the underground the themed cave biomes take. A style is one
/// amount for every theme, so every style runs through the same zone picker as `--cave-biomes`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum CaveStyle {
    /// Plain vanilla caves, no themed biomes.
    Vanilla,
    /// About 70% plain caves, 30% themed.
    MoreVanilla,
    /// Every theme at its default amount, about 77% themed.
    #[default]
    AllMix,
    /// Every theme past its default, about 89% themed.
    MoreMix,
}

impl CaveStyle {
    /// Every theme's amount (1.0 = its default).
    pub fn amount(self) -> f64 {
        match self {
            CaveStyle::Vanilla => 0.0,
            CaveStyle::MoreVanilla => MORE_VANILLA,
            CaveStyle::AllMix => 1.0,
            CaveStyle::MoreMix => MORE_MIX,
        }
    }
}

/// `--cave-ores`: the vanilla ore table as is, or with more veins of every metal and gem ore.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum CaveOres {
    #[default]
    Normal,
    /// Twice the veins of every metal and gem ore, at vanilla's heights and vein sizes.
    More,
}

/// The `--cave-style` amounts with `--cave-biomes` over them, or why the list is bad.
pub(crate) fn try_biome_amounts(args: &Args) -> Result<BiomeAmounts, String> {
    let style = BiomeAmounts::uniform(args.cave_style.unwrap_or_default().amount());
    match args.cave_biomes.as_deref() {
        Some(spec) => style.parse_onto(spec),
        None => Ok(style),
    }
}

/// [`try_biome_amounts`] for the carve. `validate_args` has already rejected a bad list, so a
/// failure here can only come from a caller that skipped it.
pub(crate) fn biome_amounts(args: &Args) -> BiomeAmounts {
    try_biome_amounts(args).unwrap_or_else(|e| {
        eprintln!("Warning: --cave-biomes ignored ({e}); using the style's amounts");
        BiomeAmounts::uniform(args.cave_style.unwrap_or_default().amount())
    })
}

/// Carve caves into the solid fillground across the whole bbox.
pub fn carve(
    editor: &mut WorldEditor,
    args: &Args,
    xzbbox: &XZBBox,
    keep_out: &dyn Fn(i32, i32) -> bool,
) {
    carve_region(
        editor,
        args,
        xzbbox,
        xzbbox.min_x(),
        xzbbox.max_x(),
        xzbbox.min_z(),
        xzbbox.max_z(),
        keep_out,
    );
}

/// Carve over an explicit block-coordinate rect (per-tile callers pass strict tile bounds) of the
/// world `world`. Noise is a pure position-fn, so per-tile carving is seamless (same coords → same
/// density), and the features that cross tile edges are planned against the pure [`CaveShape`].
/// Nothing is written outside the rect: a tile editor's halo merges into its neighbour's caves.
/// `keep_out` marks columns dug by tunnels and stations, which geodes keep out of.
#[allow(clippy::too_many_arguments)]
pub fn carve_region(
    editor: &mut WorldEditor,
    args: &Args,
    world: &XZBBox,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    keep_out: &dyn Fn(i32, i32) -> bool,
) {
    set_datum_y(args.cave_datum_y);
    // The ore variants and the lava rims match the host rock, so the deepslate line goes first.
    deepslate::apply_region(editor, min_x, max_x, min_z, max_z);

    let seed = seed(args);
    let floor = terrain_floor_y();
    let gen = CaveGen::new(seed);
    let decor = Decor::new(seed, biome_amounts(args));
    // resolve + load the cave asset pack once (explicit flag dir, else exe-adjacent `cave-pack/`).
    schems::init_pack(args.cave_asset_pack.as_deref());

    let region = Rect {
        min_x,
        max_x,
        min_z,
        max_z,
    };
    // Pools, rivers and geodes stop at the run's bbox, unless --cave-datum-y says this run is
    // one of several that must line up: then they are planned past it, so a feature crossing
    // into a neighbouring area or piece is the same feature on both sides and each writes its
    // part.
    let world = if args.cave_datum_y.is_some() {
        Rect::EVERYWHERE
    } else {
        Rect {
            min_x: world.min_x(),
            max_x: world.max_x(),
            min_z: world.min_z(),
            max_z: world.max_z(),
        }
    };
    let shape = CaveShape::new(&gen, seed, world, region, |x, z| {
        editor.get_ground_level(x, z)
    });

    // 1) surface heightmap, X-major.
    let h = (max_z - min_z + 1) as usize;
    let mut surf = Vec::with_capacity((max_x - min_x + 1) as usize * h);
    for x in min_x..=max_x {
        for z in min_z..=max_z {
            surf.push(shape.surf(x, z));
        }
    }
    let surf = &surf;

    // 2) the noise caves.
    let carved = noise_cells(&gen, region, surf, floor);

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
    let plan = water::plan(&shape, &decor, seed, region);
    let (feat_carved, water_cells) = water::apply(editor, &plan, region, CAVE_HOST);
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
                if !region.contains(nx, nz) || supported.contains(&np) || air.contains(&np) {
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
    let more_ores = args.cave_ores == Some(CaveOres::More);
    ores::place_ores(editor, seed, more_ores, min_x, max_x, min_z, max_z);

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
        &shape,
        &plan,
        region,
        keep_out,
    );
}

/// The noise caves over a region, via vanilla CELL INTERPOLATION (4×8×4 cells: sample the combined
/// cheese/spaghetti/entrances/pillars density at 8 cell corners, trilerp per block — gives
/// vanilla-sized smooth rooms instead of per-block-fat blobs). noodle is min'd in per-block at full
/// res. Cells are on GLOBAL boundaries so tiles/regions stay seamless. ~16× fewer density evals than
/// per-block. The floor sits on a section boundary, so cells line up with vanilla's after
/// translation. `surf` is the region's surface, X-major. [`CaveShape`] answers the same question for
/// one block at a time, with the same arithmetic.
fn noise_cells(gen: &CaveGen, region: Rect, surf: &[i32], floor: i32) -> Vec<(i32, i32, i32)> {
    let Rect {
        min_x,
        max_x,
        min_z,
        max_z,
    } = region;
    let h = (max_z - min_z + 1) as usize;
    let max_surf = surf.iter().copied().fold(floor, i32::max);
    let cx0 = min_x.div_euclid(CELL_W);
    let cx1 = max_x.div_euclid(CELL_W);
    let cz0 = min_z.div_euclid(CELL_W);
    let cz1 = max_z.div_euclid(CELL_W);
    let cy_lo = (floor + 1).div_euclid(CELL_H);
    let cy_hi = (max_surf - TOP_GATE).div_euclid(CELL_H);

    let cell_cols: Vec<(i32, i32)> = (cx0..=cx1)
        .flat_map(|cx| (cz0..=cz1).map(move |cz| (cx, cz)))
        .collect();

    cell_cols
        .par_iter()
        .flat_map_iter(|&(cx, cz)| {
            let mut out: Vec<(i32, i32, i32)> = Vec::new();
            let (wx0, wx1) = (cx * CELL_W, cx * CELL_W + CELL_W);
            let (wz0, wz1) = (cz * CELL_W, cz * CELL_W + CELL_W);
            // A cell's TOP corner plane is the next cell's BOTTOM plane: same four
            // coordinates, so the same four values. Carrying it up the column halves the
            // density evaluations - the dominant cost of cave generation - and cannot
            // change a result, because it reuses values instead of recomputing them.
            let mut b00 = gen.combined_density(wx0, cy_lo * CELL_H, wz0);
            let mut b10 = gen.combined_density(wx1, cy_lo * CELL_H, wz0);
            let mut b01 = gen.combined_density(wx0, cy_lo * CELL_H, wz1);
            let mut b11 = gen.combined_density(wx1, cy_lo * CELL_H, wz1);
            for cy in cy_lo..=cy_hi {
                let (wy0, wy1) = (cy * CELL_H, cy * CELL_H + CELL_H);
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
                    let fy = (by - wy0) as f64 / CELL_H as f64;
                    let xz00 = lerp(fy, n000, n010);
                    let xz10 = lerp(fy, n100, n110);
                    let xz01 = lerp(fy, n001, n011);
                    let xz11 = lerp(fy, n101, n111);
                    for bx in wx0.max(min_x)..wx1.min(max_x + 1) {
                        let fx = (bx - wx0) as f64 / CELL_W as f64;
                        let z0v = lerp(fx, xz00, xz10);
                        let z1v = lerp(fx, xz01, xz11);
                        let ix = (bx - min_x) as usize;
                        for bz in wz0.max(min_z)..wz1.min(max_z + 1) {
                            let top = surf[ix * h + (bz - min_z) as usize] - TOP_GATE;
                            if by > top {
                                continue;
                            }
                            let fz = (bz - wz0) as f64 / CELL_W as f64;
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
        .collect()
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

    /// Share of the sampled cave volume that a style gives a theme, upper and deep bands averaged.
    fn themed_share(style: CaveStyle) -> f64 {
        let decor = Decor::new(SEED, BiomeAmounts::uniform(style.amount()));
        let (mut themed, mut total) = (0u32, 0u32);
        for y in [-20, -48] {
            for ix in 0..96 {
                for iz in 0..96 {
                    let (x, z) = (ix * 40 - 1920, iz * 40 - 1920);
                    let hit = decor.zone(x, y, z, 200) != decoration::Zone::Normal
                        || decor.coral_zone(x, z);
                    themed += u32::from(hit);
                    total += 1;
                }
            }
        }
        f64::from(themed) * 100.0 / f64::from(total)
    }

    /// The styles keep the themed shares their amounts were measured at.
    #[test]
    fn style_shares_match() {
        for (style, want) in [
            (CaveStyle::Vanilla, 0.0),
            (CaveStyle::MoreVanilla, 29.5),
            (CaveStyle::AllMix, 77.0),
            (CaveStyle::MoreMix, 88.6),
        ] {
            let got = themed_share(style);
            assert!(
                (got - want).abs() <= 3.0,
                "{style:?}: {got:.1}% themed, want {want}"
            );
        }
    }

    /// `--caves` alone is v3.4.0's caves: default amounts, the built-in seed, floor datum.
    #[test]
    fn no_flags_is_v340_caves() {
        use clap::Parser;
        let args = Args::parse_from(["arnis", "--output-dir", ".", "--bbox", "1,2,3,4", "--caves"]);
        assert_eq!(biome_amounts(&args), BiomeAmounts::default());
        assert_eq!(seed(&args), SEED);
        assert!(args.cave_ores.is_none() && args.cave_datum_y.is_none());
        let own = Args::parse_from([
            "arnis",
            "--output-dir",
            ".",
            "--bbox",
            "1,2,3,4",
            "--caves",
            "--cave-seed",
            "7",
        ]);
        assert_eq!(seed(&own), 7);
        // A --cave-biomes value replaces the style's amount for its theme only.
        let mixed = Args::parse_from([
            "arnis",
            "--bbox",
            "1,2,3,4",
            "--caves",
            "--cave-style",
            "vanilla",
            "--cave-biomes",
            "lush=150",
        ]);
        assert_eq!(
            biome_amounts(&mixed),
            BiomeAmounts {
                lush: 1.5,
                ..BiomeAmounts::uniform(0.0)
            }
        );
    }

    /// Two runs whose bedrock planes differ carve the same field once `--cave-datum-y` pins it.
    #[test]
    fn a_pinned_datum_ignores_the_floor() {
        use crate::world_editor::{set_terrain_floor_y, DEFAULT_MIN_Y};
        let _g = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        set_terrain_floor_y(-64);
        set_datum_y(Some(-64));
        let low = CaveGen::new(SEED);
        set_terrain_floor_y(16);
        let floor = terrain_floor_y();
        let high = CaveGen::new(SEED);
        let pinned = datum_y();
        set_datum_y(None);
        let unpinned = datum_y();
        set_terrain_floor_y(DEFAULT_MIN_Y + 2);
        assert_eq!(pinned, -64);
        assert!(
            unpinned == floor && floor > -64,
            "unpinned, the datum is the floor"
        );
        for (x, y, z) in [(3, -40, 9), (40, 0, -17), (-75, 60, 33)] {
            assert_eq!(
                low.combined_density(x, y, z),
                high.combined_density(x, y, z)
            );
        }
    }

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

    /// Uneven synthetic terrain for the pure planning tests.
    fn rolling_surface(x: i32, z: i32) -> i32 {
        60 + (x * 7 + z * 13).rem_euclid(31)
    }

    fn vanilla_bounds() {
        use crate::world_editor::{
            set_terrain_floor_y, set_world_bounds, DEFAULT_MAX_Y, DEFAULT_MIN_Y,
        };
        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
        set_terrain_floor_y(DEFAULT_MIN_Y + 2);
    }

    /// `CaveShape` must answer exactly what the carve does (noise caves plus carvers), or features
    /// planned against it would not line up with the caves they meet.
    #[test]
    fn the_shape_is_the_carve() {
        let _g = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        vanilla_bounds();
        let region = Rect {
            min_x: -12,
            max_x: 11,
            min_z: 3,
            max_z: 26,
        };
        let gen = CaveGen::new(SEED);
        let shape = CaveShape::new(&gen, SEED, region.grow(300), region, rolling_surface);
        let floor = terrain_floor_y();
        let mut surf = Vec::new();
        for x in region.min_x..=region.max_x {
            for z in region.min_z..=region.max_z {
                surf.push(rolling_surface(x, z));
            }
        }
        let mut carve: std::collections::HashSet<(i32, i32, i32)> =
            noise_cells(&gen, region, &surf, floor)
                .into_iter()
                .collect();
        for (x, y, z) in
            carver::carve_positions(SEED, region.min_x, region.max_x, region.min_z, region.max_z)
        {
            if y > floor && y <= rolling_surface(x, z) - TOP_GATE {
                carve.insert((x, y, z));
            }
        }
        assert!(!carve.is_empty());
        for x in region.min_x..=region.max_x {
            for z in region.min_z..=region.max_z {
                for y in floor..=rolling_surface(x, z) {
                    assert_eq!(
                        shape.is_cave(x, y, z),
                        carve.contains(&(x, y, z)),
                        "({x}, {y}, {z})"
                    );
                }
            }
        }
    }

    /// A pool, river or geode near a tile edge is planned by both tiles. Each must plan exactly
    /// what one pass over the whole world plans for its side, or tiled worlds cut these features
    /// at every seam.
    #[test]
    fn features_do_not_depend_on_the_tile_split() {
        use decoration::GeodeWrite;
        use std::collections::BTreeSet;
        let _g = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        vanilla_bounds();
        let world = Rect {
            min_x: 0,
            max_x: 511,
            min_z: 0,
            max_z: 383,
        };
        let gen = CaveGen::new(SEED);
        let decor = Decor::new(SEED, BiomeAmounts::default());
        let plan_for = |region: Rect| {
            let shape = CaveShape::new(&gen, SEED, world, region, rolling_surface);
            let water = water::plan(&shape, &decor, SEED, region);
            let geodes = decoration::plan_geodes(decor.seed, &shape, &water, region, &|_, _| false);
            (water, geodes)
        };
        let within = |cells: &HashSet<i64>, part: Rect| -> BTreeSet<i64> {
            cells
                .iter()
                .copied()
                .filter(|&p| {
                    let (x, _, z) = unpack(p);
                    part.contains(x, z)
                })
                .collect()
        };
        let geode_writes = |writes: &[GeodeWrite], part: Rect| -> Vec<GeodeWrite> {
            writes
                .iter()
                .copied()
                .filter(|w| match *w {
                    GeodeWrite::Block((x, _, z), _) => part.contains(x, z),
                    GeodeWrite::Cluster {
                        budding: (x, _, z), ..
                    } => part.contains(x, z),
                })
                .collect()
        };

        let (whole_water, whole_geodes) = plan_for(world);
        // Split through a geode and through a pool or river, so the seams really cut something.
        let geode_x = whole_geodes
            .iter()
            .find_map(|w| match *w {
                GeodeWrite::Block((x, _, _), AIR) => Some(x),
                _ => None,
            })
            .expect("the test world holds no geode");
        let mut water_xs: Vec<i32> = whole_water.water.iter().map(|&p| unpack(p).0).collect();
        water_xs.sort_unstable();
        let water_x = *water_xs
            .get(water_xs.len() / 2)
            .expect("the test world holds no water");

        let geode_xs: BTreeSet<i32> = whole_geodes
            .iter()
            .filter_map(|w| match *w {
                GeodeWrite::Block((x, _, _), _) => Some(x),
                _ => None,
            })
            .collect();
        assert!(geode_xs.contains(&(geode_x - 1)) && geode_xs.contains(&geode_x));
        assert!(water_xs.contains(&(water_x - 1)) && water_xs.contains(&water_x));

        for split in [geode_x, water_x] {
            let left = Rect {
                max_x: split - 1,
                ..world
            };
            let right = Rect {
                min_x: split,
                ..world
            };
            for part in [left, right] {
                let (water, geodes) = plan_for(part);
                assert_eq!(
                    within(&water.carved, part),
                    within(&whole_water.carved, part)
                );
                assert_eq!(within(&water.water, part), within(&whole_water.water, part));
                assert_eq!(
                    geode_writes(&geodes, part),
                    geode_writes(&whole_geodes, part)
                );
            }
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
        carve_region(
            &mut editor,
            &args,
            xzbbox,
            0,
            SIDE - 1,
            0,
            SIDE - 1,
            &|_, _| false,
        );
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
