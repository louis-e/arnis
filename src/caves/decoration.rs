//! Vanilla-style cave decoration: eight biome themes (lush, dripstone, deep dark, mushroom, ice,
//! amethyst, volcanic, coral reefs in pools), glow lichen on all cave surfaces, and rare amethyst
//! geodes. Runs as a post-carve pass over the cave-air cells. Biome patches are chosen by
//! low-frequency noise (the full vanilla climate model is not reproduced), blotchy so themed
//! pockets sit in plain rock.

use super::noise::NormalNoise;
use super::rng::XoroRandom;
use super::shape::{CaveShape, Rect};
use super::water::WaterPlan;
use crate::block_definitions::*;
use crate::world_editor::WorldEditor;
use fastnbt::Value;
// FnvHashSet: std seeds its hasher randomly per process, so iterating one to
// apply world edits gives a different write order -- and a different world --
// every run. FNV is deterministic. See the note in caves/mod.rs.
use fnv::{FnvHashMap, FnvHashSet as HashSet};
use std::collections::HashMap;
use std::sync::Arc;

pub(super) const ROCK: &[Block] = &[
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

fn props(pairs: &[(&str, &str)]) -> Option<Arc<Value>> {
    let mut m: HashMap<String, Value> = HashMap::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), Value::String((*v).to_string()));
    }
    Some(crate::block_definitions::intern_props(Value::Compound(m)))
}
fn multiface(face: &str) -> Option<Arc<Value>> {
    let mut m: HashMap<String, Value> = HashMap::new();
    for f in ["down", "up", "north", "south", "east", "west"] {
        m.insert(
            f.to_string(),
            Value::String(if f == face { "true" } else { "false" }.into()),
        );
    }
    Some(crate::block_definitions::intern_props(Value::Compound(m)))
}
/// place into AIR only (won't touch ore/rock/buildings).
fn put(ed: &mut WorldEditor, b: Block, x: i32, y: i32, z: i32, p: Option<Arc<Value>>) {
    ed.set_block_with_properties_absolute(
        crate::block_definitions::BlockWithProperties {
            block: b,
            properties: p,
        },
        x,
        y,
        z,
        Some(&[AIR]),
        None,
    );
}

#[inline]
fn hash(x: i32, y: i32, z: i32, seed: u64) -> u64 {
    let mut h = seed
        ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (z as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h
}
#[inline]
fn chance(x: i32, y: i32, z: i32, seed: u64, pct: u64) -> bool {
    hash(x, y, z, seed) % 1000 < pct
}

/// Cave biome themes. The whole underground is partitioned into big (~50-130 block) blotches per
/// theme; roughly half of all cave columns land in SOME theme, so the cave system reads populated
/// and varied with plain rock as the filler between themed zones.
#[derive(PartialEq, Clone, Copy)]
pub(super) enum Zone {
    Normal,
    Lush,
    Dripstone,
    DeepDark,
    Mushroom,
    Ice,
    Amethyst,
    Volcanic,
}

/// Per-biome area multipliers from `--cave-biomes` (1.0 = the default share, 0.0 = biome off).
/// A multiplier shifts that biome's noise threshold on a log2 curve (see `eff_thr`), so the
/// area grows/shrinks smoothly and stays a pure function of (seed, position) — seam-safety is
/// unaffected because the multipliers are constant for the whole run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BiomeAmounts {
    pub lush: f64,
    pub dripstone: f64,
    pub deepdark: f64,
    pub mushroom: f64,
    pub ice: f64,
    pub amethyst: f64,
    pub volcanic: f64,
    pub coral: f64,
}

impl Default for BiomeAmounts {
    fn default() -> Self {
        BiomeAmounts {
            lush: 1.0,
            dripstone: 1.0,
            deepdark: 1.0,
            mushroom: 1.0,
            ice: 1.0,
            amethyst: 1.0,
            volcanic: 1.0,
            coral: 1.0,
        }
    }
}

impl BiomeAmounts {
    /// Parse `lush=150,deepdark=0,...` (percent 0..=200, clamped; omitted names stay 100).
    ///
    /// Non-finite percents (`nan`, `inf`) are REJECTED rather than clamped: `clamp` passes
    /// NaN straight through, and a NaN amount would disable every threshold comparison.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut a = BiomeAmounts::default();
        for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (name, val) = part
                .split_once('=')
                .ok_or_else(|| format!("'{part}': expected name=percent"))?;
            let pct: f64 = val
                .trim()
                .parse()
                .map_err(|_| format!("'{part}': percent is not a number"))?;
            if !pct.is_finite() {
                return Err(format!(
                    "'{part}': percent must be a finite number (0..=200), got '{}'",
                    val.trim()
                ));
            }
            let f = (pct.clamp(0.0, 200.0)) / 100.0;
            match name.trim().to_ascii_lowercase().as_str() {
                "lush" => a.lush = f,
                "dripstone" | "drip" => a.dripstone = f,
                "deepdark" | "deep_dark" | "sculk" => a.deepdark = f,
                "mushroom" | "shroom" => a.mushroom = f,
                "ice" => a.ice = f,
                "amethyst" | "crystal" => a.amethyst = f,
                "volcanic" | "volcano" => a.volcanic = f,
                "coral" => a.coral = f,
                other => return Err(format!("unknown cave biome '{other}'")),
            }
        }
        Ok(a)
    }
}

/// Effective threshold for a biome: default multiplier returns the base EXACTLY (log2(1)=0),
/// so runs without --cave-biomes are byte-identical. 200% ≈ -0.10 on the threshold (roughly
/// double the area for these blotch fields), 50% ≈ +0.10; 0 disables the biome outright.
#[inline]
fn eff_thr(base: f64, amt: f64) -> Option<f64> {
    if amt <= 0.0 {
        return None;
    }
    Some((base - 0.10 * amt.log2()).max(0.02))
}

