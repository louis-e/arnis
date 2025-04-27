use super::translator::translate_by_vector;
use super::Operator;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::osm_parser::ProcessedElement;
use serde::Deserialize;

// move the map so that start goes to end
#[derive(Debug, Deserialize)]
pub struct StartEndTranslator {
    pub start: XZPoint,
    pub end: XZPoint,
}

impl Operator for StartEndTranslator {
    fn operate(&self, elements: &mut Vec<ProcessedElement>, xzbbox: &mut XZBBox) {
        translate_by_vector(self.end - self.start, elements, xzbbox);
    }

    fn repr(&self) -> String {
        "translate by start and end points".to_string()
    }
}
