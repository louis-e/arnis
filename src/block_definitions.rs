use serde::Deserialize;
use std::collections::HashMap;
use std::sync::LazyLock;
use crate::colors::RGBTuple;

/// Use this!
pub static BLOCKS: LazyLock<Blocks> = LazyLock::new(|| Blocks::load());

#[derive(Deserialize, Debug)]
enum PropertyType {
    IntType(i32),
    StrType(String),
    BoolType(bool),
}

#[derive(Deserialize, Debug)]
pub struct Block {
    pub name: String,
    pub id: u8, // TODO what type should this be?
    pub properties: Option<HashMap<String, PropertyType>>,
    pub building_corner: bool, // TODO may need Option

    /// https://wiki.openstreetmap.org/wiki/Key:building:colour
    pub wall_color: Option<RGBTuple>,
    pub floor_color: Option<RGBTuple>,
}

#[derive(Deserialize, Debug)]
pub struct Blocks {
    blocks: Vec<Block>,
}

impl Blocks {

    fn load() -> Self {
        let blocks_toml = std::fs::read_to_string("../data/blocks.toml")
            .expect("Should have been able to read data/blocks.toml");

        toml::from_str(&blocks_toml).unwrap()
    }

    pub fn by_name(&self, name: &str) -> Option<&Block> {
        self.blocks.iter().find(|e| e.name == name)
    }

    pub fn by_id(&self, id: u8) -> Option<&Block> {
        self.blocks.iter().find(|e| e.id == id)
    }

    // Variations for building walls
    // pub fn building_wall_variations(&self) -> Vec<Block> {
    //     todo!();
    // }
    //
    // Variations for building floors
    // pub fn building_floor_variations(&self) -> Vec<Block> {
    //     todo!();
    // }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocks_load_fn() {
        let blocks = Blocks::load();
    }

    #[test]
    fn test_by_name() {
        let air_block = &*BLOCKS.by_name("air").unwrap();
        assert_eq!(air_block.id, 1);
    }

    #[test]
    fn test_by_id() {
        let blackstone_block = &*BLOCKS.by_id(6).unwrap();
        assert_eq!(blackstone_block.name, "blackstone");
    }

    #[test]
    fn test_fields() {
        // [[blocks]]
        // name = "blackstone"
        // id = 6
        // building_corner = true
        // floor_color = [22, 15, 16]
        let blackstone_block = &*BLOCKS.by_id(6).unwrap();
        assert_eq!(blackstone_block.name, "blackstone");
        assert_eq!(blackstone_block.id, 6);
        assert_eq!(blackstone_block.properties, None);
        assert_eq!(blackstone_block.building_corner, true);
        assert_eq!(blackstone_block.wall_color, None);
        assert_eq!(blackstone_block.floor_color, Some((22, 15, 16)));

        // [[blocks]]
        // name = "polished_diorite"
        // id = 60
        // wall_color = [174, 173, 174]
        // floor_color = [255, 255, 255]
        let polished_diorite_block = &*BLOCKS.by_name("polished_diorite").unwrap();
        assert_eq!(polished_diorite_block.name, "polished_diorite");
        assert_eq!(polished_diorite_block.id, 6);
        assert_eq!(polished_diorite_block.properties, None);
        assert_eq!(polished_diorite_block.building_corner, false);
        assert_eq!(polished_diorite_block.wall_color, Some((174, 173, 174)));
        assert_eq!(polished_diorite_block.floor_color, Some((255, 255, 255)));
    }

    #[test]
    fn test_properties() {
        // [[blocks]]
        // name = "oak_leaves"
        // id = 49
        // properties.persistent = true
        let oak_leaves_block = &*BLOCKS.by_id(49).unwrap();
        assert_eq!(oak_leaves_block.name, "oak_leaves");
        assert_eq!(oak_leaves_block.properties.unwrap().get("persistent"), true);

        // [[blocks]]
        // name = "sign"
        // id = 113
        // properties.rotation = 6
        // properties.waterlogged = false
        let sign_block = &*BLOCKS.by_name("sign").unwrap();
        assert_eq!(sign_block.id, 113);
        assert_eq!(sign_block.properties.unwrap().get("waterlogged"), false);
        assert_eq!(sign_block.properties.unwrap().get("rotation"), 6);

        // [[blocks]]
        // name = "dark_oak_door_lower"
        // id = 106
        // properties.half = "lower"
        let dark_oak_door_lower_block = &*BLOCKS.by_id(106).unwrap();
        assert_eq!(dark_oak_door_lower_block.name, "dark_oak_door_lower");
        assert_eq!(dark_oak_door_lower_block.properties.unwrap().get("half"), "lower");
    }

}
