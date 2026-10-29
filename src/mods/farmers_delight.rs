//! Farmer's Delight: kitchens, market crates and the crops OSM names on farmland.

use super::{dir_name, state};
use crate::block_definitions::*;

pub fn stove(front: (i32, i32)) -> BlockWithProperties {
    state(
        "farmersdelight:stove",
        FURNACE,
        &[("facing", dir_name(front)), ("lit", "false")],
    )
}

pub fn cooking_pot(front: (i32, i32)) -> BlockWithProperties {
    state(
        "farmersdelight:cooking_pot",
        CAULDRON,
        &[("facing", dir_name(front)), ("support", "none")],
    )
}

pub fn skillet(front: (i32, i32)) -> BlockWithProperties {
    state(
        "farmersdelight:skillet",
        OAK_PRESSURE_PLATE,
        &[("facing", dir_name(front)), ("support", "false")],
    )
}

pub fn cutting_board(front: (i32, i32)) -> BlockWithProperties {
    state(
        "farmersdelight:cutting_board",
        OAK_PRESSURE_PLATE,
        &[("facing", dir_name(front))],
    )
}

/// A wooden kitchen cabinet; only oak, spruce, dark oak and a few more exist.
pub fn cabinet(wood: &str, front: (i32, i32)) -> BlockWithProperties {
    state(
        &format!("farmersdelight:{wood}_cabinet"),
        BARREL,
        &[("facing", dir_name(front)), ("open", "false")],
    )
}

/// A crate of produce for a market stall or a greengrocer.
pub fn crate_of(h: u64) -> BlockWithProperties {
    const CRATES: [&str; 7] = [
        "tomato_crate",
        "cabbage_crate",
        "onion_crate",
        "carrot_crate",
        "potato_crate",
        "beetroot_crate",
        "rice_bag",
    ];
    let name = CRATES[(h % CRATES.len() as u64) as usize];
    state(&format!("farmersdelight:{name}"), BARREL, &[])
}

/// A meal set out on a table, for restaurants.
pub fn feast(front: (i32, i32), h: u64) -> BlockWithProperties {
    let (name, fallback) = match h % 4 {
        0 => ("roast_chicken_block", CAKE),
        1 => ("honey_glazed_ham_block", CAKE),
        2 => ("shepherds_pie_block", CAKE),
        _ => ("apple_pie", CAKE),
    };
    if name == "apple_pie" {
        return state(
            "farmersdelight:apple_pie",
            fallback,
            &[("facing", dir_name(front)), ("bites", "0")],
        );
    }
    state(
        &format!("farmersdelight:{name}"),
        fallback,
        &[("facing", dir_name(front)), ("servings", "4")],
    )
}

/// Something a bakery or a butcher sets out on the counter.
pub fn shop_ware(butcher: bool, front: (i32, i32), h: u64) -> BlockWithProperties {
    let facing = dir_name(front);
    if butcher {
        let name = if h.is_multiple_of(2) {
            "honey_glazed_ham_block"
        } else {
            "roast_chicken_block"
        };
        return state(
            &format!("farmersdelight:{name}"),
            CAKE,
            &[("facing", facing), ("servings", "4")],
        );
    }
    let name = ["apple_pie", "chocolate_pie", "sweet_berry_cheesecake"][(h % 3) as usize];
    state(
        &format!("farmersdelight:{name}"),
        CAKE,
        &[("facing", facing), ("bites", "0")],
    )
}

/// A crop OSM names that vanilla has no plant for, fully grown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Crop {
    Tomato,
    Cabbage,
    Onion,
    Rice,
}

impl Crop {
    /// From an OSM `crop` or `produce` value.
    pub fn from_tag(value: &str) -> Option<Crop> {
        let v = value.to_ascii_lowercase();
        let has = |w: &str| {
            v.split([';', ',', ' '])
                .any(|p| p.trim_end_matches('s') == w)
        };
        if has("tomato") || has("tomatoe") {
            Some(Crop::Tomato)
        } else if has("cabbage") || has("vegetable") || has("lettuce") || has("kale") {
            Some(Crop::Cabbage)
        } else if has("onion") || has("leek") || has("garlic") {
            Some(Crop::Onion)
        } else if has("rice") || has("paddy") {
            Some(Crop::Rice)
        } else {
            None
        }
    }

    /// The ripe plant standing on farmland; rice stands in water instead.
    pub fn plant(self, h: u64) -> BlockWithProperties {
        match self {
            Crop::Tomato => {
                // Vines and fresh shoots, so a field is not one flat colour.
                if h.is_multiple_of(4) {
                    state("farmersdelight:budding_tomatoes", WHEAT, &[("age", "4")])
                } else {
                    state(
                        "farmersdelight:tomatoes",
                        WHEAT,
                        &[("age", "3"), ("ropelogged", "false")],
                    )
                }
            }
            Crop::Cabbage => state("farmersdelight:cabbages", CARROTS, &[("age", "7")]),
            Crop::Onion => state("farmersdelight:onions", POTATOES, &[("age", "7")]),
            Crop::Rice => state(
                "farmersdelight:rice",
                SEAGRASS,
                &[("age", "3"), ("supporting", "true")],
            ),
        }
    }

    /// Rice heads above the water.
    pub fn panicles(self) -> Option<BlockWithProperties> {
        (self == Crop::Rice).then(|| state("farmersdelight:rice_panicles", AIR, &[("age", "3")]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_tags_map_to_the_mod_plants() {
        assert_eq!(Crop::from_tag("tomatoes"), Some(Crop::Tomato));
        assert_eq!(Crop::from_tag("rice"), Some(Crop::Rice));
        assert_eq!(Crop::from_tag("wheat;onion"), Some(Crop::Onion));
        assert_eq!(Crop::from_tag("vegetables"), Some(Crop::Cabbage));
        assert_eq!(Crop::from_tag("wheat"), None);
    }
}
