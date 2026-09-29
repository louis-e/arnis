//! Natural surface materials: rock on steep ground, and snow by altitude and
//! terrain shape.
//!
//! Every choice reads smooth, salted noise fields keyed on world coordinates, so
//! materials form patches and layers tens of blocks across instead of per-block
//! speckle, and tiles stitch without seams.

use crate::block_definitions::{
    Block, ANDESITE, BLUE_ICE, COARSE_DIRT, COBBLED_DEEPSLATE, COBBLESTONE, DEEPSLATE, DIRT,
    GRASS_BLOCK, GRAVEL, ICE, MOSS_BLOCK, PACKED_ICE, PODZOL, SNOWY_GRASS_BLOCK, SNOWY_PODZOL,
    SNOW_BLOCK, SNOW_LAYERS, STONE, TUFF,
};
use crate::climate::Climate;
use crate::coordinate_system::cartesian::XZPoint;
use crate::ground::Ground;
use crate::ground_generation::{patch_noise, value_noise_salted};
use crate::land_cover::{
    LC_BARE, LC_CROPLAND, LC_GRASSLAND, LC_MOSS, LC_SHRUBLAND, LC_SNOW_ICE, LC_TREE_COVER,
};
use crate::world_editor::WorldEditor;

const SALT_STRATA_WARP: u32 = 0x5157_A7A1;
const SALT_STRATA_WAVE: u32 = 0x5157_A7A2;
const SALT_BED_EDGE: u32 = 0x5157_A7A4;
const SALT_BED_LENS: u32 = 0x5157_A7A5;
const SALT_LEDGE: u32 = 0x1ED6_E5A1;
const SALT_SCREE: u32 = 0x5C4E_E0B2;
const SALT_SCREE_EDGE: u32 = 0x5C4E_ED6E;
const SALT_WORN: u32 = 0x0B0A_4E11;
const SALT_BARE_ROCK: u32 = 0xBA4E_40C3;
const SALT_SNOW_LINE: u32 = 0x5A0E_11A4;
const SALT_SNOW_DRIFT: u32 = 0xD41F_7B05;
const SALT_SNOW_FIELD: u32 = 0xF1E1_D5A0;
const SALT_STREAK: u32 = 0x57EA_C0DE;
const SALT_STREAK_RUN: u32 = 0x57EA_0F11;
const SALT_TALUS: u32 = 0x07A1_05CE;
const SALT_TALUS_ROCK: u32 = 0x07A1_0B0D;
const SALT_SHADE_X: u32 = 0x05AD_E0A1;
const SALT_SHADE_Z: u32 = 0x05AD_E0B2;

/// Face rock from light to dark: fresh breaks, weathered rock, then wet and shaded rock.
const FACE_RAMP: [Block; 6] = [
    ANDESITE,
    STONE,
    COBBLESTONE,
    TUFF,
    COBBLED_DEEPSLATE,
    DEEPSLATE,
];
/// Darkest ramp step a cliff top may take, so ledges don't read black from above.
const TOP_RAMP_MAX: usize = 3;

/// Bed thicknesses in blocks, drawn per bed so the stack never repeats a rhythm.
const BED_THICKNESS: [i32; 10] = [2, 3, 3, 4, 4, 5, 5, 6, 7, 9];
/// Bottom and top of the bed stack, past any build height plus the strata warp.
const BED_FLOOR: i32 = -2200;
const BED_CEIL: i32 = 4200;

/// Where each bed of the stack starts, bottom up; the same stack everywhere.
static BED_STARTS: std::sync::LazyLock<Vec<i32>> = std::sync::LazyLock::new(|| {
    let mut starts = Vec::new();
    let mut y = BED_FLOOR;
    while y < BED_CEIL {
        starts.push(y);
        let k = starts.len() as i32;
        y += BED_THICKNESS[(crate::land_cover::coord_hash(k, 0x0BED) % 10) as usize];
    }
    starts
});

/// Rock strata of one column: layers of uneven thickness that bend across the
/// landscape, with ragged edges, and whose andesite and tuff pinch out into stone
/// along a face, so cliffs show bedding instead of noise or ruled stripes.
#[derive(Clone, Copy)]
pub(crate) struct Strata {
    x: i32,
    z: i32,
    warp: f64,
    starts: &'static [i32],
}

impl Strata {
    pub(crate) fn at(x: i32, z: i32) -> Self {
        Self {
            x,
            z,
            warp: (value_noise_salted(x, z, 48, SALT_STRATA_WARP) - 0.5) * 7.0
                + (value_noise_salted(x, z, 13, SALT_STRATA_WAVE) - 0.5) * 2.5,
            starts: &BED_STARTS,
        }
    }

    /// Layer index at `y` in the stack of beds, whose edges wander by a block.
    fn layer(self, y: i32) -> i32 {
        self.layer_walk(y, &mut 0)
    }

