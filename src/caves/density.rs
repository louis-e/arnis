//! Vanilla 1.21.8 cave density field: an exact transcription of the extracted vanilla density
//! JSON with `sloped_cheese` pinned (terrain-decoupled). Noise math is bit-identical to vanilla
//! (validated by a Java parity harness). The carve integration lives in `mod.rs`.
//!
//! The field is vanilla's, translated so vanilla's floor lands on this world's bedrock plane:
//! every Y is shifted by [`super::y_shift`] before it reaches the noise, so a world whose floor
//! is -64 gets exactly vanilla's field.
//!
//! `sloped_cheese`: in true vanilla it GROWS with depth (a spline over continentalness/erosion),
//! so the cheese-bias term `clamp(1.5 - 0.64*sloped_cheese, 0, 0.5)` goes to 0 deep underground
//! (unconstrained, full-strength caverns) but stays solid-biased near normal cave depths. A flat
//! constant at ALL depths would let big cheese caverns appear as often at shallow/mid depths as
//! near bedrock, so `combined_density` applies a depth-tapered `depth_k` instead.

use super::noise::NormalNoise;
use super::rng::XoroRandom;
use super::VANILLA_FLOOR;

/// Layer-banding strength: `LAYER_SQUEEZE * layer^2` is added to the cheese term, so cheese caverns
/// can only form where `layer` (a noise that cycles roughly every 16-32 world-y) is near zero — this
/// is vanilla's actual anisotropy control, pinching caverns into flat horizontal bands instead of
/// round/tall blobs. Raised from vanilla's 4.0: at 4.0 the bands are weak enough that tall "egg"
/// caverns survive even off-center; 7.0 squeezes caverns flatter/wider.
const LAYER_SQUEEZE: f64 = 7.0;
/// Shrink for the `entrances` (spaghetti_3d "cave entrance") system — without it this system is the
/// main source of giant tall/round "egg" caverns (the cheese term is already solid at those cells;
/// entrances alone can stay shallowly negative across a ~70-block vertical span at a ~20-block-wide
/// cross-section — a real wide+tall cavern, not a thin spaghetti tube). Added to `arg_a` only near
/// normal cave depths (see `depth_bias` in `entrances()`); 0 near bedrock so deep entrance caverns
/// are still allowed, matching vanilla intent.
const ENTRANCE_SHRINK: f64 = 0.32;
/// Shrink for the `entrances` spaghetti_3d worm term (arg_b) — see `entrances()` doc. Moderate on
/// purpose: this is the SAME system that gives spaghetti_3d its "occasional bigger tunnel"
/// rare-bucket variety in vanilla, and an aggressive offset can silently kill an entire cave system
/// (a gate pushed past the noise's reachable range never fires) — so keep this small relative to
/// ENTRANCE_SHRINK/CHEESE_SHRINK and re-measure when tuning rather than maxing it out.
const TUBE_SHRINK: f64 = 0.05;

/// Extra positive bias added to the cheese term ONLY (not spaghetti/entrances/pillars). Vanilla's
/// `cheese_bias` clamp caps at 0.5, which still leaves the cell-interpolated cheese caverns very
/// large and round. This shrinks the cheese voids — their outer
/// shell (density in [-SHRINK, 0]) stays solid — without touching the tunnel network (spaghetti +
/// carvers carry that). 0.0 = pure-vanilla cheese size.
const CHEESE_SHRINK: f64 = 0.38;

