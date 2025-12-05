use super::Operator;
use super::translator::translate_by_vector;
use crate::coordinate_system::cartesian::{XZBBox, XZVector};
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;
use serde::Deserialize;

/// Translate by directly specifying displacement on x, z directions
#[derive(Debug, Deserialize, PartialEq)]
pub struct VectorTranslator {
    pub vector: XZVector,
}

impl Operator for VectorTranslator {
    fn operate(&self, elements: &mut Vec<ProcessedElement>, xzbbox: &mut XZBBox, _: &mut Ground) {
        translate_by_vector(self.vector, elements, xzbbox);
    }

    fn repr(&self) -> String {
        format!("translate diaplacement {}", self.vector)
    }
}