    /// `layer` for a walk up the column: `cursor` keeps the bed index between calls, so
    /// each step moves on by a bed instead of searching the stack again.
    fn layer_walk(self, y: i32, cursor: &mut usize) -> i32 {
        let w = f64::from(y) + self.warp;
        let starts = self.starts;
        if *cursor == 0 {
            *cursor = starts.partition_point(|&s| f64::from(s) <= w).max(1);
        }
        while *cursor < starts.len() && f64::from(starts[*cursor]) <= w {
            *cursor += 1;
        }
        let i = *cursor;
        let start = f64::from(starts[i - 1]);
        let end = starts.get(i).map_or(f64::INFINITY, |&e| f64::from(e));
        let bed = i as i32 - 1;
        // Only near an edge does the jitter decide which side a block falls on. It
        // moves a block by half at most and beds are two thick, so one bed over at most.
        if w - start >= 0.5 && end - w > 0.5 {
            return bed;
        }
        let edge = value_noise_salted(2 * self.x + 3 * self.z, y, 4, SALT_BED_EDGE) - 0.5;
        let jittered = w + edge;
        if jittered < start {
            (bed - 1).max(0)
        } else if jittered >= end {
            bed + 1
        } else {
            bed
        }
    }

    pub(crate) fn block(self, y: i32) -> Block {
        self.kind(self.layer(y))
    }

    /// Rock of a bed: stone, or andesite and tuff as lenses tens of blocks long.
    fn kind(self, layer: i32) -> Block {
        let kind = match crate::land_cover::coord_hash(layer, 0x57A7) % 100 {
            0..=69 => return STONE,
            70..=89 => ANDESITE,
            _ => TUFF,
        };
        let lens = patch_noise(
            self.x + layer.wrapping_mul(97),
            self.z - layer.wrapping_mul(61),
            24,
            SALT_BED_LENS,
        );
        if lens < 0.7 {
            kind
        } else {
            STONE
        }
    }

    /// Weathering tone at `y`. One field per horizontal axis, so faces of any heading
    /// get shading twice as tall as wide.
    fn tone(self, y: i32) -> f64 {
        0.5 * (value_noise_salted(2 * self.x, y, 12, SALT_SHADE_X)
            + value_noise_salted(2 * self.z, y, 12, SALT_SHADE_Z))
    }

    /// Ramp step of a bed at a tone, darker toward the foot of a face `depth` blocks
    /// below its top. Thresholds are the tone's measured quantiles: about 14% lighter,
    /// 12% darker and 4% darker still, leaving the beds readable.
    fn step(kind: Block, tone: f64, depth: i32) -> usize {
        let base: i32 = match kind {
            ANDESITE => 0,
            TUFF => 3,
            _ => 1,
        };
        let tone = tone + 0.09 * (f64::from(depth) / 32.0).min(1.0);
        let shift = if tone < 0.328 {
            -1
        } else if tone < 0.682 {
            0
        } else if tone < 0.763 {
            1
        } else {
            2
        };
        (base + shift).clamp(0, FACE_RAMP.len() as i32 - 1) as usize
    }

    /// The bed at `y`, shaded for a face `depth` blocks below its top.
    #[cfg(test)]
    fn shaded(self, y: i32, depth: i32) -> Block {
        FACE_RAMP[Self::step(self.block(y), self.tone(y), depth)]
    }
}

/// Blocks between tone samples down a rock column; the tone barely changes in between.
const TONE_STEP: i32 = 4;

/// Fills `y_min..=y_max` of a rock column with its strata, keeping whatever is
/// already there. `streaks` runs the weathering streaks of a sheer face down the beds.
pub(crate) fn fill_strata(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    y_min: i32,
    y_max: i32,
    streaks: bool,
) {
    if y_min > y_max {
        return;
    }
    let strata = Strata::at(x, z);
    let streak = streaks.then(|| streak_block(x, z)).flatten();
    // Tone is sampled every few blocks and interpolated, and a bed's lens is looked up
    // once per bed, since fills walk up a column one block at a time.
    let first = strata.tone(y_min);
    let next = if y_max > y_min {
        strata.tone(y_min + TONE_STEP)
    } else {
        first
    };
    let mut tones = (y_min, first, next);
    let mut bed: Option<(i32, Block)> = None;
    let mut cursor = 0;
    let block_at = |y: i32| {
        while y >= tones.0 + TONE_STEP {
            let at = tones.0 + TONE_STEP;
            tones = (at, tones.2, strata.tone(at + TONE_STEP));
        }
        if let Some(b) = streak.filter(|_| streak_runs(x, z, y)) {
            return b;
        }
        let layer = strata.layer_walk(y, &mut cursor);
        let kind = match bed {
            Some((l, k)) if l == layer => k,
            _ => {
                let k = strata.kind(layer);
                bed = Some((layer, k));
                k
            }
        };
        let t = tones.1 + (tones.2 - tones.1) * f64::from(y - tones.0) / f64::from(TONE_STEP);
        FACE_RAMP[Strata::step(kind, t, y_max + 1 - y)]
    };
    editor.fill_column_with_absolute(x, z, y_min, y_max, block_at);
}

/// Weathering streak down a sheer face in this column: deepslate at the core, tuff around.
fn streak_block(x: i32, z: i32) -> Option<Block> {
    let n = patch_noise(x, z, 3, SALT_STREAK);
    if n < 0.08 {
        Some(DEEPSLATE)
    } else if n < 0.22 {
        Some(TUFF)
    } else {
        None
    }
}

