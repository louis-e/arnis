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
        _ => Err(format!("Unrecognized translator type '{type_str}'")),
    };

    translator_result.map_err(|e| format!("Translator config format error:\n{e}"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZVector;
    use crate::test_utilities::generate_default_example;

    // this ensures translate_by_vector function is correct
    #[test]
    fn test_translate_by_vector() {
        let dx: i32 = 123;
        let dz: i32 = -234;
        let vector = XZVector { dx, dz };

        let (xzbbox1, elements1, _) = generate_default_example();

        let mut xzbbox2 = xzbbox1.clone();
        let mut elements2 = elements1.clone();

        translate_by_vector(vector, &mut elements2, &mut xzbbox2);

        // 1. Elem type should not change
        // 2. For node,
        //      2.1 id and tags should not change
        //      2.2 x, z should be displaced as required
        // 3. For way,
        //      3.1 id and tags should not change
        //      3.2 For every node included, satisfies (2)
        // 4. For relation, everything is unchanged
        for (original, translated) in elements1.iter().zip(elements2.iter()) {
            match (original, translated) {
                (ProcessedElement::Node(a), ProcessedElement::Node(b)) => {
                    assert_eq!(a.id, b.id);
                    assert_eq!(a.tags, b.tags);
                    assert_eq!(b.x, a.x + dx);
                    assert_eq!(b.z, a.z + dz);
                }
                (ProcessedElement::Way(a), ProcessedElement::Way(b)) => {
                    assert_eq!(a.id, b.id);
                    assert_eq!(a.tags, b.tags);
                    for (nodea, nodeb) in a.nodes.iter().zip(b.nodes.iter()) {
                        assert_eq!(nodea.id, nodeb.id);
                        assert_eq!(nodea.tags, nodeb.tags);
                        assert_eq!(nodeb.x, nodea.x + dx);
                        assert_eq!(nodeb.z, nodea.z + dz);
                    }
                }
                (ProcessedElement::Relation(a), ProcessedElement::Relation(b)) => {
                    assert_eq!(a, b);
                }
                _ => {
                    panic!(
                        "Element type changed: original {} to {}",
                        original.kind(),
                        translated.kind()
                    );
                }
            }
        }
    }
}
