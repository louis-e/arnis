use super::translator::translate_by_vector;
use super::Operator;
use crate::coordinate_system::cartesian::{XZBBox, XZVector};
use crate::osm_parser::ProcessedElement;
use serde::Deserialize;

/// Translate by directly specifying displacement on x, z directions
#[derive(Debug, Deserialize, PartialEq)]
pub struct VectorTranslator {
    pub vector: XZVector,
}

impl Operator for VectorTranslator {
    fn operate(&self, elements: &mut Vec<ProcessedElement>, xzbbox: &mut XZBBox) {
        translate_by_vector(self.vector, elements, xzbbox);
    }

    fn repr(&self) -> String {
        format!("translate diaplacement {}", self.vector)
    }
}