/// Streaks break off down the face instead of running as unbroken poles.
fn streak_runs(x: i32, z: i32, y: i32) -> bool {
    value_noise_salted(x.wrapping_mul(31) ^ z, y, 10, SALT_STREAK_RUN) < 0.7
}

/// Horizontal steps out from a wall at which fallen rock is looked for, and how thick it
/// lies there; taller walls reach the far rings.
const TALUS_REACH: [(i32, f64); 3] = [(4, 1.0), (8, 0.6), (16, 0.3)];
/// Rise over run up to a wall steep enough to shed rock (about 50 degrees).
const CLIFF_TAN: f64 = 1.2;

/// Terrain heights on a 4-block lattice around one chunk, so the talus test reads an
/// array instead of interpolating the elevation grid a dozen times per column. The
/// lattice sits on world multiples of 4, so tiles and runs agree at their seams.
pub(crate) struct TalusField {
    x0: i32,
    z0: i32,
    heights: [f64; Self::SIDE * Self::SIDE],
    highest: f64,
    correction: f64,
}

impl TalusField {
    const STEP: i32 = 4;
    const SIDE: usize = 15;

    /// `None` where the relief around the chunk could not hold a cliff, which spares
    /// the plains the lattice. `origin` is the world block of ground (0, 0).
    pub(crate) fn new(
        ground: &Ground,
        chunk_x: i32,
        chunk_z: i32,
        origin: (i32, i32),
    ) -> Option<Self> {
        let at = |x: i32, z: i32| ground.level_exact(XZPoint::new(x - origin.0, z - origin.1));
        let correction = ground.slope_correction();
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for i in 0..5 {
            for j in 0..5 {
                // Spans the chunk plus the talus reach on every side.
                let h = at((chunk_x << 4) - 16 + i * 12, (chunk_z << 4) - 16 + j * 12);
                lo = lo.min(h);
                hi = hi.max(h);
            }
        }
        if (hi - lo) * correction < 6.0 {
            return None;
        }
        let (x0, z0) = ((chunk_x << 4) - 20, (chunk_z << 4) - 20);
        let mut heights = [0.0; Self::SIDE * Self::SIDE];
        for (k, h) in heights.iter_mut().enumerate() {
            let (i, j) = ((k % Self::SIDE) as i32, (k / Self::SIDE) as i32);
            *h = at(x0 + i * Self::STEP, z0 + j * Self::STEP);
        }
        Some(Self {
            x0,
            z0,
            heights,
            highest: heights.iter().copied().fold(f64::MIN, f64::max),
            correction,
        })
    }

    /// Height at the lattice point nearest (x, z), clamped to the lattice.
    fn height(&self, x: i32, z: i32) -> f64 {
        let cell = |v: i32, v0: i32| {
            ((v - v0 + Self::STEP / 2).div_euclid(Self::STEP)).clamp(0, Self::SIDE as i32 - 1)
                as usize
        };
        self.heights[cell(z, self.z0) * Self::SIDE + cell(x, self.x0)]
    }

    /// How close a column at height `here` sits below a cliff: 1 at its foot, less
    /// further out, else 0.
    pub(crate) fn near(&self, x: i32, z: i32, here: f64) -> f64 {
        // Nothing around stands a wall's height above the column.
        if (self.highest - here) * self.correction < CLIFF_TAN * f64::from(TALUS_REACH[0].0) {
            return 0.0;
        }
        for (reach, near) in TALUS_REACH {
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let wall = self.height(x + dx * reach, z + dz * reach);
                if (wall - here) * self.correction >= CLIFF_TAN * f64::from(reach) {
                    return near;
                }
            }
        }
        0.0
    }
}

/// Soil that fallen rock may bury, even where a mapped meadow or wood painted it.
pub(crate) const TALUS_BURIES: &[Block] = &[GRASS_BLOCK, DIRT, COARSE_DIRT, PODZOL, MOSS_BLOCK];

/// Open ground that fallen rock lands on, as opposed to fields, towns, water and ice.
pub(crate) fn takes_talus(cover: u8) -> bool {
    matches!(
        cover,
        0 | LC_TREE_COVER | LC_SHRUBLAND | LC_GRASSLAND | LC_MOSS | LC_BARE
    )
}

/// Fallen rock below a cliff on open ground: gravel fans with andesite and cobblestone
/// boulders, thinning out away from the wall.
pub(crate) fn talus_palette(x: i32, z: i32, near: f64, cover: u8) -> Option<(Block, Block)> {
    if near <= 0.0 || !takes_talus(cover) || patch_noise(x, z, 7, SALT_TALUS) >= 0.8 * near {
        return None;
    }
    let n = patch_noise(x, z, 4, SALT_TALUS_ROCK);
    Some(if n < 0.55 {
        (GRAVEL, STONE)
    } else if n < 0.75 {
        (ANDESITE, STONE)
    } else if n < 0.9 {
        (COBBLESTONE, STONE)
    } else {
        (STONE, STONE)
    })
}