pub struct Decor {
    lush: NormalNoise,
    drip: NormalNoise,
    sculk: NormalNoise,
    shroom: NormalNoise,
    ice: NormalNoise,
    crystal: NormalNoise,
    coral: NormalNoise,
    volc: NormalNoise,
    amt: BiomeAmounts,
    /// World Y minus vanilla Y; the depth gates below are vanilla's.
    y_shift: i32,
    pub(super) seed: u64,
}

impl Decor {
    pub(crate) fn new(seed: i64, amt: BiomeAmounts) -> Self {
        let f = XoroRandom::from_seed(seed ^ 0x0DEC_0DEC).fork_positional();
        let mk = |id: &str| {
            let mut r = f.from_hash_of(id);
            NormalNoise::create(&mut r, -7, &[1.0]) // wavelength ~128 → big blotches
        };
        Decor {
            lush: mk("lush_select"),
            drip: mk("dripstone_select"),
            sculk: mk("sculk_select"),
            shroom: mk("shroom_select"),
            ice: mk("ice_select"),
            crystal: mk("crystal_select"),
            coral: mk("coral_select"),
            volc: mk("volcanic_select"),
            amt,
            y_shift: super::y_shift(),
            seed: seed as u64,
        }
    }

    /// Theme at (x,z) with per-depth gating: volcanic only in the deep half, deep dark only deep,
    /// ice only shallow-to-mid AND only under mountain terrain (`surf_y` = surface height at this
    /// column). Every theme claims only where its noise clears threshold + MARGIN — the band
    /// [thr, thr+MARGIN] around every blotch stays plain rock, so two adjacent themes always have a
    /// vanilla buffer strip between them instead of bleeding into each other. Blotch SCALE varies
    /// with depth (deep = bigger, broader zones; shallow = smaller pockets). Thresholds target ~50%
    /// total coverage — half themed, half vanilla-plain.
    pub(super) fn zone(&self, x: i32, y: i32, z: i32, surf_y: i32) -> Zone {
        const MARGIN: f64 = 0.06;
        let (y, surf_y) = (y - self.y_shift, surf_y - self.y_shift);
        // depth-scaled blotch size: deeper caves get broader, more sprawling theme zones.
        let s = if y < -30 { 0.72 } else { 1.15 };
        let at = |n: &NormalNoise| n.get_value(x as f64 * s, 0.0, z as f64 * s);
        let hit = |n: &NormalNoise, base: f64, amt: f64| match eff_thr(base, amt) {
            Some(t) => at(n) > t,
            None => false,
        };
        // Default distribution: roughly half plain vanilla rock; lush/dripstone/deep dark ~10%
        // each; mushroom next (~7%); amethyst rarest (~4%); volcanic pinned to the BOTTOM of the
        // world (pairs with the y<-54 lava sea); ice mountains-only; coral pools-only. The
        // per-biome --cave-biomes multipliers shift each threshold (see eff_thr); the depth and
        // terrain gates below always apply regardless of the multipliers.
        if y <= -40 && hit(&self.volc, 0.18 + MARGIN, self.amt.volcanic) {
            return Zone::Volcanic;
        }
        // deep dark is a DEEP-ONLY theme: only below the deepslate line (bottom of the world),
        // never in the shallow/mid caves — matches vanilla, where sculk belongs to the depths.
        if y <= -35 && hit(&self.sculk, 0.17 + MARGIN, self.amt.deepdark) {
            return Zone::DeepDark;
        }
        // ice caves only under real mountains (surface ≥115, 177 blocks above the default base).
        if y >= -36 && surf_y >= 115 && hit(&self.ice, 0.20 + MARGIN, self.amt.ice) {
            return Zone::Ice;
        }
        if hit(&self.lush, 0.17 + MARGIN, self.amt.lush) {
            return Zone::Lush;
        }
        if hit(&self.drip, 0.18 + MARGIN, self.amt.dripstone) {
            return Zone::Dripstone;
        }
        if hit(&self.shroom, 0.21 + MARGIN, self.amt.mushroom) {
            return Zone::Mushroom;
        }
        if hit(&self.crystal, 0.25 + MARGIN, self.amt.amethyst) {
            return Zone::Amethyst;
        }
        Zone::Normal
    }

    /// Coral is a WATER theme: it doesn't claim floor space in the chain above, it converts water
    /// POOLS that land inside a coral blotch into flooded reefs.
    pub(super) fn coral_zone(&self, x: i32, z: i32) -> bool {
        match eff_thr(0.34, self.amt.coral) {
            Some(t) => self.coral.get_value(x as f64, 0.0, z as f64) > t,
            None => false,
        }
    }
}

