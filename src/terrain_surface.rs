//! Natural surface materials: rock on steep ground, snow by altitude and terrain
//! shape, and the ground left over in built-up land.
//!
//! Every choice reads smooth, salted noise fields keyed on world coordinates, so
//! materials form patches and layers tens of blocks across instead of per-block
//! speckle, and tiles stitch without seams.

use crate::block_definitions::{
    Block, BlockWithProperties, ANDESITE, COARSE_DIRT, DIRT, GRASS_BLOCK, GRAVEL, PACKED_ICE,
    PODZOL, SNOW_LAYER, STONE, TUFF,
};
use crate::climate::Climate;
use crate::ground::Ground;
use crate::ground_generation::{patch_noise, value_noise_salted};
use crate::land_cover::{
    LC_CROPLAND, LC_GRASSLAND, LC_MOSS, LC_SHRUBLAND, LC_SNOW_ICE, LC_TREE_COVER,
};
use crate::world_editor::WorldEditor;

const SALT_STRATA_WARP: u32 = 0x5157_A7A1;
const SALT_LEDGE: u32 = 0x1ED6_E5A1;
const SALT_SCREE: u32 = 0x5C4E_E0B2;
const SALT_SCREE_EDGE: u32 = 0x5C4E_ED6E;
const SALT_WORN: u32 = 0x0B0A_4E11;
const SALT_BARE_ROCK: u32 = 0xBA4E_40C3;
const SALT_SNOW_LINE: u32 = 0x5A0E_11A4;
const SALT_SNOW_DRIFT: u32 = 0xD41F_7B05;
const SALT_SNOW_FIELD: u32 = 0xF1E1_D5A0;
const SALT_YARD: u32 = 0x7A4D_0C16;

/// Rock strata of one column: horizontal layers that bend gently across the
/// landscape, so cliff faces and the steps of a stepped slope show bedding
/// instead of noise.
#[derive(Clone, Copy)]
pub(crate) struct Strata {
    warp: f64,
}

impl Strata {
    const LAYER_BLOCKS: f64 = 4.0;

    pub(crate) fn at(x: i32, z: i32) -> Self {
        Self {
            warp: (value_noise_salted(x, z, 48, SALT_STRATA_WARP) - 0.5) * 7.0,
        }
    }

    /// Layers repeat with altitude alone, so one bed runs unbroken across a whole face.
    pub(crate) fn block(self, y: i32) -> Block {
        let layer = ((f64::from(y) + self.warp) / Self::LAYER_BLOCKS).floor() as i32;
        match crate::land_cover::coord_hash(layer, 0x57A7) % 100 {
            0..=69 => STONE,
            70..=89 => ANDESITE,
            _ => TUFF,
        }
    }
}

/// Fills `y_min..=y_max` of a rock column with its strata, keeping whatever is
/// already there.
pub(crate) fn fill_strata(editor: &mut WorldEditor, x: i32, z: i32, y_min: i32, y_max: i32) {
    let strata = Strata::at(x, z);
    for y in y_min..=y_max {
        editor.set_block_if_absent_absolute(strata.block(y), x, y, z);
    }
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
        return (Strata::at(x, z).block(ground_y), STONE);
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

/// Ground in built-up land that no mapped feature claimed: yards and verges, so
/// the local natural surface with a few worn patches, not paving.
pub(crate) fn built_up_palette(climate: Climate, x: i32, z: i32) -> (Block, Block) {
    if let Some(p) = climate.surface_palette(LC_GRASSLAND, x, z) {
        return p;
    }
    if patch_noise(x, z, 8, SALT_YARD) < 0.12 {
        (COARSE_DIRT, DIRT)
    } else {
        (GRASS_BLOCK, DIRT)
    }
}

/// How snow settles on one column.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Snow {
    None,
    /// A thin layer over the ground below.
    Layer,
    /// Full cover: the surface itself is snow.
    Block,
}

/// The climatic snow line of a run and the band over which snow thickens.
#[derive(Clone, Copy)]
pub(crate) struct SnowLine {
    threshold_y: i32,
    band_blocks: f64,
}

