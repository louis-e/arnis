//! 3D Model Repository (3DMR): fetch + voxelize + place glTF models for `3dmr=<id>` elements.

pub(crate) mod client;
mod placement;

pub use placement::{place_three_dmr_models, prescan, PrescanResult};