/// Decorate all cave-air cells in the region. `air` is the carved-air set (packed via super::pack).
/// `water_cells` is the PURE pool/river water set — coral reef decoration converts pool floors that
/// land inside a coral blotch. Nothing is written outside `region`: a block placed in a tile's halo
/// would land in the neighbouring tile's caves when the tiles merge.
#[allow(clippy::too_many_arguments)]
pub(super) fn decorate(
    ed: &mut WorldEditor,
    d: &Decor,
    air: &HashSet<i64>,
    basin_fluid: &HashSet<i64>,
    water_cells: &HashSet<i64>,
    shape: &CaveShape,
    plan: &WaterPlan,
    region: Rect,
) {
    let seed = d.seed;

    // iterate cave-air cells; classify floor / ceiling / wall by solid neighbors.
    let cells: Vec<(i32, i32, i32)> = air
        .iter()
        .map(|&p| super::unpack(p))
        .filter(|&(x, _, z)| region.contains(x, z))
        .collect();

    for (x, y, z) in cells {
        let solid = |ed: &WorldEditor, bx, by, bz| {
            ed.check_for_block_absolute(bx, by, bz, Some(ROCK), None)
        };
        let below = solid(ed, x, y - 1, z);
        let above = solid(ed, x, y + 1, z);
        let biome = d.zone(x, y, z, ed.get_ground_level(x, z));

        // ---- glow lichen: sparse on ANY exposed rock face (all biomes) ----
        if chance(x, y, z, seed ^ 0x11C4, 22) {
            // 2.2%
            if below {
                put(ed, GLOW_LICHEN, x, y, z, multiface("down"));
            } else if above {
                put(ed, GLOW_LICHEN, x, y, z, multiface("up"));
            } else if solid(ed, x + 1, y, z) {
                put(ed, GLOW_LICHEN, x, y, z, multiface("east"));
            } else if solid(ed, x - 1, y, z) {
                put(ed, GLOW_LICHEN, x, y, z, multiface("west"));
            } else if solid(ed, x, y, z + 1) {
                put(ed, GLOW_LICHEN, x, y, z, multiface("south"));
            } else if solid(ed, x, y, z - 1) {
                put(ed, GLOW_LICHEN, x, y, z, multiface("north"));
            }
            continue;
        }

        match biome {
            Zone::Lush => {
                if below {
                    // MUD banks where the lush floor touches pool/river water (1.19 block — wet
                    // lush shorelines read like the vanilla mangrove/lush transitions).
                    let wet = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dz)| {
                        basin_fluid.contains(&super::pack(x + dx, y, z + dz))
                            || basin_fluid.contains(&super::pack(x + dx, y - 1, z + dz))
                    });
                    if wet && chance(x, 0, z, seed ^ 0x4D06, 600) {
                        ed.set_block_absolute(MUD, x, y - 1, z, Some(ROCK), None);
                    } else
                    // moss floor blotch (dense — a lush zone should read fully overgrown)
                    if chance(x, 0, z, seed ^ 0x4D05, 780) {
                        ed.set_block_absolute(MOSS_BLOCK, x, y - 1, z, Some(ROCK), None);
                        // vegetation_chance 0.8: grass/carpet/azalea palette
                        if chance(x, y, z, seed ^ 0x7EE5, 800) {
                            let h = hash(x, y, z, seed ^ 0xA21E) % 96;
                            if h < 50 {
                                put(ed, GRASS, x, y, z, None);
                            } else if h < 75 {
                                put(ed, MOSS_CARPET, x, y, z, None);
                            } else if h < 85 {
                                put(ed, TALL_GRASS_BOTTOM, x, y, z, props(&[("half", "lower")]));
                                put(ed, TALL_GRASS_TOP, x, y + 1, z, props(&[("half", "upper")]));
                            } else if h < 92 {
                                put(ed, AZALEA, x, y, z, None);
                            } else {
                                put(ed, FLOWERING_AZALEA, x, y, z, None);
                            }
                        }
                    } else if rare_chance(x, y, z, seed ^ 0xC1A1, 700) {
                        // CLAY POOL — a real multi-block feature, not a random water dot: a 3x3
                        // water bed on a clay-lined floor with a raised clay lip, placed only on
                        // FLAT solid floor (all 25 cells of the 5x5 footprint must be true floor).
                        // Contained by construction: every water cell's sides are lip clay or more
                        // water, its bed is clay — nothing can spread. Single-cell water "dots" on
                        // cave floors read as bugs, so water only ever appears as features.
                        place_clay_pool(ed, air, x, y, z, seed);
                    }
                } else if above {
                    // ceiling: moss + hanging cave vines (glow berries), rare spore blossom
                    if chance(x, 0, z, seed ^ 0x4D05, 650) {
                        ed.set_block_absolute(MOSS_BLOCK, x, y + 1, z, Some(ROCK), None);
                    }
                    if chance(x, y, z, seed ^ 0x617E, 190) {
                        hang_cave_vines(ed, air, x, y, z, seed);
                    } else if chance(x, y, z, seed ^ 0x5B07, 25) {
                        put(ed, SPORE_BLOSSOM, x, y, z, None);
                    }
                }
            }
            Zone::Dripstone => {
                // the cave is MADE of dripstone: convert floor + ceiling surface rock to dripstone_block
                // in dense blotches, so it reads as a dripstone cave (not just a few spikes).
                if below && chance(x, 0, z, seed ^ 0xDB10, 680) {
                    ed.set_block_absolute(DRIPSTONE_BLOCK, x, y - 1, z, Some(ROCK), None);
                }
                if above && chance(x, 0, z, seed ^ 0xDB11, 680) {
                    ed.set_block_absolute(DRIPSTONE_BLOCK, x, y + 1, z, Some(ROCK), None);
                }
                // pointed dripstone: stalactites from the ceiling, stalagmites from the floor
                if above && chance(x, y, z, seed ^ 0xD21A, 170) {
                    grow_dripstone(ed, air, x, y, z, seed, false);
                } else if below && chance(x, y, z, seed ^ 0xD21B, 130) {
                    grow_dripstone(ed, air, x, y, z, seed, true);
                }
            }
            Zone::DeepDark => {
                // sculk floor blanket, sculk veins climbing the walls, rare sensor/shrieker/catalyst
                // point features on flat sculk — the vanilla "deep dark" look.
                if below {
                    if chance(x, 0, z, seed ^ 0x5CA1, 720) {
                        ed.set_block_absolute(SCULK, x, y - 1, z, Some(ROCK), None);
                        let hf = hash(x, y, z, seed ^ 0x5CB2) % 1000;
                        if hf < 12 {
                            put(ed, SCULK_SENSOR, x, y, z, None);
                        } else if hf < 17 {
                            put(ed, SCULK_SHRIEKER, x, y, z, None);
                        } else if hf < 22 {
                            ed.set_block_absolute(
                                SCULK_CATALYST,
                                x,
                                y - 1,
                                z,
                                Some(&[SCULK]),
                                None,
                            );
                        }
                    }
                } else if above {
                    if chance(x, 0, z, seed ^ 0x5CA2, 380) {
                        ed.set_block_absolute(SCULK, x, y + 1, z, Some(ROCK), None);
                    }
                } else if chance(x, y, z, seed ^ 0x5CA3, 50) {
                    // sculk veins on exposed wall faces (multiface, like glow lichen)
                    if solid(ed, x + 1, y, z) {
                        put(ed, SCULK_VEIN, x, y, z, multiface("east"));
                    } else if solid(ed, x - 1, y, z) {
                        put(ed, SCULK_VEIN, x, y, z, multiface("west"));
                    } else if solid(ed, x, y, z + 1) {
                        put(ed, SCULK_VEIN, x, y, z, multiface("south"));
                    } else if solid(ed, x, y, z - 1) {
                        put(ed, SCULK_VEIN, x, y, z, multiface("north"));
                    }
                }
            }
            Zone::Mushroom => {
                // mycelium floors, mushroom scatter, rare GIANT mushrooms (procedural stem + cap).
                if below {
                    if chance(x, 0, z, seed ^ 0x54AD, 750) {
                        ed.set_block_absolute(MYCELIUM, x, y - 1, z, Some(ROCK), None);
                        let hf = hash(x, y, z, seed ^ 0x54AE) % 1000;
                        if hf < 30 {
                            put(ed, RED_MUSHROOM, x, y, z, None);
                        } else if hf < 65 {
                            put(ed, BROWN_MUSHROOM, x, y, z, None);
                        }
                    }
                    if rare_chance(x, y, z, seed ^ 0x54AF, 240) {
                        build_giant_mushroom(ed, air, region, x, y, z, seed);
                    }
                } else if above && chance(x, y, z, seed ^ 0x54B0, 35) {
                    // sparse hanging vines give the fungal grotto some drape
                    hang_cave_vines(ed, air, x, y, z, seed);
                }
            }
            Zone::Ice => {
                // glacial palette: ice / packed ice / blue ice floors + ceilings, snow dusting.
                if below {
                    if chance(x, 0, z, seed ^ 0x1CE1, 700) {
                        let hf = hash(x, 0, z, seed ^ 0x1CE2) % 10;
                        let b = if hf < 6 {
                            ICE
                        } else if hf < 9 {
                            PACKED_ICE
                        } else {
                            BLUE_ICE
                        };
                        ed.set_block_absolute(b, x, y - 1, z, Some(ROCK), None);
                        if chance(x, y, z, seed ^ 0x1CE3, 120) {
                            put(ed, SNOW_LAYER, x, y, z, None);
                        }
                    }
                } else if above && chance(x, 0, z, seed ^ 0x1CE4, 620) {
                    let hf = hash(x, 1, z, seed ^ 0x1CE2) % 10;
                    let b = if hf < 6 {
                        ICE
                    } else if hf < 9 {
                        PACKED_ICE
                    } else {
                        BLUE_ICE
                    };
                    ed.set_block_absolute(b, x, y + 1, z, Some(ROCK), None);
                }
            }
            Zone::Amethyst => {
                // crystal cavern: amethyst/budding walls with calcite + smooth basalt veining, buds
                // growing off floors and ceilings (the geode look, opened into a whole zone).
                // Conversions require 2-thick rock backing: a free-floating 1-thick shelf recolored
                // to amethyst reads as a floating crystal, so those stay plain.
                if below {
                    if solid(ed, x, y - 2, z) && chance(x, 0, z, seed ^ 0xA3E1, 620) {
                        let hf = hash(x, 0, z, seed ^ 0xA3E2) % 100;
                        let b = if hf < 52 {
                            AMETHYST_BLOCK
                        } else if hf < 68 {
                            BUDDING_AMETHYST
                        } else if hf < 86 {
                            CALCITE
                        } else {
                            SMOOTH_BASALT
                        };
                        ed.set_block_absolute(b, x, y - 1, z, Some(ROCK), None);
                        if chance(x, y, z, seed ^ 0xA3E3, 85) {
                            let hb = hash(x, y, z, seed ^ 0xA3E4) % 100;
                            let bud = if hb < 35 {
                                SMALL_AMETHYST_BUD
                            } else if hb < 60 {
                                MEDIUM_AMETHYST_BUD
                            } else if hb < 82 {
                                LARGE_AMETHYST_BUD
                            } else {
                                AMETHYST_CLUSTER
                            };
                            put(ed, bud, x, y, z, props(&[("facing", "up")]));
                        }
                    }
                } else if above && solid(ed, x, y + 2, z) && chance(x, 0, z, seed ^ 0xA3E5, 560) {
                    let hf = hash(x, 1, z, seed ^ 0xA3E2) % 100;
                    let b = if hf < 52 {
                        AMETHYST_BLOCK
                    } else if hf < 68 {
                        BUDDING_AMETHYST
                    } else if hf < 86 {
                        CALCITE
                    } else {
                        SMOOTH_BASALT
                    };
                    ed.set_block_absolute(b, x, y + 1, z, Some(ROCK), None);
                    if chance(x, y, z, seed ^ 0xA3E6, 65) {
                        let hb = hash(x, y, z, seed ^ 0xA3E4) % 100;
                        let bud = if hb < 40 {
                            SMALL_AMETHYST_BUD
                        } else if hb < 70 {
                            MEDIUM_AMETHYST_BUD
                        } else {
                            AMETHYST_CLUSTER
                        };
                        put(ed, bud, x, y, z, props(&[("facing", "down")]));
                    }
                }
            }
            Zone::Volcanic => {
                // basalt cavern: basalt/smooth basalt/blackstone/tuff with glowing magma clusters,
                // floor-to-ceiling basalt COLUMNS in the big rooms, and rare inset lava ponds —
                // pairs with the deep lava sea (obsidian rims painted by the lava pass in mod.rs).
                if below {
                    if solid(ed, x, y - 2, z) && chance(x, 0, z, seed ^ 0xBA5A, 720) {
                        let hf = hash(x, 0, z, seed ^ 0xBA5B) % 100;
                        let b = if hf < 40 {
                            BASALT
                        } else if hf < 64 {
                            SMOOTH_BASALT
                        } else if hf < 78 {
                            BLACKSTONE
                        } else if hf < 90 {
                            POLISHED_BLACKSTONE
                        } else {
                            MAGMA_BLOCK
                        };
                        ed.set_block_absolute(b, x, y - 1, z, Some(ROCK), None);
                    }
                    // basalt pillar: rises through the room, reads best in the big open caverns
                    // (put() is AIR-only so it clips naturally in low passages).
                    if rare_chance(x, y, z, seed ^ 0xBA5D, 170) {
                        for i in 0..14 {
                            if !is_air(air, ed, x, y + i, z) {
                                break;
                            }
                            put(ed, BASALT, x, y + i, z, props(&[("axis", "y")]));
                        }
                    } else if rare_chance(x, y, z, seed ^ 0xBA5E, 520) {
                        // inset lava pond: 1 block INTO the floor (supported, contained), only when
                        // no water is anywhere near — same zero-contact rule as everything else.
                        let mut water_near = false;
                        'scan: for dx in -3..=3i32 {
                            for dy in -2..=2i32 {
                                for dz in -3..=3i32 {
                                    if ed.check_for_block_absolute(
                                        x + dx,
                                        y + dy,
                                        z + dz,
                                        Some(&[WATER]),
                                        None,
                                    ) {
                                        water_near = true;
                                        break 'scan;
                                    }
                                }
                            }
                        }
                        let contained = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                            .iter()
                            .all(|&(dx, dz)| solid(ed, x + dx, y - 1, z + dz))
                            && solid(ed, x, y - 2, z);
                        if !water_near && contained {
                            ed.set_block_absolute(LAVA, x, y - 1, z, Some(ROCK), None);
                            ed.schedule_fluid_tick(LAVA, x, y - 1, z);
                        }
                    }
                } else if above && solid(ed, x, y + 2, z) && chance(x, 0, z, seed ^ 0xBA5C, 550) {
                    let hf = hash(x, 1, z, seed ^ 0xBA5B) % 100;
                    let b = if hf < 50 {
                        BASALT
                    } else if hf < 85 {
                        SMOOTH_BASALT
                    } else {
                        POLISHED_BLACKSTONE
                    };
                    ed.set_block_absolute(b, x, y + 1, z, Some(ROCK), None);
                }
            }
            Zone::Normal => {}
        }
    }

    // CORAL REEFS in water pools: a pool whose floor lands in a coral blotch becomes a flooded
    // reef — dead-coral/sand bed, live coral blocks + fans (default-waterlogged), sea pickles,
    // seagrass. Writes into WATER cells only (whitelist), so dry caves are never touched.
    for &p in water_cells {
        let (x, y, z) = super::unpack(p);
        if !region.contains(x, z) {
            continue;
        }
        if !d.coral_zone(x, z) {
            continue;
        }
        let floor = ed.check_for_block_absolute(x, y - 1, z, Some(ROCK), None);
        if floor {
            // reef bed replaces the pool's rock floor
            let hf = hash(x, 0, z, seed ^ 0xC0A1) % 100;
            let bed = match hf {
                0..=17 => DEAD_TUBE_CORAL_BLOCK,
                18..=35 => DEAD_BRAIN_CORAL_BLOCK,
                36..=48 => DEAD_FIRE_CORAL_BLOCK,
                49..=61 => DEAD_HORN_CORAL_BLOCK,
                62..=74 => DEAD_BUBBLE_CORAL_BLOCK,
                75..=88 => SAND,
                _ => GRAVEL,
            };
            ed.set_block_absolute(bed, x, y - 1, z, Some(ROCK), None);
            // live growth INSIDE the water column just above the bed
            let hg = hash(x, y, z, seed ^ 0xC0A2) % 1000;
            let put_w = |ed: &mut WorldEditor, b: Block| {
                ed.set_block_with_properties_absolute(
                    BlockWithProperties {
                        block: b,
                        properties: None,
                    },
                    x,
                    y,
                    z,
                    Some(&[WATER]),
                    None,
                );
            };
            if hg < 90 {
                let hb = hash(x, y, z, seed ^ 0xC0A3) % 5;
                let cb = [
                    TUBE_CORAL_BLOCK,
                    BRAIN_CORAL_BLOCK,
                    FIRE_CORAL_BLOCK,
                    HORN_CORAL_BLOCK,
                    BUBBLE_CORAL_BLOCK,
                ][hb as usize];
                put_w(ed, cb);
            } else if hg < 210 {
                let hb = hash(x, y, z, seed ^ 0xC0A4) % 5;
                let cf = [
                    TUBE_CORAL_FAN,
                    BRAIN_CORAL_FAN,
                    FIRE_CORAL_FAN,
                    HORN_CORAL_FAN,
                    BUBBLE_CORAL_FAN,
                ][hb as usize];
                put_w(ed, cf);
            } else if hg < 300 {
                let hb = hash(x, y, z, seed ^ 0xC0A5) % 5;
                let cc = [
                    TUBE_CORAL,
                    BRAIN_CORAL,
                    FIRE_CORAL,
                    HORN_CORAL,
                    BUBBLE_CORAL,
                ][hb as usize];
                put_w(ed, cc);
            } else if hg < 360 {
                put_w(ed, SEA_PICKLE);
            } else if hg < 450 {
                put_w(ed, SEAGRASS);
            }
        }
    }

    // amethyst geodes — rare, per ~chunk, embedded in rock (independent of caves).
    place_geodes(ed, seed, shape, plan, region);

    // NO springs/drips of any kind: water exists ONLY as rivers and pool caves. Isolated
    // single-source trickles invariably read as bugs — do not reintroduce them.
    let _ = basin_fluid;
}