impl SnowLine {
    /// Metres from where the snow line starts to where snow fully covers flat ground.
    const BAND_METRES: f64 = 200.0;
    /// Depth values are capped, so even the highest summit sheds snow off its cliffs.
    const MAX_DEPTH: f64 = 2.5;

    pub(crate) fn new(ground: &Ground) -> Self {
        Self {
            threshold_y: ground.snow_threshold_y(),
            band_blocks: (Self::BAND_METRES * ground.blocks_per_meter()).max(4.0),
        }
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
pub(crate) const SNOW_MIN_DEPTH: f64 = -1.0;

/// Snow on a column `depth` bands above the snow line, from its unrounded slope
/// and convexity (`Ground::slope_exact`, `Ground::convexity`).
///
/// Flat ground and hollows hold snow; steep faces shed it and wind strips ridges,
/// so cliffs stay dark with snow only on ledges and in gullies. Every term is
/// continuous, so the cover changes in patches rather than flickering along
/// each terrace step.
pub(crate) fn snow_cover(depth: f64, slope: f64, convexity: f64, x: i32, z: i32) -> Snow {
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
    let hollow_term = 0.25 * convexity.clamp(-2.0, 2.0);
    // Broad snowfields and bare stretches, with finer drift along their edges.
    let drift = (patch_noise(x, z, 40, SALT_SNOW_FIELD) - 0.5) * 0.5
        + (patch_noise(x, z, 9, SALT_SNOW_DRIFT) - 0.5) * 0.25;
    let score = depth + slope_term + hollow_term + drift;
    if score >= 0.6 {
        Snow::Block
    } else if score >= 0.0 {
        Snow::Layer
    } else {
        Snow::None
    }
}

/// Depth given to an ESA glacier cell, so ice below the snow line still carries
/// patches of old snow.
pub(crate) fn glacier_depth(depth: f64) -> f64 {
    depth.max(-0.2)
}

/// Surface for flat and moderate glacier ground before snow is laid on it.
pub(crate) const GLACIER_ICE: (Block, Block) = (PACKED_ICE, PACKED_ICE);

/// Lays a snow layer on `ground_y` and marks grass and podzol under it as snowy,
/// which the game only does on a block update.
pub(crate) fn place_snow_layer(editor: &mut WorldEditor, x: i32, ground_y: i32, z: i32) {
    if editor.block_exists_absolute(x, ground_y + 1, z) {
        return;
    }
    editor.set_block_if_absent_absolute(SNOW_LAYER, x, ground_y + 1, z);
    for soil in [GRASS_BLOCK, PODZOL] {
        if editor.check_for_block_absolute(x, ground_y, z, Some(&[soil]), None) {
            editor.set_block_with_properties_absolute(
                BlockWithProperties::new(soil, Some(fastnbt::nbt!({ "snowy": "true" }))),
                x,
                ground_y,
                z,
                Some(&[soil]),
                None,
            );
        }
    }
}

/// Cover class treated as a glacier by the surface pass.
pub(crate) fn is_glacier_cover(cover: u8) -> bool {
    cover == LC_SNOW_ICE
}

#[cfg(test)]
mod tests {
    use super::*;

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
                assert!(matches!(top, STONE | ANDESITE | TUFF), "{top:?}");
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
        let flat = snow_cover(1.5, 1.0, 0.0, 3, 4);
        assert_eq!(flat, Snow::Block);
        let mut cliff_blocks = 0;
        for x in 0..64 {
            for z in 0..64 {
                if snow_cover(1.5, 12.0, 0.0, x, z) == Snow::Block {
                    cliff_blocks += 1;
                }
            }
        }
        assert_eq!(
            cliff_blocks, 0,
            "a sheer face never carries full snow cover"
        );
        // Far below the line nothing settles, whatever the shape.
        assert_eq!(snow_cover(-2.0, 0.0, 2.0, 0, 0), Snow::None);
    }

    #[test]
    fn a_disabled_snow_line_never_snows() {
        let line = SnowLine {
            threshold_y: i32::MAX,
            band_blocks: 10.0,
        };
        assert_eq!(
            snow_cover(line.depth(0, 0, 5000), 0.0, 2.0, 0, 0),
            Snow::None
        );
    }
}