/// Land cover whose steep ground still carries soil between rock outcrops.
fn is_vegetated(cover: u8) -> bool {
    matches!(
        cover,
        0 | LC_TREE_COVER | LC_SHRUBLAND | LC_GRASSLAND | LC_CROPLAND | LC_MOSS
    )
}

/// Surface and under-block for ground steeper than about 27 degrees (`slope > 4`).
///
/// Cliffs and very steep faces are bedded rock with the odd gravel ledge. The
/// steep tier below them keeps soil and grass on vegetated slopes, broken by
/// outcrops, and turns bare slopes into stone with scree fans.
pub(crate) fn steep_palette(
    x: i32,
    z: i32,
    ground_y: i32,
    slope: i32,
    cover: u8,
) -> (Block, Block) {
    if slope > 6 {
        if slope <= 8 && patch_noise(x, z, 10, SALT_LEDGE) < 0.1 {
            return (GRAVEL, STONE);
        }
        let strata = Strata::at(x, z);
        let step = Strata::step(strata.block(ground_y), strata.tone(ground_y), 0);
        return (FACE_RAMP[step.min(TOP_RAMP_MAX)], STONE);
    }
    // A finer second field roughens the outcrop edges. Worn soil is a field of
    // its own, so it doesn't rim every patch of grass.
    let rock =
        0.7 * patch_noise(x, z, 14, SALT_SCREE) + 0.3 * patch_noise(x, z, 5, SALT_SCREE_EDGE);
    if is_vegetated(cover) && rock < 0.53 {
        if patch_noise(x, z, 6, SALT_WORN) < 0.08 {
            (COARSE_DIRT, DIRT)
        } else {
            (GRASS_BLOCK, DIRT)
        }
    } else if !is_vegetated(cover) && rock < 0.33 {
        (GRAVEL, STONE)
    } else {
        bare_rock_palette(x, z)
    }
}

/// Exposed rock between the soil patches of bare ground: stone with andesite
/// and gravel patches.
pub(crate) fn bare_rock_palette(x: i32, z: i32) -> (Block, Block) {
    let n = patch_noise(x, z, 9, SALT_BARE_ROCK);
    if n < 0.15 {
        (GRAVEL, STONE)
    } else if n < 0.35 {
        (ANDESITE, STONE)
    } else {
        (STONE, STONE)
    }
}

/// How snow settles on one column.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Snow {
    None,
    /// Partial cover over the ground below, at most this many eighths of a block
    /// deep, thinning out toward its edge.
    Layer(u8),
    /// Full cover: the surface itself is snow.
    Block,
}

/// The climatic snow line of a run and the band over which snow thickens.
#[derive(Clone, Copy)]
pub(crate) struct SnowLine {
    threshold_y: i32,
    band_blocks: f64,
    /// Direction of the nearer pole in world XZ, shortened toward the tropics where
    /// the sun stands high on every side.
    pole: (f64, f64),
}

impl SnowLine {
    /// Metres from where the snow line starts to where snow fully covers flat ground.
    const BAND_METRES: f64 = 200.0;
    /// Depth values are capped, so even the highest summit sheds snow off its cliffs.
    const MAX_DEPTH: f64 = 2.5;

    /// `rotation` is the world's clockwise rotation in degrees.
    pub(crate) fn new(ground: &Ground, lat: f64, rotation: f64) -> Self {
        let (sin, cos) = rotation.to_radians().sin_cos();
        let reach = ((lat.abs() - 10.0) / 15.0).clamp(0.0, 1.0) * lat.signum();
        Self {
            threshold_y: ground.snow_threshold_y(),
            band_blocks: (Self::BAND_METRES * ground.blocks_per_meter()).max(4.0),
            pole: (sin * reach, -cos * reach),
        }
    }

    /// How squarely a column faces the pole, from -1 facing the sun to 1 facing
    /// away from it. Counts fully on slopes of 27 to 45 degrees; flatter ground
    /// faces nowhere, and cliffs shed snow whichever way they face. Takes
    /// `Ground::slope_and_gradient`.
    pub(crate) fn shade(&self, (gx, gz): (f64, f64), slope: f64) -> f64 {
        if self.pole == (0.0, 0.0) || slope <= 0.0 {
            return 0.0;
        }
        let rise = gx.hypot(gz);
        if rise < 1e-9 {
            return 0.0;
        }
        // Ground falling away toward the pole faces it.
        let weight = (slope / 4.0).min(1.0) - ((slope - 8.0) / 4.0).clamp(0.0, 0.75);
        -(gx * self.pole.0 + gz * self.pole.1) / rise * weight
    }

    /// Height above the snow line in bands, with the line itself wandering by
    /// about a third of a band so it never reads as a contour.
    pub(crate) fn depth(&self, x: i32, z: i32, y: i32) -> f64 {
        match self.threshold_y {
            i32::MAX => f64::NEG_INFINITY,
            i32::MIN => Self::MAX_DEPTH,
            t => {
                let wobble = (patch_noise(x, z, 32, SALT_SNOW_LINE) - 0.5) * 0.6;
                ((f64::from(y) - f64::from(t)) / self.band_blocks + wobble).min(Self::MAX_DEPTH)
            }
        }
    }
}