#[inline]
fn is_air(air: &HashSet<i64>, ed: &WorldEditor, x: i32, y: i32, z: i32) -> bool {
    air.contains(&super::pack(x, y, z)) || !ed.block_exists_absolute(x, y, z)
}

/// hang a cave-vine column from a ceiling cell (length ~1..8, 20% berries per segment).
fn hang_cave_vines(ed: &mut WorldEditor, air: &HashSet<i64>, x: i32, y: i32, z: i32, seed: u64) {
    let len = 1 + (hash(x, y, z, seed ^ 0x617E) % 8) as i32; // 1..8
    for i in 0..len {
        let yy = y - i;
        if !is_air(air, ed, x, yy, z) {
            break;
        }
        let berries = hash(x, yy, z, seed ^ 0xBEEF).is_multiple_of(5); // 20%
        let b = if berries { "true" } else { "false" };
        if i == len - 1 {
            put(
                ed,
                CAVE_VINES,
                x,
                yy,
                z,
                props(&[("berries", b), ("age", "25")]),
            );
        } else {
            put(ed, CAVE_VINES_PLANT, x, yy, z, props(&[("berries", b)]));
        }
    }
}

/// CLAY POOL: 3x3 still-water bed sunk flush with the floor, clay-lined underneath, clay ring
/// around it. Placed only where the whole 5x5 footprint is flat true floor (air at pool level with
/// ROCK directly below every cell) so it completes into its ground — no lips hanging over slopes,
/// no open side to leak from. Water is NOT ticked: a fully-enclosed still pool needs no physics.
fn place_clay_pool(ed: &mut WorldEditor, air: &HashSet<i64>, x: i32, y: i32, z: i32, seed: u64) {
    // footprint check: 5x5, every cell air-at-y with rock below (flat floor)
    for dx in -2i32..=2 {
        for dz in -2i32..=2 {
            if !air.contains(&super::pack(x + dx, y, z + dz))
                || !ed.check_for_block_absolute(x + dx, y - 1, z + dz, Some(ROCK), None)
            {
                return;
            }
        }
    }
    let _ = seed;
    for dx in -2i32..=2 {
        for dz in -2i32..=2 {
            let ring = dx.abs() == 2 || dz.abs() == 2;
            if ring {
                // clay ring flush with the floor surface
                ed.set_block_absolute(CLAY, x + dx, y - 1, z + dz, Some(ROCK), None);
            } else {
                // pool: water sunk INTO the floor (replaces the floor block), clay bed below
                ed.set_block_absolute(WATER, x + dx, y - 1, z + dz, Some(ROCK), None);
                ed.set_block_absolute(CLAY, x + dx, y - 2, z + dz, Some(ROCK), None);
            }
        }
    }
}

