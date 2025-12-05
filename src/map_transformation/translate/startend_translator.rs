use super::Operator;
use super::translator::translate_by_vector;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;
use serde::Deserialize;

// move the map so that start goes to end
#[derive(Debug, Deserialize, PartialEq)]
pub struct StartEndTranslator {
    pub start: XZPoint,
    pub end: XZPoint,
}

impl Operator for StartEndTranslator {
    fn operate(&self, elements: &mut Vec<ProcessedElement>, xzbbox: &mut XZBBox, _: &mut Ground) {
        translate_by_vector(self.end - self.start, elements, xzbbox);
    }

    fn repr(&self) -> String {
        format!("translate {} to {}", self.start, self.end)
    }
}
