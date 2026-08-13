//! Farmland texturing: parcels, monoculture crop plots, and style presets.
//!
//! `landuse=farmland` renders today as one uniform sheet of crops. Real farmland is a
//! patchwork of separate plots, each worked on its own, each growing one thing.
//!
//! This module splits farmland into rectangular **parcels** laid out like real plots
//! seen from above, separated by dirt tracks, with a fine internal sub-noise so each
//! parcel reads as varied ground. Every farm parcel grows exactly **one crop** (wheat,
//! potato, carrot, beetroot, sunflower, pumpkin, or fallow), so the field system reads
//! as a crop patchwork rather than a single colour. Plots carry interior character:
//! worn coarse-dirt spots, a mid-plot working path on large parcels, sunflower rows on
//! dirt, a pumpkin patch on a grass and coarse mosaic.
//!
//! Parcel grids sit at one of six orientations chosen per macro-region, aligned to the
//! dominant nearby road where there is one (real fields are laid out off their access
//! road), so plots do not all snap to the world axes.
//!
//! Everything is a pure function of `(x, z)`. No per-run state, no RNG threading, so
//! output is identical across separate runs and across overlapping bounding boxes.
//! [`FieldPreset::Classic`] reproduces the previous surface exactly.

use crate::block_definitions::*;
use crate::ground_generation::value_noise_01;
use crate::land_cover::coord_hash;

/// Farmland style preset, matching the GUI's segmented control (src/gui/js/main.js).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum FieldPreset {
    /// Uniform crops, the pre-3.1 surface
    Classic,
    /// Many small plots, full crop variety, dense tracks
    Smallholding,
    /// Balanced mixed farmland
    #[default]
    Patchwork,
    /// Large industrial fields, wheat-led, few tracks
    Prairie,
    /// Grazing land: grass and wildflowers with a few crop plots
    Pasture,
}

impl FieldPreset {
    /// Parse the GUI's segment value, falling back to the shipped default.
    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "classic" => FieldPreset::Classic,
            "smallholding" => FieldPreset::Smallholding,
            "prairie" => FieldPreset::Prairie,
            "pasture" => FieldPreset::Pasture,
            _ => FieldPreset::Patchwork,
        }
    }
}

/// One patch style.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldCategory {
    Coarse,
    Plains,
    Flower,
    Farm,
    Moss,
}

/// One farm-plot crop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FarmCrop {
    Wheat,
    Potato,
    Carrot,
    Beetroot,
    Sunflower,
    Pumpkin,
    Fallow,
}

/// A resolved cell: style, surface block, per-plot crop, growth level, and track flag.
/// Decoration keys off the surface (for example sunflower rows are the coarse-dirt rows).
#[derive(Clone, Copy)]
pub struct FieldCell {
    pub cat: FieldCategory,
    pub crop: Option<FarmCrop>,
    /// 0..=7 growth level, uniform within a farm parcel (a field is planted at once).
    /// Mapped per crop when placed, since beetroot only reaches age 3.
    pub crop_age: u8,
    /// Stable per-parcel seed; flower plots derive their 2-3 species subset from it.
    pub species_seed: u32,
    pub surface: Block,
    pub is_track: bool,
}

/// Relative area shares for the five categories.
#[derive(Clone, Copy)]
struct FieldMix {
    coarse: u16,
    plains: u16,
    flower: u16,
    farm: u16,
    moss: u16,
}

/// Relative shares of the seven farm-plot crops.
#[derive(Clone, Copy)]
struct FarmCrops {
    weights: [u16; 7],
}

const CROP_ORDER: [FarmCrop; 7] = [
    FarmCrop::Wheat,
    FarmCrop::Potato,
    FarmCrop::Carrot,
    FarmCrop::Beetroot,
    FarmCrop::Sunflower,
    FarmCrop::Pumpkin,
    FarmCrop::Fallow,
];

impl FarmCrops {
    fn pick(&self, px: i32, pz: i32) -> FarmCrop {
        let total: u64 = self.weights.iter().map(|&v| v as u64).sum();
        if total == 0 {
            return FarmCrop::Wheat;
        }
        // Distinct stream from the category roll so crop and style do not correlate.
        let mut roll = coord_hash(px ^ 0x0000_C0FE, pz.wrapping_mul(13)) % total;
        for (i, &w) in self.weights.iter().enumerate() {
            if roll < w as u64 {
                return CROP_ORDER[i];
            }
            roll -= w as u64;
        }
        FarmCrop::Wheat
    }
}

