use crate::cartesian::XZBBox;
use crate::osm_parser::ProcessedElement;

pub mod translate;

use translate::Translator;

pub enum Operator {
    Translate(translate::Translator),
}

impl Operator {
    pub fn operate(&self, elements: &mut Vec<ProcessedElement>, xzbbox: &mut XZBBox) {
        match self {
            Self::Translate(o) => o.translate(elements, xzbbox),
        }
    }

    pub fn from_json(config: &serde_json::Value) -> Self {
        let operation_str = config
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .expect("Expected field 'operation' to be a string and present for one operation");

        match operation_str {
            "translate" => {
                let o: Translator = Translator::from_json(&config["config"]);
                Self::Translate(o)
            }
            _ => panic!("Unknown operation type: {}", operation_str),
        }
    }

    pub fn vec_from_json(list: &serde_json::Value) -> Vec<Self> {
        let oplist = list.as_array().expect("Expected a list of operations");

        let operators: Vec<Self> = oplist.iter().map(|v| Self::from_json(v)).collect();

        operators
    }
}

pub fn edit_map(
    elements: &mut Vec<ProcessedElement>,
    xzbbox: &mut XZBBox,
    opjson: &serde_json::Value,
) {
    let ops = Operator::vec_from_json(opjson);
    for op in ops {
        op.operate(elements, xzbbox);
    }
}
