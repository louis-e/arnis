mod translator;
// quick access to parent trait for this mod
use super::operator::Operator;

mod startend_translator;
mod vector_translator;

// interface for generation from json
pub use translator::translator_from_json;

// interface for direct generation in memory, currently only used by test
#[cfg(test)]
pub use startend_translator::StartEndTranslator;
#[cfg(test)]
pub use vector_translator::VectorTranslator;
