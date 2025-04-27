use crate::coordinate_system::cartesian::XZVector;
use serde::Deserialize;

// directly specify movement on x, z directions
#[derive(Debug, Deserialize)]
pub struct VectorTranslator {
    pub vector: XZVector,
}