/// Whether an ESA snow/ice cell is a glacier or snowfield rather than a
/// misclassified bright roof or salt flat: near the snow line, or in a cold climate.
pub(crate) fn is_plausible_ice(depth: f64, climate: Climate) -> bool {
    depth > -3.0 || matches!(climate, Climate::Tundra | Climate::IceCap | Climate::Boreal)
}

/// Below this depth no terrain shape can hold snow, so callers may skip it.
pub(crate) const SNOW_MIN_DEPTH: f64 = -1.5;

/// Score from which snow fully covers the ground.
const FULL_COVER: f64 = 0.6;
/// Most a hollow or a ridge moves the score.
const HOLLOW_MAX: f64 = 0.5;

/// Snow on a column `depth` bands above the snow line, from its unrounded slope,
/// convexity (`Ground::slope_and_gradient`, `Ground::convexity`) and `SnowLine::shade`.
///
/// Flat ground and hollows hold snow; steep faces shed it and wind strips ridges,
/// so cliffs stay dark with snow only on ledges and in gullies. Slopes facing away
/// from the sun keep it about a hundred metres lower, sunny ones lose it as much
/// higher. Every term is continuous, so the cover changes in patches rather than
/// flickering along each terrace step.
pub(crate) fn snow_cover(
    depth: f64,
    slope: f64,
    convexity: impl FnOnce() -> f64,
    shade: f64,
    x: i32,
    z: i32,
) -> Snow {
    if depth < SNOW_MIN_DEPTH {
        return Snow::None;
    }
    // Loses snow from 2 (about 14 degrees) and holds almost none past 10 (51).
    const SLOPE_TERM: [(f64, f64); 5] = [
        (2.0, 0.1),
        (4.0, 0.0),
        (6.0, -0.7),
        (8.0, -1.5),
        (10.0, -2.8),
    ];
    let slope_term = if slope <= SLOPE_TERM[0].0 {
        SLOPE_TERM[0].1
    } else {
        SLOPE_TERM
            .windows(2)
            .find(|w| slope <= w[1].0)
            .map_or(SLOPE_TERM[4].1, |w| {
                let t = (slope - w[0].0) / (w[1].0 - w[0].0);
                w[0].1 + t * (w[1].1 - w[0].1)
            })
    };
    let shade_term = 0.5 * shade.clamp(-1.0, 1.0);
    // Broad snowfields and bare stretches, with finer drift along their edges.
    let drift = (patch_noise(x, z, 40, SALT_SNOW_FIELD) - 0.5) * 0.5
        + (patch_noise(x, z, 9, SALT_SNOW_DRIFT) - 0.5) * 0.25;
    let base = depth + slope_term + shade_term + drift;
    // Convexity costs nine lookups, so only where it can change the outcome.
    let score = if base + HOLLOW_MAX < 0.0 || base - HOLLOW_MAX >= FULL_COVER {
        base
    } else {
        depth + slope_term + 0.25 * convexity().clamp(-2.0, 2.0) + shade_term + drift
    };
    if score >= FULL_COVER {
        Snow::Block
    } else if score >= 0.0 {
        Snow::Layer(1 + (score / FULL_COVER * 7.0) as u8)
    } else {
        Snow::None
    }
}

/// Eighths of a block of snow over a column whose unrounded height stands `rise`
/// above the bottom of its top block, so snow follows the true slope and fills
/// the whole-block terrace steps. `None`, a world without terrain, keeps the
/// thinnest cover.
pub(crate) fn snow_eighths(snow: Snow, rise: Option<f64>) -> u8 {
    let smooth = rise
        .filter(|r| (0.0..1.0).contains(r))
        .map_or(0, |r| (r * 8.0).round() as u8)
        .min(7);
    match snow {
        Snow::None => 0,
        Snow::Layer(most) => smooth.clamp(1, most.max(1)),
        Snow::Block => smooth,
    }
}

/// Depth given to an ESA glacier cell, so ice below the snow line still carries
/// patches of old snow.
pub(crate) fn glacier_depth(depth: f64) -> f64 {
    depth.max(-0.2)
}

/// Surface for flat and moderate glacier ground before snow is laid on it.
pub(crate) const GLACIER_ICE: (Block, Block) = (PACKED_ICE, PACKED_ICE);

const ICE_BLOCKS: [Block; 3] = [ICE, PACKED_ICE, BLUE_ICE];

pub(crate) fn is_ice(block: Block) -> bool {
    ICE_BLOCKS.contains(&block)
}

/// Lays `eighths` of snow over `top`, the block at `ground_y`, and swaps grass and
/// podzol for their snowy variants, which the game only does on a block update.
/// Those are blocks of their own, so a snowfield costs no per-block property
/// storage. Mapped ice turns to snow instead, as the game drops any layer on it.
pub(crate) fn place_snow(
    editor: &mut WorldEditor,
    x: i32,
    ground_y: i32,
    z: i32,
    top: Option<Block>,
    eighths: u8,
) {
    let Some(top) = top else {
        return;
    };
    let open = if eighths == 0 {
        is_ice(top) && !editor.block_exists_absolute(x, ground_y + 1, z)
    } else {
        let layer = SNOW_LAYERS[usize::from(eighths.min(7)) - 1];
        editor.set_block_if_absent_absolute(layer, x, ground_y + 1, z)
    };
    if !open {
        return;
    }
    let under = match top {
        GRASS_BLOCK => SNOWY_GRASS_BLOCK,
        PODZOL => SNOWY_PODZOL,
        _ if is_ice(top) => SNOW_BLOCK,
        _ => return,
    };
    editor.set_block_absolute(under, x, ground_y, z, Some(&[top]), None);
}

