mod rotator;
// quick access to parent trait for this mod
use super::operator::Operator;

mod angle_rotator;

// interface for generation from json
pub use rotator::rotator_from_json;

// // interface for direct generation in memory, currently only used by test
// #[cfg(test)]
// pub use angle_rotator::AngleRotator;
