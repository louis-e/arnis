use crate::bbox::BBox;
use crate::coordinate_system::cartesian::XZBBox;
use crate::osm_parser;
use crate::osm_parser::ProcessedElement;
use crate::retrieve_data;

// this is copied from main.rs
pub fn generate_example(llbbox: BBox) -> (XZBBox, Vec<ProcessedElement>) {
    // Fetch data
    let raw_data: serde_json::Value =
        retrieve_data::fetch_data_from_overpass(llbbox, false, "requests", None)
            .expect("Failed to fetch data");

    // Parse raw data
    let (mut parsed_elements, scale_factor_x, scale_factor_z) =
        osm_parser::parse_osm_data(&raw_data, llbbox, 1.0, false);
    parsed_elements
        .sort_by_key(|element: &osm_parser::ProcessedElement| osm_parser::get_priority(element));

    let xzbbox = XZBBox::rect_from_xz_lengths(scale_factor_x, scale_factor_z)
        .expect("Parsed world lengths < 1");

    (xzbbox, parsed_elements)
}

pub fn generate_default_example() -> (XZBBox, Vec<ProcessedElement>) {
    // Arnis, Germany
    generate_example(BBox::new(54.627053, 9.927928, 54.634902, 9.937563).unwrap())
}