/// procedural giant mushroom: mushroom_stem column + cap. Red = domed cap (3x3 crown over a 5x5
/// ring), brown = flat 5x5 plate (corners cut). All cap/stem writes are AIR-only (`put`) so the
/// mushroom clips cleanly against walls/ceilings instead of cutting rock; requires headroom checked
/// by the caller's air test per segment.
fn build_giant_mushroom(
    ed: &mut WorldEditor,
    air: &HashSet<i64>,
    region: Rect,
    x: i32,
    y: i32,
    z: i32,
    seed: u64,
) {
    // The cap reaches two blocks out, past the region's edge near a tile seam.
    let cap = |ed: &mut WorldEditor, b: Block, bx: i32, by: i32, bz: i32| {
        if region.contains(bx, bz) {
            put(ed, b, bx, by, bz, None);
        }
    };
    let stem_h = 3 + (hash(x, y, z, seed ^ 0x9145) % 4) as i32; // 3..6
                                                                // need at least stem + 1 cap row of air or it looks squashed
    for i in 0..=stem_h {
        if !is_air(air, ed, x, y + i, z) {
            return;
        }
    }
    let red = hash(x, y, z, seed ^ 0x9146).is_multiple_of(2);
    for i in 0..stem_h {
        put(ed, MUSHROOM_STEM, x, y + i, z, None);
    }
    let cap_y = y + stem_h;
    if red {
        // dome: 3x3 crown on top, 5x5 ring (minus corners) one below the crown top
        for dx in -1..=1 {
            for dz in -1..=1 {
                cap(ed, RED_MUSHROOM_BLOCK, x + dx, cap_y, z + dz);
            }
        }
        for dx in -2i32..=2 {
            for dz in -2i32..=2 {
                let rim = dx.abs() == 2 || dz.abs() == 2;
                if rim && !(dx.abs() == 2 && dz.abs() == 2) {
                    cap(ed, RED_MUSHROOM_BLOCK, x + dx, cap_y - 1, z + dz);
                }
            }
        }
    } else {
        // brown: flat 5x5 plate, corners cut; occasional shroomlight tucked under the center
        for dx in -2i32..=2 {
            for dz in -2i32..=2 {
                if dx.abs() == 2 && dz.abs() == 2 {
                    continue;
                }
                cap(ed, BROWN_MUSHROOM_BLOCK, x + dx, cap_y, z + dz);
            }
        }
        if hash(x, cap_y, z, seed ^ 0x9147).is_multiple_of(4) {
            put(ed, SHROOMLIGHT, x, cap_y - 1, z, None);
        }
    }
}