/// A farmland texture: style mix, parcel-size band, track probability, and crop shares.
#[derive(Clone, Copy)]
pub struct FieldProfile {
    preset: FieldPreset,
    mix: FieldMix,
    crops: FarmCrops,
    sizes: [i32; 3],
    track_pct: u64,
    /// Map scale (blocks per metre). Parcel sizes are defined at the 0.05 (1:20)
    /// reference, so real-world parcel size stays constant across map scales.
    map_scale: f64,
}

/// A resolved parcel reference: grid id, dimensions, cell-local coordinates, the
/// orientation-domain salt that keeps neighbouring domains' parcels independent, and
/// the parcel's own world centre (terrain steering is sampled there so a plot never
/// splits its crop down the middle).
#[derive(Clone, Copy)]
struct ParcelRef {
    px: i32,
    pz: i32,
    w: i32,
    l: i32,
    lx: i32,
    lz: i32,
    dsalt: i32,
    cx: i32,
    cz: i32,
    /// This block sits on the line where two orientation domains meet.
    on_domain_edge: bool,
}

/// Orientation-domain edge length in blocks.
const MACRO: i32 = 192;
const WARP: f64 = 4.0;
const WARP_SCALE: i32 = 24;
const SUB_SCALE: i32 = 6;
/// Half-width (blocks) of the boundary between two orientation domains. The warp that
/// makes the border meander also stretches and compresses it, so a 1-block line would
/// break up; 1 here gives a 2-3 block track that survives the distortion.
const DOMAIN_EDGE: i32 = 1;
/// Share of orientation-domain borders that carry a headland track. Below 100 so the
/// boundary network keeps some gaps instead of reading as a lattice.
const DOMAIN_TRACK_PCT: u64 = 72;

/// Fallback field-system orientations, used where no road is near enough to define one.
/// Each macro cell picks one, so parcel grids sit at several angles like real farmland
/// rather than one world-aligned grid.
const ANGLES: [(f64, f64); 6] = [
    // (sin, cos) for 0, 15, 30, 45, -15, -30 degrees
    (0.0, 1.0),
    (0.258_819, 0.965_926),
    (0.5, 0.866_025),
    (
        std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
    ),
    (-0.258_819, 0.965_926),
    (-0.5, 0.866_025),
];

/// Surface for a farm plot cell, including its interior character: noise-driven worn
/// spots, mid-plot working path, sunflower rows, pumpkin mosaic.
fn farm_surface(crop: FarmCrop, x: i32, z: i32, p: &ParcelRef) -> Block {
    let n = (value_noise_01(x, z, SUB_SCALE) * 1000.0) as i32;
    // Large parcels get a worn mid-plot working path on roughly 45% of plots.
    let has_mid_path =
        p.w >= 30 && coord_hash(p.px ^ 0x0000_11C7, (p.pz ^ 0x0000_33B1) ^ p.dsalt) % 100 < 45;
    if has_mid_path && p.lx == p.w / 2 && !matches!(crop, FarmCrop::Sunflower) {
        return COARSE_DIRT;
    }
    match crop {
        FarmCrop::Wheat | FarmCrop::Potato | FarmCrop::Carrot | FarmCrop::Beetroot => {
            // Tilled plot with noise-driven worn spots (coarse and rooted dirt).
            if n < 38 {
                COARSE_DIRT
            } else if n < 52 {
                ROOTED_DIRT
            } else {
                FARMLAND
            }
        }
        FarmCrop::Sunflower => {
            // Planted rows on coarse dirt, packed mud between the rows. Plain dirt would
            // slowly regrow grass in-game, packed mud stays bare. Grass creeps in at the
            // low end of the noise.
            if n < 160 {
                GRASS_BLOCK
            } else if p.lz.rem_euclid(2) == 0 {
                COARSE_DIRT
            } else {
                PACKED_MUD
            }
        }
        FarmCrop::Pumpkin => {
            // Pumpkin patch: grass and coarse mosaic, not tilled.
            if n < 420 {
                COARSE_DIRT
            } else {
                GRASS_BLOCK
            }
        }
        FarmCrop::Fallow => {
            // Resting field: worked bare ground being reclaimed by grass. No farmland
            // here, because bare farmland with no crop on top reverts to dirt in-game.
            // The locked bare palette never decays.
            if n < 330 {
                COARSE_DIRT
            } else if n < 450 {
                ROOTED_DIRT
            } else if n < 540 {
                PACKED_MUD
            } else if n < 700 {
                GRASS_BLOCK
            } else {
                COARSE_DIRT
            }
        }
    }
}

