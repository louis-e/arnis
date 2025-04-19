use crate::cartesian::{XZBBox, XZPoint, XZVector};
use crate::osm_parser::ProcessedElement;
use serde::Deserialize;

// types of translation
pub enum Translator {
    Vector(VectorTranslator),
    StartEnd(StartEndTranslator),
}

// directly specify movement on x, z directions
#[derive(Debug, Deserialize)]
pub struct VectorTranslator {
    pub vector: XZVector,
}

// move the map so that start goes to end
#[derive(Debug, Deserialize)]
pub struct StartEndTranslator {
    pub start: XZPoint,
    pub end: XZPoint,
}

impl Translator {
    pub fn to_xzvector(&self) -> XZVector {
        match self {
            Self::Vector(t) => t.vector,
            Self::StartEnd(t) => t.end - t.start,
        }
    }

    pub fn translate(&self, elements: &mut Vec<ProcessedElement>, xzbbox: &mut XZBBox) {
        let vector = self.to_xzvector();

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

    pub fn from_json(config: &serde_json::Value) -> Self {
        let type_str = config
            .get("type")
            .and_then(serde_json::Value::as_str)
            .expect(
                "Expected field 'type' to be a string and present in the config for translation",
            );

        match type_str {
            "vector" => {
                let t: VectorTranslator = serde_json::from_value(config["config"].clone()).unwrap();
                Self::Vector(t)
            }
            "start_end" => {
                let t: StartEndTranslator =
                    serde_json::from_value(config["config"].clone()).unwrap();
                Self::StartEnd(t)
            }
            _ => panic!("Unknown translation type: {}", type_str),
        }
    }
}
