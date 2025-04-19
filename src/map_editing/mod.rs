use crate::cartesian::XZBBox;
use crate::osm_parser::ProcessedElement;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use std::fs;
use translate::Translator;

pub mod translate;

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

        let operators: Vec<Self> = oplist.iter().map(Self::from_json).collect();

        operators
    }

    pub fn kind(&self) -> String {
        match self {
            Self::Translate(_) => "translate".to_string(),
        }
    }
}

pub fn edit_map(elements: &mut Vec<ProcessedElement>, xzbbox: &mut XZBBox) {
    println!("{} Editing data...", "[3/6]".bold());
    emit_gui_progress_update(11.0, "Reading map editing config...");

    match fs::read_to_string("map_editing.json") {
        Ok(opjson_string) => {
            let opjson = serde_json::from_str(&opjson_string)
                .expect("Failed to parse map editing config json");

            let ops = Operator::vec_from_json(&opjson);
            let nop: usize = ops.len();
            let mut iop: usize = 1;

            let progress_increment_prcs: f64 = 9.0 / nop as f64;
            let mut current_progress_prcs: f64 = 11.0;

            for op in ops {
                current_progress_prcs += progress_increment_prcs;
                let message = format!("Applying operation: {}, {}/{}", op.kind(), iop, nop);
                emit_gui_progress_update(current_progress_prcs, &message);

                iop += 1;

                op.operate(elements, xzbbox);
            }

            emit_gui_progress_update(20.0, "Map operations applied...");
        }
        Err(_) => {
            emit_gui_progress_update(20.0, "No map editing config, skipped...");
        }
    }
}