/// Per-category surface for the non-farm styles.
fn surface_block(cat: FieldCategory, x: i32, z: i32) -> Block {
    let n = (value_noise_01(x, z, SUB_SCALE) * 1000.0) as i32;
    match cat {
        FieldCategory::Coarse => {
            // Bare, disturbed ground: coarse dirt, grass, packed mud, rooted dirt, plus
            // locked dirt-path patches. No plain dirt or mud, since none of the blocks
            // used here regrow grass in-game and the patch should stay disturbed.
            if n < 160 {
                PACKED_MUD
            } else if n < 250 {
                ROOTED_DIRT
            } else if n < 300 {
                DIRT_PATH
            } else if n < 380 {
                GRASS_BLOCK
            } else {
                COARSE_DIRT
            }
        }
        FieldCategory::Moss => {
            if n < 300 {
                GRASS_BLOCK
            } else if n < 360 {
                COARSE_DIRT
            } else if n < 400 {
                ROOTED_DIRT
            } else {
                MOSS_BLOCK
            }
        }
        // Vanilla-plains look: grass with sparse coarse-dirt patches breaking it up.
        FieldCategory::Plains => {
            if n < 35 {
                COARSE_DIRT
            } else {
                GRASS_BLOCK
            }
        }
        FieldCategory::Flower => GRASS_BLOCK,
        FieldCategory::Farm => FARMLAND,
    }
}

impl FieldMix {
    fn total(&self) -> u64 {
        self.coarse as u64
            + self.plains as u64
            + self.flower as u64
            + self.farm as u64
            + self.moss as u64
    }
}

impl FieldProfile {
    /// Build the profile for a preset. [`FieldPreset::Classic`] yields an inactive
    /// profile: callers skip the whole field pass and the surface is untouched.
    pub fn new(preset: FieldPreset) -> Self {
        // (coarse, plains, flower, farm, moss), parcel size band, track %, crop weights
        // in CROP_ORDER (wheat, potato, carrot, beetroot, sunflower, pumpkin, fallow).
        let (mix, sizes, track_pct, weights) = match preset {
            FieldPreset::Classic => ((0, 0, 0, 100, 0), [18, 30, 46], 0, [100, 0, 0, 0, 0, 0, 0]),
            FieldPreset::Smallholding => (
                (12, 10, 4, 70, 4),
                [10, 16, 24],
                60,
                [22, 18, 18, 14, 12, 10, 6],
            ),
            FieldPreset::Patchwork => (
                (10, 8, 2, 75, 5),
                [18, 30, 46],
                45,
                [40, 15, 15, 8, 12, 5, 5],
            ),
            FieldPreset::Prairie => ((6, 6, 1, 85, 2), [46, 78, 120], 22, [62, 8, 6, 4, 12, 2, 6]),
            FieldPreset::Pasture => (
                (6, 58, 24, 6, 6),
                [40, 80, 140],
                18,
                [45, 10, 10, 5, 20, 5, 5],
            ),
        };
        FieldProfile {
            preset,
            mix: FieldMix {
                coarse: mix.0,
                plains: mix.1,
                flower: mix.2,
                farm: mix.3,
                moss: mix.4,
            },
            crops: FarmCrops { weights },
            sizes,
            track_pct,
            map_scale: 0.05,
        }
    }

    /// Bind the profile to the map scale so parcels keep their real-world size. The base
    /// sizes are defined at the 0.05 (1:20) reference, so a 1:10 map gets parcels twice
    /// as many blocks across, covering the same metres of land.
    pub fn with_map_scale(mut self, scale: f64) -> Self {
        if scale.is_finite() && scale > 0.0 {
            self.map_scale = scale;
        }
        self
    }

    /// True when this profile changes the surface at all.
    pub fn is_active(&self) -> bool {
        self.preset != FieldPreset::Classic
    }