/// grow a tapered pointed-dripstone column (down=stalactite from ceiling, up=stalagmite from floor).
fn grow_dripstone(
    ed: &mut WorldEditor,
    air: &HashSet<i64>,
    x: i32,
    y: i32,
    z: i32,
    seed: u64,
    up: bool,
) {
    let len = 1 + (hash(x, y, z, seed ^ 0xD21A) % 5) as i32; // 1..5
    let dir = if up { "up" } else { "down" };
    let step = if up { 1 } else { -1 };
    for i in 0..len {
        let yy = y + step * i;
        if !is_air(air, ed, x, yy, z) {
            break;
        }
        let thickness = if len == 1 {
            "tip"
        } else if i == 0 {
            "base"
        } else if i == len - 1 {
            "tip"
        } else if i == 1 {
            "frustum"
        } else {
            "middle"
        };
        put(
            ed,
            POINTED_DRIPSTONE,
            x,
            yy,
            z,
            props(&[
                ("thickness", thickness),
                ("vertical_direction", dir),
                ("waterlogged", "false"),
            ]),
        );
    }
}

/// amethyst geode: layered sphere — smooth_basalt outer shell, calcite, amethyst_block inner shell
/// (with budding_amethyst), hollow center lined with amethyst_cluster crystals pointing inward.
/// Rare (~1/80 chunks) and ONLY placed with its center in solid rock (never floating/exposed; clears
/// any ore in its volume so nothing shows inside). Where a PRE-EXISTING cave passage crosses the
/// shell, that cell is left as cave air instead of being walled off — so a tunnel that grazes a
/// geode shows a real cut cross-section into it, rather than sealing the passage shut.
///
/// Planned against [`CaveShape`] and the water plan, for every geode that can reach the region, so a
/// geode on a tile seam comes out whole: each tile writes its own part of the same sphere.
fn place_geodes(
    ed: &mut WorldEditor,
    seed: u64,
    shape: &CaveShape,
    plan: &WaterPlan,
    region: Rect,
) {
    for write in plan_geodes(seed, shape, plan, region) {
        match write {
            GeodeWrite::Block((x, y, z), block) => geode_set(ed, block, x, y, z),
            GeodeWrite::Cluster { budding, center } => {
                grow_cluster(ed, region, center, budding, seed)
            }
        }
    }
}

