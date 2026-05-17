//! 3D model sources that substitute generated buildings: the 3D Model
//! Repository (3DMR, glTF) and Wikimedia Commons via Wikidata P4896 (STL).
//! Both pipelines share the voxelizer and color palette in this module.

pub(crate) mod palette;
pub(crate) mod three_dmr;
pub(crate) mod voxelize;
pub(crate) mod wikidata;
