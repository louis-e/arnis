use super::operator::operator_vec_from_json;
use crate::coordinate_system::cartesian::XZBBox;
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use std::fs;

pub fn transform_map(
    elements: &mut Vec<ProcessedElement>,
    xzbbox: &mut XZBBox,
    ground: &mut Ground,
) {
    println!("{} Transforming map...", "[3/6]".bold());
    emit_gui_progress_update(21.0, "Reading map transformation config...");

    match fs::read_to_string("example_transformations.json") {
        Err(_) => {
            emit_gui_progress_update(40.0, "No map transformation config, skipped...");
        }
        Ok(opjson_string) => {
            let opjson = serde_json::from_str(&opjson_string)
                .expect("Failed to parse map transformations config json");

            let ops = operator_vec_from_json(&opjson)
                .map_err(|e| format!("Map transformations json format error:\n{}", e))
                .unwrap_or_else(|e| {
                    eprintln!("{}", e);
                    panic!();
                });

            let nop: usize = ops.len();
            let mut iop: usize = 1;

            let progress_increment_prcs: f64 = 19.0 / nop as f64;
            let mut current_progress_prcs: f64 = 21.0;

            for op in ops {
                current_progress_prcs += progress_increment_prcs;
                let message = format!("Applying operation: {}, {}/{}", op.repr(), iop, nop);
                emit_gui_progress_update(current_progress_prcs, &message);

                iop += 1;

                op.operate(elements, xzbbox, ground);
            }

            emit_gui_progress_update(40.0, "Map operations applied...");
        }
    }
}
