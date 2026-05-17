//! 3D Model Repository (3DMR): fetch + voxelize + place glTF models for `3dmr=<id>` elements.

mod client;
pub(crate) mod palette;
mod placement;
pub(crate) mod voxelize;

pub use placement::{place_three_dmr_models, prescan};