/// One step of laying out the geodes, in the order they apply.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum GeodeWrite {
    /// Set a block inside the region, whatever was there (bedrock aside).
    Block((i32, i32, i32), Block),
    /// Try to grow a crystal off a budding block toward its geode's center.
    Cluster {
        budding: (i32, i32, i32),
        center: (i32, i32, i32),
    },
}

/// The geodes that reach `region`, as the writes that lay them out there.
pub(super) fn plan_geodes(
    seed: u64,
    shape: &CaveShape,
    plan: &WaterPlan,
    region: Rect,
) -> Vec<GeodeWrite> {
    let mut writes: Vec<GeodeWrite> = Vec::new();
    let r_inner = 4.2_f64;
    let r_amethyst = 5.0_f64;
    let r_calcite = 6.0_f64;
    let r_basalt = 6.7_f64;
    let ri = r_basalt.ceil() as i32;
    let world = shape.world();
    // What earlier geodes of this pass left at a block: true for shell, false for air.
    let mut overlay: FnvHashMap<i64, bool> = FnvHashMap::default();
    // A block the carve or a water feature opened (fluids included) — the geode leaves it open.
    let opened = |overlay: &FnvHashMap<i64, bool>, x: i32, y: i32, z: i32| match overlay
        .get(&super::pack(x, y, z))
    {
        Some(&shell) => !shell,
        None => shape.is_cave(x, y, z) || plan.carved.contains(&super::pack(x, y, z)),
    };
    // A block standing there, rock or fluid.
    let standing = |overlay: &FnvHashMap<i64, bool>, x: i32, y: i32, z: i32| match overlay
        .get(&super::pack(x, y, z))
    {
        Some(&shell) => shell,
        None => plan.solid(shape, x, y, z),
    };

    // A geode reaches `ri` blocks from its center, and its support sweep looks one further.
    let (cx0, cx1, cz0, cz1) = region.grow(ri + 1).clip(world).chunks();
    for cx in cx0..=cx1 {
        for cz in cz0..=cz1 {
            if !hash(cx, 0, cz, seed ^ 0x006E_0DE5).is_multiple_of(80) {
                continue;
            }
            let gx = cx * 16 + (hash(cx, 1, cz, seed) % 16) as i32;
            let gz = cz * 16 + (hash(cx, 2, cz, seed) % 16) as i32;
            let gy = super::vy(-54) + (hash(cx, 3, cz, seed) % 36) as i32; // vanilla y ~ -54..-18
            if !world.contains(gx, gz) {
                continue;
            }
            // must be FULLY BELOW the surface (so it never floats in the sky / pokes out of the ground)
            // and its center must sit in solid rock (not mid-cave). Intersecting a cave is fine —
            // vanilla geodes do too. The center test ignores other geodes, so whether a geode exists
            // never depends on one a tile may not have planned.
            if gy + ri + 2 >= shape.surf(gx, gz) || !plan.solid(shape, gx, gy, gz) {
                continue;
            }

            // pass 1: lay out the layered sphere (overwrite rock/ore/fluid — protect only bedrock);
            //         hollow center carved to air. SHELL cells (outside r_inner) that were ALREADY
            //         open are left untouched (skipped) so a pre-existing passage stays open.
            //         Collect budding cells for the crystal pass.
            let mut hollow: HashSet<(i32, i32, i32)> = HashSet::default();
            let mut shell: Vec<((i32, i32, i32), Block)> = Vec::new();
            let mut written_shell: HashSet<(i32, i32, i32)> = HashSet::default();
            let mut buddings: Vec<(i32, i32, i32)> = Vec::new();
            for dx in -ri..=ri {
                for dy in -ri..=ri {
                    for dz in -ri..=ri {
                        let dist = ((dx * dx + dy * dy + dz * dz) as f64).sqrt();
                        if dist > r_basalt {
                            continue;
                        }
                        let (px, py, pz) = (gx + dx, gy + dy, gz + dz);
                        let block = if dist <= r_inner {
                            hollow.insert((px, py, pz));
                            continue;
                        } else if opened(&overlay, px, py, pz) {
                            continue; // pre-existing cave passage crossing the shell — leave it open
                        } else if dist <= r_amethyst {
                            let facing_hollow = dist <= r_inner + 1.0;
                            if facing_hollow && hash(px, py, pz, seed ^ 0xB0DD) % 100 < 30 {
                                buddings.push((px, py, pz));
                                BUDDING_AMETHYST
                            } else {
                                AMETHYST_BLOCK
                            }
                        } else if dist <= r_calcite {
                            CALCITE
                        } else {
                            SMOOTH_BASALT
                        };
                        shell.push(((px, py, pz), block));
                        written_shell.insert((px, py, pz));
                    }
                }
            }
            // pass 1.5: SUPPORT SWEEP — a tunnel grazing the shell at a shallow angle only clips PART
            // of a thin shell ring (basalt/calcite/amethyst are ~0.7-1.0 block thick radially, thinner
            // than a typical 2-4-block tunnel), which can leave 1-2-block floating fragments where the
            // skip above removed their only neighbors. Revert any shell cell with ZERO real
            // 6-connected support (neither standing host rock outside the geode nor another shell
            // cell) back to air — same technique as the despeckle pass in mod.rs.
            let reverted: HashSet<(i32, i32, i32)> = written_shell
                .iter()
                .copied()
                .filter(|&(px, py, pz)| {
                    !super::neighbours(px, py, pz).iter().any(|&n| {
                        written_shell.contains(&n)
                            || (!hollow.contains(&n) && standing(&overlay, n.0, n.1, n.2))
                    })
                })
                .collect();

            // The hollow is sorted so the writes do not depend on the set's iteration order.
            let mut hollow: Vec<(i32, i32, i32)> = hollow.into_iter().collect();
            hollow.sort_unstable();
            for &(px, py, pz) in &hollow {
                overlay.insert(super::pack(px, py, pz), false);
                if region.contains(px, pz) {
                    writes.push(GeodeWrite::Block((px, py, pz), AIR)); // hollow (clears ore/rock)
                }
            }
            for &((px, py, pz), block) in &shell {
                let kept = !reverted.contains(&(px, py, pz));
                overlay.insert(super::pack(px, py, pz), kept);
                if region.contains(px, pz) {
                    writes.push(GeodeWrite::Block(
                        (px, py, pz),
                        if kept { block } else { AIR },
                    ));
                }
            }
            // pass 2: grow amethyst clusters off the SURVIVING budding blocks into the hollow.
            for budding in buddings {
                if !reverted.contains(&budding) {
                    writes.push(GeodeWrite::Cluster {
                        budding,
                        center: (gx, gy, gz),
                    });
                }
            }
        }
    }
    writes
}

