//! Banded sedimentary rock for arid canyon country.
//!
//! Bands are keyed to Minecraft Y and shared by every column, so they line up
//! across the whole bbox. The sequence follows the Grand Canyon column, which
//! is representative of arid sedimentary country generally.

use crate::block_definitions::*;
use crate::climate::patch_pick;
use crate::ground_generation::value_noise_01;

/// Members stay within Oklab dE ~0.14, close enough to cluster not band.
struct Band {
    top: f64,
    blocks: [Block; 3],
    /// Cumulative 0..12 weights picking between `blocks`.
    cuts: [u64; 2],
}

/// Bottom to top, as fractions of the bbox relief.
const BANDS: [Band; 9] = [
    // Vishnu schist and its granite veins: near-black inner gorge.
    Band {
        top: 0.18,
        blocks: [COBBLED_DEEPSLATE, DEEPSLATE, GRANITE],
        cuts: [7, 10],
    },
    // Tapeats sandstone: dark brown, thin-bedded.
    Band {
        top: 0.26,
        blocks: [BROWN_TERRACOTTA, TERRACOTTA, ORANGE_TERRACOTTA],
        cuts: [7, 10],
    },
    // Bright Angel shale: the grey-green bench. Tuff is the only vanilla block
    // with that cast.
    Band {
        top: 0.36,
        blocks: [TUFF, GRAY_TERRACOTTA, LIGHT_GRAY_TERRACOTTA],
        cuts: [7, 10],
    },
    // Muav/Temple Butte limestone: grey to cream.
    Band {
        top: 0.46,
        blocks: [LIGHT_GRAY_TERRACOTTA, TUFF, WHITE_TERRACOTTA],
        cuts: [6, 10],
    },
    // Redwall limestone: grey rock stained red from above, so unstained grey
    // still shows through under the overhangs.
    Band {
        top: 0.62,
        blocks: [RED_TERRACOTTA, TERRACOTTA, LIGHT_GRAY_TERRACOTTA],
        cuts: [7, 10],
    },
    // Supai group: red siltstone with tan sandstone ledges.
    Band {
        top: 0.76,
        blocks: [ORANGE_TERRACOTTA, TERRACOTTA, RED_TERRACOTTA],
        cuts: [7, 10],
    },
    // Hermit shale: the deepest red in the column.
    Band {
        top: 0.84,
        blocks: [RED_TERRACOTTA, TERRACOTTA, BROWN_TERRACOTTA],
        cuts: [7, 11],
    },
    // Coconino sandstone: the pale cross-bedded cliff below the rim.
    Band {
        top: 0.92,
        blocks: [SMOOTH_SANDSTONE, WHITE_TERRACOTTA, SANDSTONE],
        cuts: [7, 11],
    },
    // Kaibab/Toroweap: cream rimrock.
    Band {
        top: 1.01,
        blocks: [WHITE_TERRACOTTA, LIGHT_GRAY_TERRACOTTA, SANDSTONE],
        cuts: [6, 10],
    },
];

/// What a column's own land cover says about vegetation, which decides how much
/// soil the rock-desert floor keeps. The bbox is classified as a whole, but the
/// cover is measured per cell, so a shrubland pocket inside Moab still gets
/// ground its scrub can root in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FloorCover {
    /// Measured bare: the rock desert proper, soil only in the sand-flat pockets.
    Bare,
    /// Grass, shrub or moss: soil dominant, with rock and sand still showing
    /// through, so the scatter passes have somewhere to put tussocks and scrub.
    Sparse,
    /// Tree cover, or a canopy measurement: soil dominant enough that the trees
    /// the data promises have somewhere to stand.
    Wooded,
}

