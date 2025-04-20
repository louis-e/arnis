use crate::coordinate_system::cartesian::XZPoint;
use serde::Deserialize;

// move the map so that start goes to end
#[derive(Debug, Deserialize)]
pub struct StartEndTranslator {
    pub start: XZPoint,
    pub end: XZPoint,
}