    fn category_for_parcel(&self, px: i32, pz: i32, dsalt: i32) -> FieldCategory {
        let total = self.mix.total();
        if total == 0 {
            return FieldCategory::Farm;
        }
        let mut roll = coord_hash(px, (pz ^ 0x5F35_6495) ^ dsalt) % total;
        for (share, cat) in [
            (self.mix.coarse, FieldCategory::Coarse),
            (self.mix.plains, FieldCategory::Plains),
            (self.mix.flower, FieldCategory::Flower),
            (self.mix.farm, FieldCategory::Farm),
            (self.mix.moss, FieldCategory::Moss),
        ] {
            if roll < share as u64 {
                return cat;
            }
            roll -= share as u64;
        }
        FieldCategory::Farm
    }

    /// Resolve the parcel containing `(x, z)`.
    ///
    /// The map is divided into MACRO-sized orientation domains. Each domain takes its
    /// rotation from the dominant nearby road, or from a hash where no road is close,
    /// and hashes to a layout method: long strips either way, or blocky plots. Field
    /// grids therefore sit at several angles with mixed shapes, like real agricultural
    /// land. Everything stays a pure function of `(x, z)`.
    fn parcel_at(&self, x: i32, z: i32) -> ParcelRef {
        let wx = value_noise_01(x + 1000, z - 500, WARP_SCALE);
        let wz = value_noise_01(x - 700, z + 1300, WARP_SCALE);
        let sx = x + ((wx - 0.5) * 2.0 * WARP).round() as i32;
        let sz = z + ((wz - 0.5) * 2.0 * WARP).round() as i32;
        // Orientation domain. The lookup point is warped by a coarse noise (roughly a
        // 28-block wander on the 192 grid) so domain borders, where the angle and layout
        // change, meander organically instead of cutting along straight grid lines.
        let mwx = value_noise_01(x - 4000, z + 2000, 64);
        let mwz = value_noise_01(x + 5000, z - 3000, 64);
        let dx = x + ((mwx - 0.5) * 56.0).round() as i32;
        let dz = z + ((mwz - 0.5) * 56.0).round() as i32;
        let mx = dx.div_euclid(MACRO);
        let mz = dz.div_euclid(MACRO);
        let dh = coord_hash(mx ^ 0x0000_51ED, mz.wrapping_mul(7));
        let dsalt = (dh as i32) ^ (mx.wrapping_mul(0x1F12_3BB5)) ^ (mz.wrapping_mul(0x0077_F0ED));
        // Base parcel size, held at a constant real-world size across map scales.
        let scale_factor = self.map_scale / 0.05;
        let base = (self.sizes[(dh % 3) as usize] as f64 * scale_factor).round() as i64;
        let base = (base as i32).clamp(6, 400);
        // Layout method: strips one way, strips the other, or blocky plots.
        let (w, l) = match (dh >> 8) % 10 {
            0..=2 => ((base * 2 / 5).max(8), (base * 12 / 5).max(16)),
            3..=5 => ((base * 12 / 5).max(16), (base * 2 / 5).max(8)),
            _ => (base, base),
        };
        // Domain rotation: align to the dominant nearby road, since real fields are laid
        // out off their access road. Hashed angle where no road is near.
        let (sin_t, cos_t) =
            crate::road_bearings::bearing_at(mx * MACRO + MACRO / 2, mz * MACRO + MACRO / 2)
                .unwrap_or(ANGLES[((dh >> 16) % 6) as usize]);
        let fx = sx as f64;
        let fz = sz as f64;
        let rx = (fx * cos_t + fz * sin_t).round() as i32;
        let rz = (-fx * sin_t + fz * cos_t).round() as i32;
        let (px, pz) = (rx.div_euclid(w), rz.div_euclid(l));
        // Parcel centre, rotated back into world space: the one point every block of a
        // parcel agrees on, so terrain-driven steering resolves per plot, not per block.
        let (crx, crz) = ((px * w + w / 2) as f64, (pz * l + l / 2) as f64);
        // Where two orientation domains meet, the field system changes angle and a plot
        // spanning the line gets cut. Real farmland separates neighbouring field systems
        // with a track or headland, so mark the line and let `cell_at` lay a dirt track
        // along it. The cut then reads as a field boundary rather than a chopped plot.
        // Free to detect: it is the macro grid line already crossed above, in the same
        // warped space, so it meanders exactly like the domain border does.
        let edge_x = dx.rem_euclid(MACRO);
        let edge_z = dz.rem_euclid(MACRO);
        let on_domain_edge = edge_x <= DOMAIN_EDGE
            || edge_x >= MACRO - 1 - DOMAIN_EDGE
            || edge_z <= DOMAIN_EDGE
            || edge_z >= MACRO - 1 - DOMAIN_EDGE;
        ParcelRef {
            px,
            pz,
            w,
            l,
            lx: rx.rem_euclid(w),
            lz: rz.rem_euclid(l),
            dsalt,
            cx: (crx * cos_t - crz * sin_t).round() as i32,
            cz: (crx * sin_t + crz * cos_t).round() as i32,
            on_domain_edge,
        }
    }