/// Flat rock-desert ground. A coarse patch picks the formation, a finer one
/// picks within it.
///
/// Only [`FloorCover::Bare`] is the full rock palette. The vegetated levels keep
/// soil, because the surface is what gates every later plant and tree pass: laying
/// rock on a cell the land cover calls grassland silently deletes its vegetation.
pub fn desert_floor(x: i32, z: i32, cover: FloorCover) -> (Block, Block) {
    let within = patch_pick(x, z, 12, 10);
    if cover == FloorCover::Wooded {
        return match within {
            0..=7 => (COARSE_DIRT, DIRT),
            _ => (TERRACOTTA, SANDSTONE),
        };
    }
    if cover == FloorCover::Sparse {
        // Rock members drawn from the same facies as the bare floor below, so a
        // shrubland patch reads as the same country and not a different formation.
        let facies = value_noise_01(x, z, 56);
        return match within {
            0..=5 => (COARSE_DIRT, DIRT),
            6 => (DIRT, DIRT),
            7..=8 if facies < 0.60 => (TERRACOTTA, SANDSTONE),
            7..=8 => (RED_SAND, SANDSTONE),
            _ if facies < 0.44 => (ORANGE_TERRACOTTA, SANDSTONE),
            _ => (SANDSTONE, SANDSTONE),
        };
    }
    // Smoothstep noise, not a lattice hash: at formation scale a hash draws
    // visible axis-aligned rectangles across the desert floor.
    let facies = value_noise_01(x, z, 56);
    if facies < 0.44 {
        // Red rock.
        match within {
            0..=5 => (ORANGE_TERRACOTTA, SANDSTONE),
            6..=8 => (TERRACOTTA, SANDSTONE),
            _ => (RED_TERRACOTTA, SANDSTONE),
        }
    } else if facies < 0.60 {
        // Pale slickrock.
        match within {
            0..=5 => (SANDSTONE, SANDSTONE),
            6..=8 => (SMOOTH_SANDSTONE, SANDSTONE),
            _ => (WHITE_TERRACOTTA, SANDSTONE),
        }
    } else {
        // Sand flats, with soil pockets kept so the scrub pass still has ground
        // it can root in -- nothing grows on red sand.
        match within {
            0..=4 => (RED_SAND, SANDSTONE),
            5..=7 => (COARSE_DIRT, DIRT),
            _ => (TERRACOTTA, SANDSTONE),
        }
    }
}

/// Below this the sequence is too compressed to read.
const MIN_BANDED_RELIEF: i32 = 40;

/// Per-column constants, so the per-block lookup stays integer math.
#[derive(Clone, Copy)]
pub struct Column {
    /// Y offset of the band boundaries here, so they are not razor-level.
    wobble: i32,
    /// Which member of a band this column favours.
    pick: u64,
    /// Desaturates to plain terracotta, or the wall reads as a rainbow.
    weathered: bool,
}

/// Band table anchored to the bbox relief.
pub struct Strata {
    y_min: i32,
    span: f64,
    banded: bool,
}

impl Strata {
    pub fn new(y_min: i32, y_max: i32) -> Self {
        let relief = y_max - y_min;
        Self {
            y_min,
            span: (relief.max(1)) as f64,
            banded: relief >= MIN_BANDED_RELIEF,
        }
    }

    /// Per-column constants. Two hashes; call once per column, not per block.
    #[inline]
    pub fn column(&self, x: i32, z: i32) -> Column {
        Column {
            // +-3 blocks, smoothly varying, so band contours undulate instead of
            // stepping along a lattice.
            wobble: ((value_noise_01(x, z, 96) - 0.5) * 7.0) as i32,
            // Clustered: members of a band differ enough that a per-block draw
            // reads as speckle rather than as rock texture.
            pick: patch_pick(x, z, 6, 12),
            weathered: value_noise_01(x, z, 48) < 0.48,
        }
    }

    /// Rock at this height in this column.
    #[inline]
    pub fn block(&self, col: &Column, y: i32) -> Block {
        let f = ((y + col.wobble - self.y_min) as f64 / self.span).clamp(0.0, 1.0);
        let band = if self.banded {
            let mut i = 0;
            while i < BANDS.len() - 1 && f >= BANDS[i].top {
                i += 1;
            }
            &BANDS[i]
        } else {
            &BANDS[5]
        };
        // The weathering pass leaves the darkest band alone: desaturating the
        // inner gorge to terracotta would erase the strongest contrast there is.
        if col.weathered && band.blocks[0] != COBBLED_DEEPSLATE {
            return TERRACOTTA;
        }
        if col.pick < band.cuts[0] {
            band.blocks[0]
        } else if col.pick < band.cuts[1] {
            band.blocks[1]
        } else {
            band.blocks[2]
        }
    }

