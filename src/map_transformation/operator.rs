use super::translate::translator_from_json;
use crate::coordinate_system::cartesian::XZBBox;
use crate::osm_parser::ProcessedElement;

/// An Operator does transformation on the map, modifying Vec<ProcessedElement> and XZBBox
pub trait Operator {
    /// Apply the operation
    fn operate(&self, elements: &mut Vec<ProcessedElement>, xzbbox: &mut XZBBox);

    /// Return a string describing the current specific operator
    fn repr(&self) -> String;
}

pub fn operator_from_json(config: &serde_json::Value) -> Result<Box<dyn Operator>, String> {
    let operation_str = config
        .get("operation")
        .and_then(serde_json::Value::as_str)
        .ok_or("Expected a string field 'operator' in an operator dict")?;

    let operator_config = config
        .get("config")
        .ok_or("Expected a dict field 'config' in an operator dict")?;

    let operator_result: Result<Box<dyn Operator>, String> = match operation_str {
        "translate" => translator_from_json(operator_config),
        _ => Err(format!("Unrecognized operation type '{}'", operation_str)),
    };

    operator_result.map_err(|e| format!("Operator config format error:\n{}", e))
}

pub fn operator_vec_from_json(list: &serde_json::Value) -> Result<Vec<Box<dyn Operator>>, String> {
    let oplist = list
        .as_array()
        .ok_or("Expected a list of operator dict".to_string())?;

    oplist
        .iter()
        .enumerate()
        .map(|(i, v)| {
            operator_from_json(v)
                .map_err(|e| format!("Operator dict at index {} format error:\n{}", i, e))
        })
        .collect()
}
