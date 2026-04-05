mod rotator;
// quick access to parent trait for this mod
use super::operator::Operator;

// interface for generation from json
pub use rotator::rotator_from_json;

// interface for direct function call (used by CLI/GUI)
pub use rotator::rotate_world;

// interface for rotating a single point (used for spawn rotation)
pub use rotator::rotate_xz_point;
