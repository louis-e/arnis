use super::rotator::rotate_by_angle;
use super::Operator;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;
use serde::Deserialize;

/// rotate the map about center by deg (axis y)
#[derive(Debug, Deserialize, PartialEq)]
pub struct AngleRotator {
    pub center: XZPoint,
    pub deg: f64,
}

impl Operator for AngleRotator {
    fn operate(
        &self,
        elements: &mut Vec<ProcessedElement>,
        xzbbox: &mut XZBBox,
        ground: &mut Ground,
    ) {
        rotate_by_angle(self.center, self.deg, elements, xzbbox, ground);
    }

    fn repr(&self) -> String {
        format!("rotate about {} by {} deg", self.center, self.deg)
    }
}