    /// Y of the next band boundary at or above `y`, so a column can be filled in
    /// one run per band instead of one write per block.
    #[inline]
    pub fn band_top(&self, col: &Column, y: i32) -> i32 {
        if !self.banded {
            return i32::MAX;
        }
        let f = ((y + col.wobble - self.y_min) as f64 / self.span).clamp(0.0, 1.0);
        for b in BANDS.iter() {
            if f < b.top {
                // Truncating the boundary can land it on `y` itself, which would
                // stall a fill loop; the band is always at least one block tall.
                let top = self.y_min + (b.top * self.span) as i32 - col.wobble;
                return top.max(y + 1);
            }
        }
        i32::MAX
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strata() -> Strata {
        Strata::new(0, 200)
    }

    #[test]
    fn bands_run_dark_at_the_bottom_and_pale_at_the_rim() {
        let s = strata();
        let mut col = s.column(10, 10);
        col.weathered = false;
        assert!(matches!(
            s.block(&col, 5),
            COBBLED_DEEPSLATE | DEEPSLATE | GRANITE
        ));
        assert!(matches!(
            s.block(&col, 199),
            WHITE_TERRACOTTA | LIGHT_GRAY_TERRACOTTA | SANDSTONE
        ));
    }

    #[test]
    fn a_column_is_constant_within_a_band() {
        let s = strata();
        let col = s.column(3, 7);
        let a = s.block(&col, 100);
        let top = s.band_top(&col, 100);
        for y in 100..top.min(140) {
            assert_eq!(s.block(&col, y), a, "y {y} differs inside its band");
        }
    }

    #[test]
    fn band_top_advances_past_its_band() {
        let s = strata();
        let mut col = s.column(1, 1);
        col.weathered = false;
        let top = s.band_top(&col, 50);
        assert!(top > 50);
        assert_ne!(s.block(&col, top), s.block(&col, 50));
    }

    #[test]
    fn shallow_relief_skips_banding() {
        let s = Strata::new(0, 10);
        let mut col = s.column(4, 4);
        col.weathered = false;
        assert_eq!(s.block(&col, 0), s.block(&col, 9));
        assert_eq!(s.band_top(&col, 0), i32::MAX);
    }

    #[test]
    fn band_top_always_advances_so_a_fill_loop_terminates() {
        for (lo, hi) in [(0, 200), (-64, 320), (5, 45), (-2032, 100)] {
            let s = Strata::new(lo, hi);
            for (x, z) in [(0, 0), (13, 91), (-7, 5), (1000, -1000)] {
                let col = s.column(x, z);
                for y in lo..hi {
                    let top = s.band_top(&col, y);
                    assert!(top > y, "band_top({y}) = {top} in {lo}..{hi} at ({x},{z})");
                }
            }
        }
    }

    #[test]
    fn bands_never_use_a_falling_block() {
        // A band is placed on cliff faces; gravity blocks there drop out and
        // leave holes the first time a player loads the chunk.
        for b in BANDS.iter() {
            for blk in b.blocks.iter() {
                assert!(
                    !matches!(*blk, SAND | RED_SAND | GRAVEL),
                    "{} falls and cannot be a band material",
                    blk.name()
                );
            }
        }
    }

    #[test]
    fn every_band_block_stays_in_the_narrow_id_range() {
        for b in BANDS.iter() {
            for blk in b.blocks.iter() {
                assert!(
                    blk.id() < BYTE_ID_LIMIT,
                    "{} would widen every section it lands in",
                    blk.name()
                );
            }
        }
        assert!(TERRACOTTA.id() < BYTE_ID_LIMIT);
    }

    #[test]
    fn desert_floor_stays_in_the_narrow_id_range_and_varies() {
        let mut seen = std::collections::HashSet::new();
        for x in 0..200 {
            for z in 0..200 {
                let cover = match (x + z) % 3 {
                    0 => FloorCover::Wooded,
                    1 => FloorCover::Sparse,
                    _ => FloorCover::Bare,
                };
                let (surf, under) = desert_floor(x, z, cover);
                assert!(surf.id() < BYTE_ID_LIMIT && under.id() < BYTE_ID_LIMIT);
                seen.insert(surf.id());
            }
        }
        assert!(
            seen.len() >= 5,
            "floor should use several materials: {seen:?}"
        );
    }

    #[test]
    fn weathering_covers_a_minority_of_columns() {
        let s = strata();
        // Sample wide enough to cover many noise lattice cells, or the estimate
        // is dominated by a handful of corner values.
        let n = 400;
        let weathered = (0..n)
            .flat_map(|x| (0..n).map(move |z| (x * 5, z * 5)))
            .filter(|&(x, z)| s.column(x, z).weathered)
            .count();
        let frac = weathered as f64 / (n as f64 * n as f64);
        // Vanilla badlands desaturates roughly this share of the wall; it is what
        // stops banded rock reading as a rainbow.
        assert!((0.25..0.55).contains(&frac), "weathered fraction {frac}");
    }
}
