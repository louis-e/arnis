mod operator;
mod transform_map;

// interface for world generation pipeline
pub use transform_map::transform_map;

// interface for custom specific operator generation
pub mod rotate;
pub mod translate;
