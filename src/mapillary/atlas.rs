//! The block atlas budget the facade photo panels are sized against.
//!
//! Every panel texture the resource pack carries is stitched into the game's
//! block atlas, whichever source the panel came from (`displays.rs` for the
//! Mapillary photographs, `building_facades` for the preset set). The budget
//! is process-wide because both sources write into the same atlas and it is
//! one limit for the two of them, set once per generation from the settings
//! before anything measures a panel against it.

use std::sync::atomic::{AtomicU64, Ordering};

/// The atlas side the panels are budgeted against, in pixels.
///
/// Minecraft stitches every block texture into one image and grows it in powers
/// of two up to the largest texture the driver actually accepts. Overflowing it
/// is not a soft failure: the game drops the whole pack, switches off the
/// player's other resource packs with it, and saves that to options.txt.
///
/// 8192 is the safe floor. The game's own stated minimum is an OpenGL 4.4 GPU,
/// and that specification requires at least 16384, so `ATLAS_SIDE_HIGH` is
/// defensible; it is not the default because 16384 x 8192 is 716 MB of atlas
/// VRAM against a stated 2 GB minimum, which is the player's call and not ours.
pub const ATLAS_SIDE_STANDARD: u32 = 8192;
pub const ATLAS_SIDE_HIGH: u32 = 16384;

/// Usable pixels in an atlas of `side`, after packing waste.
///
/// The six tenths is a packing allowance, not room for vanilla: every vanilla
/// block sprite together is 0.44 Mpx, which is under one per cent of an 8192
/// atlas. Measured packing efficiency is 84 to 95 per cent, so this is
/// conservative by a third, deliberately: from 1.21.11 the player's
/// anisotropic filtering setting pads every sprite and can add 29 per cent to
/// the same pack, and a pack that stitches here must still stitch there.
const fn atlas_budget_for(side: u32) -> u64 {
    (side as u64) * (side as u64) * 6 / 10
}

/// The budget this run is working to, set once from the settings.
static ATLAS_BUDGET: AtomicU64 = AtomicU64::new(atlas_budget_for(ATLAS_SIDE_STANDARD));

/// Chooses the atlas the panels are budgeted against for this generation.
pub fn set_atlas_side(side: u32) {
    ATLAS_BUDGET.store(atlas_budget_for(side), Ordering::Relaxed);
}

/// Atlas pixels the panels of this run may fill between them.
pub(super) fn atlas_budget() -> u64 {
    ATLAS_BUDGET.load(Ordering::Relaxed)
}

/// Lowest resolution the budget rule falls back to, in pixels per block.
pub(super) const MIN_PX_PER_BLOCK: u32 = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_is_six_tenths_of_the_atlas() {
        assert_eq!(atlas_budget_for(ATLAS_SIDE_STANDARD), 8192 * 8192 * 6 / 10);
        assert_eq!(atlas_budget_for(ATLAS_SIDE_HIGH), 16384 * 16384 * 6 / 10);
    }
}