/// Cover class treated as a glacier by the surface pass.
pub(crate) fn is_glacier_cover(cover: u8) -> bool {
    cover == LC_SNOW_ICE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_strata_lays_every_bed_and_keeps_what_is_there() {
        use crate::coordinate_system::cartesian::XZBBox;
        use crate::coordinate_system::geographic::LLBBox;

        let xzbbox = XZBBox::rect_from_min_max(0, 0, 15, 15).unwrap();
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), &xzbbox, llbbox);
        editor.set_block_absolute(GRAVEL, 5, 20, 5, None, None);
        fill_strata(&mut editor, 5, 5, 0, 40, false);
        let strata = Strata::at(5, 5);
        for y in 0..=40 {
            // Tone is interpolated between samples, so only those match exactly.
            let want: &[Block] = if y == 20 {
                &[GRAVEL]
            } else if y % TONE_STEP == 0 {
                &[strata.shaded(y, 41 - y)]
            } else {
                &FACE_RAMP
            };
            assert!(
                editor.check_for_block_absolute(5, y, 5, Some(want), None),
                "y={y}"
            );
        }
    }

    #[test]
    fn faces_shade_from_light_to_dark_and_darken_toward_the_foot() {
        let mut counts = std::collections::HashMap::new();
        let (mut top_dark, mut foot_dark) = (0, 0);
        for x in 0..40 {
            for z in 0..40 {
                let strata = Strata::at(x, z);
                for y in 0..300 {
                    *counts.entry(strata.shaded(y, 0)).or_insert(0) += 1;
                }
                let (kind, tone) = (strata.block(60), strata.tone(60));
                top_dark += Strata::step(kind, tone, 0);
                foot_dark += Strata::step(kind, tone, 40);
            }
        }
        for block in FACE_RAMP {
            assert!(
                counts.get(&block).copied().unwrap_or(0) > 0,
                "{block:?} unused"
            );
        }
        let stone = f64::from(counts[&STONE]) / 480_000.0;
        assert!((0.35..0.65).contains(&stone), "{counts:?}");
        assert!(foot_dark > top_dark, "{foot_dark} vs {top_dark}");
    }

    #[test]
    fn streaks_mark_a_minority_of_sheer_faces() {
        let (mut deep, mut tuff, mut total) = (0, 0, 0);
        for x in 0..200 {
            for z in 0..200 {
                total += 1;
                match streak_block(x, z) {
                    Some(DEEPSLATE) => deep += 1,
                    Some(TUFF) => tuff += 1,
                    _ => {}
                }
            }
        }
        let share = f64::from(deep + tuff) / f64::from(total);
        assert!((0.15..0.3).contains(&share), "{share}");
        assert!(deep > 0 && deep < tuff, "{deep} deepslate, {tuff} tuff");
        // Down one streaked column the streak breaks off now and then.
        let (x, z) = (0..200)
            .flat_map(|x| (0..200).map(move |z| (x, z)))
            .find(|&(x, z)| streak_block(x, z).is_some())
            .unwrap();
        let runs = (0..200).filter(|&y| streak_runs(x, z, y)).count();
        assert!((80..200).contains(&runs), "{runs}");
    }

    #[test]
    fn talus_gathers_below_walls_on_open_ground() {
        let coverage = |near: f64| {
            (0..200)
                .flat_map(|x| (0..200).map(move |z| (x, z)))
                .filter(|&(x, z)| talus_palette(x, z, near, LC_GRASSLAND).is_some())
                .count() as f64
                / 40_000.0
        };
        let (foot, out) = (coverage(1.0), coverage(0.3));
        assert!(
            (0.7..0.9).contains(&foot) && (0.15..0.35).contains(&out),
            "{foot} {out}"
        );
        assert_eq!(coverage(0.0), 0.0);
        for cover in [crate::land_cover::LC_BUILT_UP, LC_CROPLAND, LC_SNOW_ICE] {
            assert!((0..50).all(|x| talus_palette(x, 3, 1.0, cover).is_none()));
        }

        // A 40-block wall rising from x = 24 on flat ground.
        let heights: Vec<Vec<f32>> = (0..64)
            .map(|_| (0..64).map(|x| if x >= 24 { 40.0 } else { 0.0 }).collect())
            .collect();
        let ground = Ground::new_elevation_test(heights, 64, 64);
        let near = |chunk_x: i32, x: i32| {
            let field = TalusField::new(&ground, chunk_x, 1, (0, 0)).expect("relief");
            field.near(x, 30, ground.level_exact(XZPoint::new(x, 30)))
        };
        assert_eq!(near(1, 21), 1.0);
        assert_eq!(near(1, 16), 0.6);
        assert_eq!(near(0, 10), 0.3);
        assert_eq!(near(0, 2), 0.0);
        // On top of the wall the ground only drops away.
        assert_eq!(near(2, 40), 0.0);
        // No relief, no lattice.
        let flat = Ground::new_elevation_test(vec![vec![0.0; 64]; 64], 64, 64);
        assert!(TalusField::new(&flat, 1, 1, (0, 0)).is_none());
    }

    #[test]
    fn patch_noise_shares_match_their_thresholds() {
        let samples: Vec<f64> = (0..300)
            .flat_map(|x| (0..300).map(move |z| patch_noise(x * 3, z * 3, 16, 0xABCD)))
            .collect();
        for threshold in [0.1, 0.2, 0.5, 0.8] {
            let share =
                samples.iter().filter(|&&v| v < threshold).count() as f64 / samples.len() as f64;
            assert!(
                (share - threshold).abs() < 0.05,
                "{share} of samples below {threshold}"
            );
        }
    }

    #[test]
    fn strata_are_layered_by_altitude() {
        let strata = Strata::at(100, 200);
        // Beds vary in thickness instead of repeating every four blocks.
        let mut runs = Vec::new();
        let mut run = 1;
        for y in 0..400 {
            if strata.layer(y + 1) == strata.layer(y) {
                run += 1;
            } else {
                runs.push(run);
                run = 1;
            }
        }
        let (min, max) = (runs.iter().min().unwrap(), runs.iter().max().unwrap());
        assert!(*min <= 3 && *max >= 7, "bed thicknesses {runs:?}");
        // Walking up the column gives the same beds as looking each height up.
        let mut cursor = 0;
        for y in -64..400 {
            assert_eq!(strata.layer_walk(y, &mut cursor), strata.layer(y), "y={y}");
        }
        // Layers keep their order up the column: no bed comes back once passed.
        let layers: Vec<i32> = (0..400).map(|y| strata.layer(y)).collect();
        assert!(layers.windows(3).all(|w| w[2] >= w[0]), "{layers:?}");
        // One bed is several blocks thick, so neighbouring heights mostly agree.
        let same = (0..200)
            .filter(|&y| strata.block(y) == strata.block(y + 1))
            .count();
        assert!(same > 130, "{same} of 200 steps stayed in their bed");
        let kinds: std::collections::HashSet<_> = (0..400).map(|y| strata.block(y)).collect();
        assert!(kinds.contains(&STONE) && kinds.contains(&ANDESITE) && kinds.contains(&TUFF));
    }

    #[test]
    fn cliffs_are_rock_not_deepslate() {
        for x in 0..64 {
            for z in 0..64 {
                let (top, under) = steep_palette(x, z, 120, 12, LC_TREE_COVER);
                assert!(
                    matches!(top, STONE | ANDESITE | COBBLESTONE | TUFF),
                    "{top:?}"
                );
                assert_eq!(under, STONE);
            }
        }
    }

    #[test]
    fn steep_vegetated_slopes_keep_soil_between_outcrops() {
        let (mut soil, mut rock) = (0, 0);
        for x in 0..128 {
            for z in 0..128 {
                match steep_palette(x, z, 90, 5, LC_TREE_COVER).0 {
                    GRASS_BLOCK | COARSE_DIRT => soil += 1,
                    _ => rock += 1,
                }
            }
        }
        let share = f64::from(soil) / f64::from(soil + rock);
        assert!((0.45..0.75).contains(&share), "soil share {share}");
    }

    #[test]
    fn snow_sticks_to_flat_ground_and_hollows_not_cliffs() {
        let flat = snow_cover(1.5, 1.0, || 0.0, 0.0, 3, 4);
        assert_eq!(flat, Snow::Block);
        let mut cliff_blocks = 0;
        for x in 0..64 {
            for z in 0..64 {
                if snow_cover(1.5, 12.0, || 0.0, 1.0, x, z) == Snow::Block {
                    cliff_blocks += 1;
                }
            }
        }
        assert_eq!(
            cliff_blocks, 0,
            "a sheer face never carries full snow cover"
        );
        // Far below the line nothing settles, whatever the shape.
        assert_eq!(snow_cover(-2.0, 0.0, || 2.0, 1.0, 0, 0), Snow::None);
    }

    #[test]
    fn nothing_settles_below_the_minimum_depth() {
        for x in 0..96 {
            for z in 0..96 {
                assert_eq!(
                    snow_cover(SNOW_MIN_DEPTH, 0.0, || 2.0, 1.0, x, z),
                    Snow::None
                );
            }
        }
    }

    #[test]
    fn shaded_slopes_hold_more_snow_than_sunny_ones() {
        let covered = |shade: f64| {
            (0..64)
                .flat_map(|x| (0..64).map(move |z| (x, z)))
                .filter(|&(x, z)| snow_cover(0.2, 4.0, || 0.0, shade, x, z) != Snow::None)
                .count()
        };
        assert!(covered(1.0) > covered(0.0) && covered(0.0) > covered(-1.0));
    }

    #[test]
    fn slopes_facing_the_pole_are_shaded() {
        // Rising to the south, so the ground faces north.
        let heights: Vec<Vec<f32>> = (0..32).map(|z| vec![z as f32 * 0.5; 32]).collect();
        let ground = Ground::new_elevation_test(heights, 32, 32);
        let at = XZPoint::new(16, 16);
        let fall = ground.slope_and_gradient(at).1;
        let shade = |lat: f64, rotation: f64, slope: f64| {
            SnowLine::new(&ground, lat, rotation).shade(fall, slope)
        };
        assert!((shade(47.0, 0.0, 4.0) - 1.0).abs() < 1e-9);
        assert!((shade(-47.0, 0.0, 4.0) + 1.0).abs() < 1e-9);
        assert!((shade(47.0, 0.0, 2.0) - 0.5).abs() < 1e-9);
        assert!((shade(47.0, 0.0, 8.0) - 1.0).abs() < 1e-9);
        assert!((shade(47.0, 0.0, 14.0) - 0.25).abs() < 1e-9);
        assert_eq!(shade(3.0, 0.0, 4.0), 0.0);
        // A quarter turn clockwise points north along +x, across this slope.
        assert!(shade(47.0, 90.0, 4.0).abs() < 1e-9);
        let west: Vec<Vec<f32>> = (0..32)
            .map(|_| (0..32).map(|x| -(x as f32) * 0.5).collect())
            .collect();
        let ground = Ground::new_elevation_test(west, 32, 32);
        let line = SnowLine::new(&ground, 47.0, 90.0);
        let fall = ground.slope_and_gradient(at).1;
        assert!((line.shade(fall, 4.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn convexity_is_read_only_where_it_decides() {
        let reads = std::cell::Cell::new(0);
        let hollow = || {
            reads.set(reads.get() + 1);
            2.0
        };
        assert_eq!(snow_cover(2.5, 0.0, hollow, 0.0, 0, 0), Snow::Block);
        assert_eq!(snow_cover(-1.4, 12.0, hollow, 0.0, 0, 0), Snow::None);
        assert_eq!(reads.get(), 0);
        for x in 0..64 {
            let lazy = snow_cover(0.3, 2.0, hollow, 0.0, x, 0);
            let eager = snow_cover(0.3, 2.0, || 2.0, 0.0, x, 0);
            assert_eq!(lazy, eager);
        }
    }

    #[test]
    fn snow_lies_on_the_ground_and_takes_the_place_of_ice() {
        use crate::coordinate_system::cartesian::XZBBox;
        use crate::coordinate_system::geographic::LLBBox;

        let xzbbox = XZBBox::rect_from_min_max(0, 0, 15, 15).unwrap();
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor = WorldEditor::new(std::env::temp_dir(), &xzbbox, llbbox);
        for (x, ground) in [(1, GRASS_BLOCK), (2, PACKED_ICE), (3, STONE)] {
            editor.set_block_absolute(ground, x, 10, 1, None, None);
            place_snow(&mut editor, x, 10, 1, Some(ground), 3);
        }
        // Something already standing on the ground keeps its place.
        editor.set_block_absolute(PACKED_ICE, 4, 10, 1, None, None);
        editor.set_block_absolute(STONE, 4, 11, 1, None, None);
        place_snow(&mut editor, 4, 10, 1, Some(PACKED_ICE), 3);
        let at =
            |x: i32, y: i32, b: Block| editor.check_for_block_absolute(x, y, 1, Some(&[b]), None);
        assert!(at(1, 10, SNOWY_GRASS_BLOCK) && at(1, 11, SNOW_LAYERS[2]));
        assert!(at(2, 10, SNOW_BLOCK) && at(2, 11, SNOW_LAYERS[2]));
        assert!(at(3, 10, STONE) && at(3, 11, SNOW_LAYERS[2]));
        assert!(at(4, 10, PACKED_ICE) && at(4, 11, STONE));
    }

    #[test]
    fn snow_depth_follows_the_unrounded_surface() {
        // A terrace step rises one block over eight columns; the snow on it fills in
        // the step, eighth by eighth.
        let depths: Vec<u8> = (0..8)
            .map(|i| snow_eighths(Snow::Block, Some(f64::from(i) / 8.0)))
            .collect();
        assert_eq!(depths, [0, 1, 2, 3, 4, 5, 6, 7]);
        // Partial cover keeps at least a dusting and thins out toward its edge.
        assert_eq!(snow_eighths(Snow::Layer(7), Some(0.0)), 1);
        assert_eq!(snow_eighths(Snow::Layer(2), Some(0.9)), 2);
        assert_eq!(snow_eighths(Snow::Layer(7), None), 1);
        assert_eq!(snow_eighths(Snow::Block, None), 0);
        // Ground flattened away from the terrain keeps the thinnest cover.
        assert_eq!(snow_eighths(Snow::Layer(7), Some(3.0)), 1);
        assert_eq!(snow_eighths(Snow::None, Some(0.5)), 0);
    }

    #[test]
    fn a_disabled_snow_line_never_snows() {
        let line = SnowLine {
            threshold_y: i32::MAX,
            band_blocks: 10.0,
            pole: (0.0, -1.0),
        };
        assert_eq!(
            snow_cover(line.depth(0, 0, 5000), 0.0, || 2.0, 1.0, 0, 0),
            Snow::None
        );
    }
}