/// overwrite any block (except bedrock) — used for the geode shells/hollow so they carve into solid
/// rock (the None/None path only fills air).
fn geode_set(ed: &mut WorldEditor, b: Block, x: i32, y: i32, z: i32) {
    ed.set_block_absolute(b, x, y, z, None, Some(&[BEDROCK]));
}

/// exact 1-in-`denom` rarity (unlike `chance`, which is ‰-resolution and can't express anything
/// rarer than 1-in-1000). Used for the rarest feature rolls.
#[inline]
fn rare_chance(x: i32, y: i32, z: i32, seed: u64, denom: u64) -> bool {
    hash(x, y, z, seed).is_multiple_of(denom)
}

/// place an amethyst_cluster in the hollow cell adjacent to a budding block, facing toward the geode
/// center (so it grows off the wall into the hollow, like vanilla).
fn grow_cluster(
    ed: &mut WorldEditor,
    region: Rect,
    (gx, gy, gz): (i32, i32, i32),
    (bx, by, bz): (i32, i32, i32),
    seed: u64,
) {
    if hash(bx, by, bz, seed ^ 0xC1A5) % 100 >= 70 {
        return; // not every budding block has a cluster
    }
    let (dx, dy, dz) = (gx - bx, gy - by, gz - bz); // toward center
                                                    // dominant axis → the face the cluster points
    let (face, nx, ny, nz) = if dx.abs() >= dy.abs() && dx.abs() >= dz.abs() {
        (
            if dx > 0 { "east" } else { "west" },
            bx + dx.signum(),
            by,
            bz,
        )
    } else if dy.abs() >= dz.abs() {
        (if dy > 0 { "up" } else { "down" }, bx, by + dy.signum(), bz)
    } else {
        (
            if dz > 0 { "south" } else { "north" },
            bx,
            by,
            bz + dz.signum(),
        )
    };
    if !region.contains(nx, nz) {
        return;
    }
    put(
        ed,
        AMETHYST_CLUSTER,
        nx,
        ny,
        nz,
        props(&[("facing", face), ("waterlogged", "false")]),
    );
}

#[cfg(test)]
mod tests {
    use super::BiomeAmounts;

    #[test]
    fn parse_clamps_and_scales_percent() {
        let a = BiomeAmounts::parse("lush=150,deepdark=0,ice=500").unwrap();
        assert_eq!(a.lush, 1.5);
        assert_eq!(a.deepdark, 0.0);
        assert_eq!(a.ice, 2.0); // clamped to 200%
        assert_eq!(a.coral, 1.0); // untouched name keeps the default
    }

    /// `clamp` lets NaN through, and a NaN amount would silently disable its biome. Reject early.
    #[test]
    fn parse_rejects_non_finite_percent() {
        for spec in ["lush=nan", "lush=NaN", "lush=inf", "lush=-inf"] {
            let err = BiomeAmounts::parse(spec).unwrap_err();
            assert!(
                err.contains("finite"),
                "{spec} should be rejected as non-finite, got: {err}"
            );
        }
    }
}
