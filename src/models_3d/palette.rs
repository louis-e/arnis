//! Color → Block matching for voxelized 3D models.
//!
//! The palette itself lives in `crate::block_palette` (shared with the
//! building colour pipeline); model voxelization uses the full palette.

pub(crate) use crate::block_palette::{closest_block, closest_blocks};