// Vanilla noise parameters (firstOctave, amplitudes) — from the extracted vanilla worldgen JSON.
struct P(i32, &'static [f64]);
const CAVE_CHEESE: P = P(-8, &[0.5, 1.0, 2.0, 1.0, 2.0, 1.0, 0.0, 2.0, 0.0]);
const CAVE_LAYER: P = P(-8, &[1.0]);
const CAVE_ENTRANCE: P = P(-7, &[0.4, 0.5, 1.0]);
const SPAGHETTI_2D: P = P(-7, &[1.0]);
const SPAGHETTI_2D_ELEVATION: P = P(-8, &[1.0]);
const SPAGHETTI_2D_MODULATOR: P = P(-11, &[1.0]);
const SPAGHETTI_2D_THICKNESS: P = P(-11, &[1.0]);
const SPAGHETTI_3D_1: P = P(-7, &[1.0]);
const SPAGHETTI_3D_2: P = P(-7, &[1.0]);
const SPAGHETTI_3D_RARITY: P = P(-11, &[1.0]);
const SPAGHETTI_3D_THICKNESS: P = P(-8, &[1.0]);
const SPAGHETTI_ROUGHNESS: P = P(-5, &[1.0]);
const SPAGHETTI_ROUGHNESS_MODULATOR: P = P(-8, &[1.0]);
const NOODLE: P = P(-8, &[1.0]);
const NOODLE_THICKNESS: P = P(-8, &[1.0]);
const NOODLE_RIDGE_A: P = P(-7, &[1.0]);
const NOODLE_RIDGE_B: P = P(-7, &[1.0]);
const PILLAR: P = P(-7, &[1.0, 1.0]);
const PILLAR_RARENESS: P = P(-8, &[1.0]);
const PILLAR_THICKNESS: P = P(-8, &[1.0]);

/// All cave NormalNoises, built once per world from the seed.
pub struct CaveGen {
    /// World Y minus vanilla Y, captured once so the hot path does not reload it.
    y_shift: i32,
    cave_cheese: NormalNoise,
    cave_layer: NormalNoise,
    cave_entrance: NormalNoise,
    spaghetti_2d: NormalNoise,
    spaghetti_2d_elevation: NormalNoise,
    spaghetti_2d_modulator: NormalNoise,
    spaghetti_2d_thickness: NormalNoise,
    spaghetti_3d_1: NormalNoise,
    spaghetti_3d_2: NormalNoise,
    spaghetti_3d_rarity: NormalNoise,
    spaghetti_3d_thickness: NormalNoise,
    spaghetti_roughness: NormalNoise,
    spaghetti_roughness_modulator: NormalNoise,
    noodle: NormalNoise,
    noodle_thickness: NormalNoise,
    noodle_ridge_a: NormalNoise,
    noodle_ridge_b: NormalNoise,
    pillar: NormalNoise,
    pillar_rareness: NormalNoise,
    pillar_thickness: NormalNoise,
}

impl CaveGen {
    pub fn new(seed: i64) -> Self {
        // RandomState-style: one positional factory off the world seed; each named noise seeded by
        // md5 of its registry id. Deterministic + distinct + world-seed-dependent (vanilla-faithful;
        // exact RandomState registry-order wiring only matters for byte-exact-to-seed, not pursued).
        let factory = XoroRandom::from_seed(seed).fork_positional();
        let mk = |id: &str, p: &P| {
            let mut r = factory.from_hash_of(&format!("minecraft:{}", id));
            NormalNoise::create(&mut r, p.0, p.1)
        };
        CaveGen {
            y_shift: super::y_shift(),
            cave_cheese: mk("cave_cheese", &CAVE_CHEESE),
            cave_layer: mk("cave_layer", &CAVE_LAYER),
            cave_entrance: mk("cave_entrance", &CAVE_ENTRANCE),
            spaghetti_2d: mk("spaghetti_2d", &SPAGHETTI_2D),
            spaghetti_2d_elevation: mk("spaghetti_2d_elevation", &SPAGHETTI_2D_ELEVATION),
            spaghetti_2d_modulator: mk("spaghetti_2d_modulator", &SPAGHETTI_2D_MODULATOR),
            spaghetti_2d_thickness: mk("spaghetti_2d_thickness", &SPAGHETTI_2D_THICKNESS),
            spaghetti_3d_1: mk("spaghetti_3d_1", &SPAGHETTI_3D_1),
            spaghetti_3d_2: mk("spaghetti_3d_2", &SPAGHETTI_3D_2),
            spaghetti_3d_rarity: mk("spaghetti_3d_rarity", &SPAGHETTI_3D_RARITY),
            spaghetti_3d_thickness: mk("spaghetti_3d_thickness", &SPAGHETTI_3D_THICKNESS),
            spaghetti_roughness: mk("spaghetti_roughness", &SPAGHETTI_ROUGHNESS),
            spaghetti_roughness_modulator: mk(
                "spaghetti_roughness_modulator",
                &SPAGHETTI_ROUGHNESS_MODULATOR,
            ),
            noodle: mk("noodle", &NOODLE),
            noodle_thickness: mk("noodle_thickness", &NOODLE_THICKNESS),
            noodle_ridge_a: mk("noodle_ridge_a", &NOODLE_RIDGE_A),
            noodle_ridge_b: mk("noodle_ridge_b", &NOODLE_RIDGE_B),
            pillar: mk("pillar", &PILLAR),
            pillar_rareness: mk("pillar_rareness", &PILLAR_RARENESS),
            pillar_thickness: mk("pillar_thickness", &PILLAR_THICKNESS),
        }
    }

    // ---- density sub-trees (exact transcription of the vanilla JSON) ----

    fn s2d_thickness_modulator(&self, x: f64, y: f64, z: f64) -> f64 {
        -0.95 + -0.35000000000000003 * self.spaghetti_2d_thickness.get_value(x * 2.0, y, z * 2.0)
    }

    fn spaghetti_roughness_function(&self, x: f64, y: f64, z: f64) -> f64 {
        let a = -0.05 + -0.05 * self.spaghetti_roughness_modulator.get_value(x, y, z);
        let b = -0.4 + self.spaghetti_roughness.get_value(x, y, z).abs();
        a * b
    }

    fn spaghetti_2d(&self, x: f64, y: f64, z: f64) -> f64 {
        let modv = self.spaghetti_2d_modulator.get_value(x * 2.0, y, z * 2.0);
        let d = rarity_2d(modv);
        let wss = d * (self.spaghetti_2d.get_value(x / d, y / d, z / d)).abs();
        let thick = self.s2d_thickness_modulator(x, y, z);
        let arg1 = wss + 0.083 * thick;
        let elev = 8.0 * self.spaghetti_2d_elevation.get_value(x, 0.0, z);
        let ycg = y_clamped_gradient(y, -64, 320, 8.0, -40.0);
        let cube_arg = (elev + ycg).abs() + thick;
        let arg2 = cube_arg * cube_arg * cube_arg;
        clamp(arg1.max(arg2), -1.0, 1.0)
    }

    fn entrances(&self, x: f64, y: f64, z: f64) -> f64 {
        // y sampled at 1.1 (vanilla: 0.5). A y-scale slower than the 0.75 x/z scale stretches each
        // lobe of this noise vertically into tall egg-shaped caverns; the faster y-scale shortens a
        // lobe's vertical extent = flatter/wider caverns, matching vanilla's typical entrance-cave
        // look. ENTRANCE_SHRINK + the depth taper mirror the cheese treatment: rare/small at normal
        // cave depths, only opening into a real unconstrained entrance cavern near bedrock.
        let depth_bias = y_clamped_gradient(y, -40, 30, 0.0, ENTRANCE_SHRINK);
        let arg_a = (0.37 + self.cave_entrance.get_value(x * 0.75, y * 1.1, z * 0.75))
            + y_clamped_gradient(y, -10, 30, 0.3, 0.0)
            + depth_bias;
        let rarity_in = self.spaghetti_3d_rarity.get_value(x * 2.0, y, z * 2.0); // cache_once
        let d = rarity_3d(rarity_in);
        let s1 = d * self.spaghetti_3d_1.get_value(x / d, y / d, z / d).abs();
        let s2 = d * self.spaghetti_3d_2.get_value(x / d, y / d, z / d).abs();
        // in the "rare" d=1.5/2.0 buckets, s1/s2 scale up right along with the /d sampling zoom-out,
        // so a near-zero-crossing patch of BOTH component noises at once — normally a thin worm cross
        // -section — balloons ISOTROPICALLY into a wide, round, tens-of-blocks pocket (this term alone
        // can stay negative across a ~70-block vertical span at ~20 blocks wide). tube_shrink tightens
        // the near-zero-crossing threshold at normal cave depths (thinner tubes, no accidental
        // caverns); 0 near bedrock, matching entrances/cheese.
        let tube_shrink = y_clamped_gradient(y, -40, 30, 0.0, TUBE_SHRINK);
        let s3d_thick = -0.0765
            + -0.011499999999999996 * self.spaghetti_3d_thickness.get_value(x, y, z)
            + tube_shrink;
        let clamped = clamp(s1.max(s2) + s3d_thick, -1.0, 1.0);
        let arg_b = self.spaghetti_roughness_function(x, y, z) + clamped;
        arg_a.min(arg_b)
    }

    fn pillars(&self, x: f64, y: f64, z: f64) -> f64 {
        let a = 2.0 * self.pillar.get_value(x * 25.0, y * 0.3, z * 25.0)
            + (-1.0 - self.pillar_rareness.get_value(x, y, z));
        let t = 0.55 + 0.55 * self.pillar_thickness.get_value(x, y, z);
        a * (t * t * t)
    }

    fn noodle(&self, x: f64, y: f64, z: f64) -> f64 {
        let in_range = (-60.0..321.0).contains(&y);
        let toggle = if in_range {
            self.noodle.get_value(x, y, z)
        } else {
            -1.0
        };
        if toggle < 0.0 {
            return 64.0;
        }
        let thickness = if in_range {
            -0.07500000000000001 + -0.025 * self.noodle_thickness.get_value(x, y, z)
        } else {
            0.0
        };
        let s = 2.6666666666666665;
        let ra = if in_range {
            self.noodle_ridge_a.get_value(x * s, y * s, z * s)
        } else {
            0.0
        };
        let rb = if in_range {
            self.noodle_ridge_b.get_value(x * s, y * s, z * s)
        } else {
            0.0
        };
        thickness + 1.5 * ra.abs().max(rb.abs())
    }

    /// The combined cheese/spaghetti/entrances/pillars density AFTER slides + squeeze, but WITHOUT
    /// the noodle min. In vanilla this is the single `interpolated(...)` blob → it must be sampled at
    /// CELL CORNERS (4×8×4) and trilerped, which is what gives vanilla-sized smooth rooms (per-block
    /// eval makes rooms fatter). Block SOLID iff combined.min(noodle) > 0.
    pub fn combined_density(&self, x: i32, y: i32, z: i32) -> f64 {
        let (xf, yf, zf) = (x as f64, (y - self.y_shift) as f64, z as f64);
        let entrances = self.entrances(xf, yf, zf);
        let layer = self.cave_layer.get_value(xf, yf * 8.0, zf);
        let cheese_noise = clamp(
            0.27 + self.cave_cheese.get_value(xf, yf * 0.6666666666666666, zf),
            -1.0,
            1.0,
        );
        // depth-tapered sloped_cheese: small (solid-biased) at normal cave depths, only opens up to
        // big unconstrained caverns deep near bedrock (see the module notes).
        let depth_k = y_clamped_gradient(yf, -40, 30, 5.0, 0.5);
        let cheese_bias = clamp(1.5 - 0.64 * depth_k, 0.0, 0.5);
        let cheese_term =
            LAYER_SQUEEZE * layer * layer + cheese_noise + cheese_bias + CHEESE_SHRINK;
        let t1 = cheese_term.min(entrances);
        let spag = self.spaghetti_2d(xf, yf, zf) + self.spaghetti_roughness_function(xf, yf, zf);
        let t2 = t1.min(spag);
        let pillars = self.pillars(xf, yf, zf);
        let pillars_gated = if pillars < 0.03 { -1.0e6 } else { pillars };
        let cave = t2.max(pillars_gated);
        // Depth slides, in vanilla coordinates since `yf` was translated above.
        //
        // BOTTOM: vanilla fades caves in over the first 24 blocks above its floor, which the
        // translation puts on this world's bedrock plane.
        //
        // TOP: vanilla fades the cave term to nothing over 240..256, and above 256 the residual
        // constants are a flat -0.078125 = unconditional AIR. Here the density is only ever
        // evaluated INSIDE terrain - the sky never asks - so that fade cannot do its vanilla job
        // (blending against a terrain field we do not have); all it could do is hollow every
        // peak reaching into it, one giant void under any mountain in a tall world. The band
        // therefore sits entirely ABOVE the world ceiling: yg2 stays 1.0 for every Y that can
        // hold a block, at any world height.
        let yg1 = y_clamped_gradient(yf, VANILLA_FLOOR, VANILLA_FLOOR + 24, 0.0, 1.0);
        let (pinch_lo, pinch_hi) = top_pinch_band(self.y_shift);
        let yg2 = y_clamped_gradient(yf, pinch_lo, pinch_hi, 1.0, 0.0);
        let inner = 0.1171875 + yg1 * (-0.1171875 + (-0.078125 + yg2 * (0.078125 + cave)));
        squeeze(0.64 * inner) // blend_density = identity
    }

    /// The noodle term — `min`'d in per-block at full resolution (keeps worms thin).
    pub fn noodle_density(&self, x: i32, y: i32, z: i32) -> f64 {
        self.noodle(x as f64, (y - self.y_shift) as f64, z as f64)
    }
}

#[inline]
fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}
#[inline]
fn squeeze(v: f64) -> f64 {
    let c = clamp(v, -1.0, 1.0);
    c / 2.0 - c * c * c / 24.0
}

