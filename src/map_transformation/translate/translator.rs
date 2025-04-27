// use super::startend_translator::StartEndTranslator;
use super::startend_translator::StartEndTranslator;
use super::vector_translator::VectorTranslator;
use super::Operator;
use crate::coordinate_system::cartesian::{XZBBox, XZVector};
use crate::osm_parser::ProcessedElement;

/// Create a translate operator (translator) from json
pub fn translator_from_json(config: &serde_json::Value) -> Result<Box<dyn Operator>, String> {
    let type_str = config
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or("Expected a string field 'type' in an translator dict:\n{}".to_string())?;

    let translator_config = config
        .get("config")
        .ok_or("Expected a dict field 'config' in an translator dict")?;

    let translator_result: Result<Box<dyn Operator>, String> = match type_str {
        "vector" => {
            let upper_result: Result<Box<VectorTranslator>, _> =
                serde_json::from_value(translator_config.clone())
                    .map(Box::new)
                    .map_err(|e| e.to_string());
            upper_result.map(|o| o as Box<dyn Operator>)
        }
        "startend" => {
            let upper_result: Result<Box<StartEndTranslator>, _> =
                serde_json::from_value(translator_config.clone())
                    .map(Box::new)
                    .map_err(|e| e.to_string());
            upper_result.map(|o| o as Box<dyn Operator>)
        }
        _ => Err(format!("Unrecognized translator type '{}'", type_str)),
    };

    translator_result.map_err(|e| format!("Translator config format error:\n{}", e))
}

/// Translate elements and bounding box by a vector
pub fn translate_by_vector(
    vector: XZVector,
    elements: &mut Vec<ProcessedElement>,
    xzbbox: &mut XZBBox,
) {
    *xzbbox += vector;

    for element in elements {
        match element {
            ProcessedElement::Node(n) => {
                n.x += vector.dx;
                n.z += vector.dz;
            }
            ProcessedElement::Way(w) => {
                for n in &mut w.nodes {
                    n.x += vector.dx;
                    n.z += vector.dz;
                }
            }
            _ => {}
        }
    }
}
