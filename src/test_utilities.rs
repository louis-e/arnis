use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::osm_parser;
use crate::osm_parser::ProcessedElement;

// Tagged nodes, a building, a road and a multipolygon in Arnis, so tests run offline.
const ARNIS_SAMPLE: &str = r#"{"elements": [
    {"type":"node","id":1,"lat":54.6290,"lon":9.9300},
    {"type":"node","id":2,"lat":54.6290,"lon":9.9310},
    {"type":"node","id":3,"lat":54.6298,"lon":9.9310},
    {"type":"node","id":4,"lat":54.6298,"lon":9.9300},
    {"type":"node","id":5,"lat":54.6310,"lon":9.9290},
    {"type":"node","id":6,"lat":54.6320,"lon":9.9350,"tags":{"highway":"street_lamp"}},
    {"type":"node","id":7,"lat":54.6280,"lon":9.9320},
    {"type":"node","id":8,"lat":54.6280,"lon":9.9340},
    {"type":"node","id":9,"lat":54.6292,"lon":9.9340},
    {"type":"node","id":10,"lat":54.6292,"lon":9.9320},
    {"type":"node","id":11,"lat":54.6330,"lon":9.9330,"tags":{"natural":"tree"}},
    {"type":"node","id":12,"lat":54.6305,"lon":9.9305,"tags":{"amenity":"bench"}},
    {"type":"way","id":100,"nodes":[1,2,3,4,1],"tags":{"building":"house"}},
    {"type":"way","id":101,"nodes":[5,6],"tags":{"highway":"residential","name":"Dorfstraße"}},
    {"type":"way","id":102,"nodes":[7,8,9,10,7]},
    {"type":"relation","id":200,"members":[{"type":"way","ref":102,"role":"outer"}],
     "tags":{"type":"multipolygon","landuse":"grass"}}
]}"#;

pub fn generate_default_example() -> (XZBBox, Vec<ProcessedElement>) {
    let raw_data: osm_parser::OsmData =
        serde_json::from_str(ARNIS_SAMPLE).expect("Invalid sample data");
    let (mut parsed_elements, xzbbox, _outline_suppression, _part_groups, _housing_parts) =
        osm_parser::parse_osm_data(
            raw_data,
            get_llbbox_arnis(),
            false,
            &crate::projection::ProjectionSpec::local(1.0),
        );
    osm_parser::sort_by_priority(&mut parsed_elements);

    (xzbbox, parsed_elements)
}

pub fn get_llbbox_arnis() -> LLBBox {
    LLBBox::new(54.627053, 9.927928, 54.634902, 9.937563).unwrap()
}