    /// Category of the parcel containing `(x, z)`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn category_at(&self, x: i32, z: i32) -> FieldCategory {
        let p = self.parcel_at(x, z);
        self.category_for_parcel(p.px, p.pz, p.dsalt)
    }

    /// Crop of the farm parcel containing `(x, z)`, None off farm parcels.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn crop_at(&self, x: i32, z: i32) -> Option<FarmCrop> {
        let p = self.parcel_at(x, z);
        if self.category_for_parcel(p.px, p.pz, p.dsalt) == FieldCategory::Farm {
            Some(self.crops.pick(p.px, p.pz ^ p.dsalt))
        } else {
            None
        }
    }

    /// Full resolution of a cell: style, crop, surface, and track flag.
    pub fn cell_at(&self, x: i32, z: i32) -> FieldCell {
        let p = self.parcel_at(x, z);
        let cat = self.category_for_parcel(p.px, p.pz, p.dsalt);
        let mut is_track = false;
        if p.lx == 0 || p.lz == 0 || p.lx == p.w - 1 || p.lz == p.l - 1 {
            let (nx, nz) = if p.lx == 0 {
                (p.px - 1, p.pz)
            } else if p.lx == p.w - 1 {
                (p.px + 1, p.pz)
            } else if p.lz == 0 {
                (p.px, p.pz - 1)
            } else {
                (p.px, p.pz + 1)
            };
            if self.category_for_parcel(nx, nz, p.dsalt) != cat
                && coord_hash(p.px ^ nx, ((p.pz ^ nz) ^ 0x0000_7A11) ^ p.dsalt) % 100
                    < self.track_pct
            {
                is_track = true;
            }
        }
        // Headland track between two field systems. Most domain borders get one, since
        // real farmland separates differently-oriented systems with a working track. The
        // rest just meet, so the network does not read as a perfect grid.
        if p.on_domain_edge
            && coord_hash(p.dsalt ^ 0x0000_D0E9, p.dsalt.wrapping_mul(31)) % 100 < DOMAIN_TRACK_PCT
        {
            is_track = true;
        }
        let crop = if cat == FieldCategory::Farm {
            Some(self.crops.pick(p.px, p.pz ^ p.dsalt))
        } else {
            None
        };
        // Terrain steering: sunflower fields cluster in the low, open plains and are
        // demoted on higher ground. Parcel-hash consistent, so a field stays one crop.
        // Sampled at the parcel CENTRE, not at this block: a plot lying across a lowland
        // border would otherwise grow sunflowers in one half and wheat in the other,
        // which reads as a plot cut in two.
        let crop = crop.map(|c| {
            let low = crate::lowland::is_lowland(p.cx, p.cz);
            if low
                && c != FarmCrop::Sunflower
                && coord_hash(p.px ^ 0x0051_F10A, p.pz ^ p.dsalt) % 100 < 22
            {
                FarmCrop::Sunflower
            } else if !low
                && c == FarmCrop::Sunflower
                && coord_hash(p.px ^ 0x0051_F10B, p.pz ^ p.dsalt) % 100 < 60
            {
                FarmCrop::Wheat
            } else {
                c
            }
        });
        // Growth level, uniform per field since a field is planted together. Weighted
        // toward ripe with a few immature fields, so neighbouring plots read as
        // different growth stages.
        let crop_age = if crop.is_some() {
            match coord_hash(p.px ^ 0x0000_A9E3, p.pz.wrapping_mul(29) ^ p.dsalt) % 10 {
                0..=5 => 7,
                6 => 6,
                7 => 5,
                8 => 4,
                _ => 2,
            }
        } else {
            0
        };
        // Per-parcel species seed; flower plots pick a small species subset from it.
        let species_seed = coord_hash(p.px ^ 0x0000_F10E, p.pz.wrapping_mul(53) ^ p.dsalt) as u32;
        let surface = if is_track {
            DIRT_PATH
        } else if let Some(c) = crop {
            farm_surface(c, x, z, &p)
        } else {
            surface_block(cat, x, z)
        };
        FieldCell {
            cat,
            crop,
            crop_age,
            species_seed,
            surface,
            is_track,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Classic is inert: callers skip the field pass entirely, so the surface stays
    /// exactly what it was before this feature existed.
    #[test]
    fn classic_is_inactive_and_all_farm() {
        let p = FieldProfile::new(FieldPreset::Classic);
        assert!(!p.is_active());
        for x in -40..40 {
            for z in -40..40 {
                assert_eq!(p.category_at(x, z), FieldCategory::Farm);
            }
        }
    }

    /// Every non-Classic preset changes the surface.
    #[test]
    fn every_other_preset_is_active() {
        for preset in [
            FieldPreset::Smallholding,
            FieldPreset::Patchwork,
            FieldPreset::Prairie,
            FieldPreset::Pasture,
        ] {
            assert!(FieldProfile::new(preset).is_active(), "{preset:?}");
        }
    }

    #[test]
    fn farm_parcels_are_monoculture_and_diverse() {
        let p = FieldProfile::new(FieldPreset::Patchwork);
        // Monoculture: a cell and its immediate neighbour agree on the crop unless a
        // parcel boundary sits between them, so over a straight walk crop changes must
        // be far rarer than cells (parcels are at least 8 blocks wide).
        let mut changes = 0;
        let mut prev = p.cell_at(0, 500).crop;
        for x in 1..2000 {
            let c = p.cell_at(x, 500).crop;
            if c != prev {
                changes += 1;
                prev = c;
            }
        }
        // Strips can be as narrow as 8 blocks and boundaries sit at angles, so allow
        // more frequent changes than a blocky-only layout would give, but still far
        // below per-cell salt-and-pepper, which would be roughly 85% changes.
        assert!(
            changes < 2000 / 5,
            "crop changes {changes} too frequent for parcels"
        );
        // Diversity: all 7 crops appear over a wide area.
        let mut seen = std::collections::HashSet::new();
        for x in (0..8000).step_by(11) {
            for z in (0..8000).step_by(11) {
                if let Some(c) = p.crop_at(x, z) {
                    seen.insert(format!("{c:?}"));
                }
            }
        }
        assert!(seen.len() == 7, "all crops should appear, saw {seen:?}");
    }

    /// Prairie is wheat-led by design: its wheat share must dominate, and must be
    /// clearly higher than Smallholding's, which spreads crops evenly.
    #[test]
    fn crop_shares_follow_preset_weights() {
        fn wheat_share(preset: FieldPreset) -> f64 {
            let p = FieldProfile::new(preset);
            let (mut wheat, mut total) = (0u32, 0u32);
            for x in (0..9000).step_by(17) {
                for z in (0..9000).step_by(17) {
                    match p.crop_at(x, z) {
                        Some(FarmCrop::Wheat) => {
                            wheat += 1;
                            total += 1;
                        }
                        Some(_) => total += 1,
                        None => {}
                    }
                }
            }
            wheat as f64 / total as f64
        }
        let prairie = wheat_share(FieldPreset::Prairie);
        let small = wheat_share(FieldPreset::Smallholding);
        assert!(
            prairie > 0.5,
            "prairie wheat share {prairie} should dominate"
        );
        assert!(small < 0.35, "smallholding wheat share {small} too high");
        assert!(prairie > small + 0.2, "presets not clearly distinct");
    }

    /// Pasture is grazing land: mostly grass and flowers, only a few crop plots.
    #[test]
    fn pasture_is_mostly_grassy() {
        let p = FieldProfile::new(FieldPreset::Pasture);
        let (mut grassy, mut farm, mut n) = (0, 0, 0);
        for x in (0..8000).step_by(17) {
            for z in (0..8000).step_by(17) {
                match p.category_at(x, z) {
                    FieldCategory::Plains | FieldCategory::Flower => grassy += 1,
                    FieldCategory::Farm => farm += 1,
                    _ => {}
                }
                n += 1;
            }
        }
        assert!(
            grassy as f64 / n as f64 > 0.7,
            "pasture should be mostly grassy"
        );
        assert!(farm > 0, "pasture still keeps a few crop plots");
    }

    /// Parcel style shares must track the preset's declared mix.
    #[test]
    fn style_shares_roughly_match_the_mix() {
        let p = FieldProfile::new(FieldPreset::Prairie);
        let (mut farm, mut n) = (0, 0);
        for x in (0..6000).step_by(13) {
            for z in (0..6000).step_by(13) {
                if p.category_at(x, z) == FieldCategory::Farm {
                    farm += 1;
                }
                n += 1;
            }
        }
        // Prairie declares farm = 85 of 100.
        let ratio = farm as f64 / n as f64;
        assert!(ratio > 0.75 && ratio < 0.95, "farm share {ratio} not ~0.85");
    }

    /// Tracks appear only where two styles meet or where two field systems meet.
    #[test]
    fn tracks_only_between_different_styles_or_field_systems() {
        // Prairie's mix is farm-dominated, so most parcel boundaries are style-identical
        // and the surviving tracks are overwhelmingly headlands.
        let p = FieldProfile::new(FieldPreset::Prairie);
        let mut tracks = 0;
        for x in 0..400 {
            for z in 0..400 {
                if p.cell_at(x, z).is_track {
                    let r = p.parcel_at(x, z);
                    let boundary = r.lx == 0 || r.lz == 0 || r.lx == r.w - 1 || r.lz == r.l - 1;
                    assert!(
                        r.on_domain_edge || boundary,
                        "track at ({x},{z}) is neither a parcel boundary nor a headland"
                    );
                    tracks += 1;
                }
            }
        }
        assert!(tracks > 0, "no tracks at all");
        assert!(tracks < 400 * 400 / 5, "tracks cover too much ground");
    }

    /// A parcel grows one crop everywhere, including where terrain steering applies:
    /// the lowland probe reads the parcel centre, so a plot can never be sunflower on
    /// one side of a lowland border and wheat on the other.
    #[test]
    fn crop_is_uniform_across_a_whole_parcel() {
        let p = FieldProfile::new(FieldPreset::Patchwork);
        // A parcel is identified by its grid index AND its orientation domain: two
        // domains with different rotation and layout can reuse the same grid index, and
        // where they meet the field systems genuinely differ, as adjacent real field
        // systems laid out off different roads do.
        let mut seen: std::collections::HashMap<(i32, i32, i32), Option<FarmCrop>> =
            std::collections::HashMap::new();
        for x in -300..300 {
            for z in -300..300 {
                let r = p.parcel_at(x, z);
                let c = p.cell_at(x, z).crop;
                match seen.entry((r.px, r.pz, r.dsalt)) {
                    std::collections::hash_map::Entry::Occupied(e) => {
                        assert_eq!(*e.get(), c, "parcel ({},{}) grows two crops", r.px, r.pz);
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(c);
                    }
                }
            }
        }
    }

    /// All presets share one orientation field, so switching preset changes plot size
    /// and style mix but not which way the field system runs.
    #[test]
    fn presets_share_the_orientation_field() {
        let a = FieldProfile::new(FieldPreset::Patchwork);
        let b = FieldProfile::new(FieldPreset::Pasture);
        for x in (-500..500).step_by(37) {
            for z in (-500..500).step_by(41) {
                assert_eq!(
                    a.parcel_at(x, z).dsalt,
                    b.parcel_at(x, z).dsalt,
                    "presets disagree on the orientation domain at ({x},{z})"
                );
            }
        }
    }

    /// Parcels hold a constant real-world size: halving the map scale (1:20 to 1:10)
    /// must roughly double the parcel width in blocks.
    #[test]
    fn parcels_keep_their_real_world_size() {
        let at_20 = FieldProfile::new(FieldPreset::Patchwork).with_map_scale(0.05);
        let at_10 = FieldProfile::new(FieldPreset::Patchwork).with_map_scale(0.1);
        let a = at_20.parcel_at(1234, 5678);
        let b = at_10.parcel_at(1234, 5678);
        assert!(
            b.w >= a.w * 2 - 2 && b.w <= a.w * 2 + 2,
            "parcel width {} vs {} is not ~2x at half the scale",
            b.w,
            a.w
        );
    }
}