/// The band, in vanilla coordinates, over which the cave term fades out near the ceiling.
/// It sits entirely above this world's ceiling so it never touches a Y that can hold a block;
/// see the comment at the use site.
#[inline]
fn top_pinch_band(y_shift: i32) -> (i32, i32) {
    let top_excl = crate::world_editor::world_max_y() + 1 - y_shift;
    (top_excl, top_excl + 16)
}

#[inline]
fn y_clamped_gradient(y: f64, from_y: i32, to_y: i32, from_v: f64, to_v: f64) -> f64 {
    let (fy, ty) = (from_y as f64, to_y as f64);
    if y <= fy {
        from_v
    } else if y >= ty {
        to_v
    } else {
        from_v + (to_v - from_v) * ((y - fy) / (ty - fy))
    }
}
#[inline]
fn rarity_2d(v: f64) -> f64 {
    if v < -0.75 {
        0.5
    } else if v < -0.5 {
        0.75
    } else if v < 0.5 {
        1.0
    } else if v < 0.75 {
        2.0
    } else {
        3.0
    }
}
#[inline]
fn rarity_3d(v: f64) -> f64 {
    if v < -0.5 {
        0.75
    } else if v < 0.0 {
        1.0
    } else if v < 0.5 {
        1.5
    } else {
        2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_editor::{
        set_terrain_floor_y, set_world_bounds, DEFAULT_MAX_Y, DEFAULT_MIN_Y, FLOOR_TEST_LOCK,
    };

    fn restore_vanilla_bounds() {
        set_world_bounds(DEFAULT_MIN_Y, DEFAULT_MAX_Y);
        set_terrain_floor_y(DEFAULT_MIN_Y + 2);
    }

    /// Vanilla's top fade (240..256) would turn every underground block above y~256 into air,
    /// because above it the residual constants are a flat -0.078125. In a tall world the band
    /// sits above the ceiling, so density there must vary with the noise instead.
    #[test]
    fn tall_world_keeps_rock_above_256() {
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // A tall ceiling over the vanilla floor.
        set_world_bounds(DEFAULT_MIN_Y, 2031);
        set_terrain_floor_y(DEFAULT_MIN_Y + 2);
        let gen = CaveGen::new(1);
        let mut solid = 0;
        let mut total = 0;
        for x in (0..512).step_by(16) {
            for z in (0..512).step_by(16) {
                for y in [260, 300, 400, 800] {
                    total += 1;
                    if gen.combined_density(x, y, z) >= 0.0 {
                        solid += 1;
                    }
                }
            }
        }
        restore_vanilla_bounds();
        // Assert the void is gone without pinning noise specifics.
        assert!(
            solid > total / 2,
            "expected mostly rock above y=256 in a tall world, got {solid}/{total} solid"
        );
    }

    /// The band sits above the ceiling, in vanilla coordinates.
    #[test]
    fn band_follows_the_world_top() {
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        restore_vanilla_bounds();
        let (lo, hi) = top_pinch_band(0);
        assert_eq!((lo, hi), (DEFAULT_MAX_Y + 1, DEFAULT_MAX_Y + 17));
        let (lo, _) = top_pinch_band(96);
        assert_eq!(
            lo + 96,
            DEFAULT_MAX_Y + 1,
            "a shifted field keeps its band above the top"
        );
    }

    /// A raised bedrock plane carries the whole field up with it: the density at a block of
    /// the raised world equals the vanilla density the same distance above vanilla's floor.
    #[test]
    fn a_raised_floor_translates_the_field() {
        let _g = FLOOR_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_world_bounds(DEFAULT_MIN_Y, 2031);
        set_terrain_floor_y(DEFAULT_MIN_Y + 2);
        let vanilla = CaveGen::new(7);
        // A ground level of 100 puts the floor at 36, snapped down to 32: 96 above -64.
        set_terrain_floor_y(100);
        let raised = CaveGen::new(7);
        let shift = crate::world_editor::terrain_floor_y() - VANILLA_FLOOR;
        restore_vanilla_bounds();

        assert_eq!(shift, 96);
        for (x, y, z) in [(3, -60, 9), (40, -20, -17), (-75, 12, 33), (8, 90, 8)] {
            assert_eq!(
                vanilla.combined_density(x, y, z),
                raised.combined_density(x, y + shift, z)
            );
            assert_eq!(
                vanilla.noodle_density(x, y, z),
                raised.noodle_density(x, y + shift, z)
            );
        }
    }
}
