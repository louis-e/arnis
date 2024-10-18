use crate::{osm_parser::ProcessedRelation, world_editor::WorldEditor};

pub fn generate_water_areas(
    editor: &mut WorldEditor,
    element: &ProcessedRelation,
    ground_level: i32,
) {
    let Some(water_type) = element.tags.get("water") else {
        return;
    };

    dbg!(water_type);
    dbg!(&element.members);
}
