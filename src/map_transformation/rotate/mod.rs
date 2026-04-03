mod rotator;
// quick access to parent trait for this mod
use super::operator::Operator;

// interface for generation from json
pub use rotator::rotator_from_json;

// interface for direct construction (used by tests)
#[cfg(test)]
pub use rotator::Rotator;

// interface for direct function call (used by CLI/GUI)
pub use rotator::rotate_world;
