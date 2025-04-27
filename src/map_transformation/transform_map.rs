use super::operator::Operator;
use crate::coordinate_system::cartesian::XZBBox;
use crate::osm_parser::ProcessedElement;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use std::fs;

pub fn transform_map(elements: &mut Vec<ProcessedElement>, xzbbox: &mut XZBBox) {
    println!("{} Transforming map...", "[3/6]".bold());
    emit_gui_progress_update(11.0, "Reading map transformation config...");

    match fs::read_to_string("example_transformations.json") {
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
            emit_gui_progress_update(20.0, "No map transformation config, skipped...");
        }
    }
}
