use super::operator::operator_vec_from_json;
use crate::coordinate_system::cartesian::XZBBox;
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;

pub fn transform_map(
    elements: &mut Vec<ProcessedElement>,
    xzbbox: &mut XZBBox,
    ground: &mut Ground,
) {
    println!("{} Transforming map...", "[4/7]".bold());
    emit_gui_progress_update(19.0, "Transforming map...");

    let opjson_string = include_str!("../../tests/map_transformation/example_transformations.json");
    let opjson = serde_json::from_str(opjson_string)
        .expect("Failed to parse map transformations config json");

    let ops = operator_vec_from_json(&opjson)
        .map_err(|e| format!("Map transformations json format error:\n{e}"))
        .unwrap_or_else(|e| {
            eprintln!("{e}");
            panic!();
        });

    for op in ops {
        op.operate(elements, xzbbox, ground);
    }
}
