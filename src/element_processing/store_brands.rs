//! Chain-aware retail styles derived from business tags on OSM building outlines.
//!
//! These profiles use block palettes and furnishing patterns rather than downloaded
//! logos or textures, so output stays deterministic and works offline.

use crate::block_definitions::*;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreFamily {
    Restaurant,
    Supermarket,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreBrand {
    McDonalds,
    Kfc,
    BurgerKing,
    Subway,
    Dominos,
    Jollibee,
    MosBurger,
    Walmart,
    Carrefour,
    Aldi,
    Tesco,
    Aeon,
}

#[derive(Clone, Copy, Debug)]
pub struct BrandPalette {
    pub wall: Block,
    pub primary: Block,
    pub secondary: Block,
    pub glass: Block,
    pub roof: Block,
    pub floor: Block,
    pub counter: Block,
    pub shelving: Block,
}

impl StoreBrand {
    pub fn family(self) -> StoreFamily {
        match self {
            Self::Walmart | Self::Carrefour | Self::Aldi | Self::Tesco | Self::Aeon => {
                StoreFamily::Supermarket
            }
            _ => StoreFamily::Restaurant,
        }
    }

    pub fn palette(self) -> BrandPalette {
        let neutral = BrandPalette {
            wall: WHITE_CONCRETE,
            primary: RED_CONCRETE,
            secondary: YELLOW_CONCRETE,
            glass: LIGHT_BLUE_STAINED_GLASS,
            roof: GRAY_CONCRETE,
            floor: SMOOTH_STONE,
            counter: RED_CONCRETE,
            shelving: LIGHT_GRAY_CONCRETE,
        };
        match self {
            Self::McDonalds => neutral,
            Self::Kfc => BrandPalette {
                primary: RED_CONCRETE,
                secondary: BLACK_CONCRETE,
                counter: RED_CONCRETE,
                ..neutral
            },
            Self::BurgerKing => BrandPalette {
                wall: BROWN_CONCRETE,
                primary: ORANGE_CONCRETE,
                secondary: RED_CONCRETE,
                counter: ORANGE_CONCRETE,
                ..neutral
            },
            Self::Subway => BrandPalette {
                primary: GREEN_CONCRETE,
                secondary: YELLOW_CONCRETE,
                counter: GREEN_CONCRETE,
                ..neutral
            },
            Self::Dominos => BrandPalette {
                primary: BLUE_CONCRETE,
                secondary: RED_CONCRETE,
                counter: BLUE_CONCRETE,
                ..neutral
            },
            Self::Jollibee => BrandPalette {
                primary: RED_CONCRETE,
                secondary: YELLOW_CONCRETE,
                counter: RED_CONCRETE,
                ..neutral
            },
            Self::MosBurger => BrandPalette {
                primary: GREEN_CONCRETE,
                secondary: RED_CONCRETE,
                counter: GREEN_CONCRETE,
                ..neutral
            },
            Self::Walmart => BrandPalette {
                wall: LIGHT_GRAY_CONCRETE,
                primary: BLUE_CONCRETE,
                secondary: YELLOW_CONCRETE,
                roof: LIGHT_GRAY_CONCRETE,
                counter: BLUE_CONCRETE,
                shelving: BLUE_CONCRETE,
                ..neutral
            },
            Self::Carrefour => BrandPalette {
                primary: BLUE_CONCRETE,
                secondary: RED_CONCRETE,
                counter: BLUE_CONCRETE,
                shelving: RED_CONCRETE,
                ..neutral
            },
            Self::Aldi => BrandPalette {
                wall: LIGHT_GRAY_CONCRETE,
                primary: BLUE_CONCRETE,
                secondary: ORANGE_CONCRETE,
                roof: LIGHT_GRAY_CONCRETE,
                counter: BLUE_CONCRETE,
                shelving: LIGHT_GRAY_CONCRETE,
                ..neutral
            },
            Self::Tesco => BrandPalette {
                primary: BLUE_CONCRETE,
                secondary: RED_CONCRETE,
                counter: BLUE_CONCRETE,
                shelving: BLUE_CONCRETE,
                ..neutral
            },
            Self::Aeon => BrandPalette {
                primary: PURPLE_CONCRETE,
                secondary: MAGENTA_CONCRETE,
                counter: PURPLE_CONCRETE,
                shelving: PURPLE_CONCRETE,
                ..neutral
            },
        }
    }

    /// The shorter lanes match discount stores; larger chains use the full layout.
    pub fn checkout_lanes(self) -> i32 {
        match self {
            Self::Aldi => 2,
            _ => 3,
        }
    }

    /// Domino's is primarily a pickup counter; other listed chains emphasize dining.
    pub fn dining_spacing(self) -> i32 {
        match self {
            Self::Dominos => 6,
            Self::Subway | Self::MosBurger => 5,
            _ => 4,
        }
    }

    /// Identify a chain from commonly used business tags. Explicit brand/operator
    /// fields take precedence; `name` is only considered on a retail or food feature.
    pub fn from_tags(tags: &HashMap<String, String>) -> Option<Self> {
        for key in ["brand", "brand:en", "operator", "operator:en"] {
            if let Some(value) = tags.get(key) {
                if let Some(brand) = Self::from_value(value) {
                    return Some(brand);
                }
            }
        }

        let retail_or_food = tags.contains_key("shop")
            || tags.contains_key("amenity")
            || tags.contains_key("cuisine")
            || tags.contains_key("building")
            || tags.contains_key("building:part")
            || matches!(
                tags.get("building").map(String::as_str),
                Some("retail" | "supermarket" | "shop" | "commercial")
            );
        if retail_or_food {
            for key in ["name", "official_name", "short_name"] {
                if let Some(value) = tags.get(key) {
                    if let Some(brand) = Self::from_value(value) {
                        return Some(brand);
                    }
                }
            }
        }
        None
    }

    fn from_value(value: &str) -> Option<Self> {
        let normalized = normalize(value);
        [
            (Self::McDonalds, &["mcdonalds"][..]),
            (Self::Kfc, &["kfc", "kentuckyfriedchicken"][..]),
            (Self::BurgerKing, &["burgerking"][..]),
            (Self::Subway, &["subway"][..]),
            (Self::Dominos, &["dominospizza", "dominos"][..]),
            (Self::Jollibee, &["jollibee"][..]),
            (Self::MosBurger, &["mosburger", "mosfoodservices"][..]),
            (Self::Walmart, &["walmart", "walmartsupercenter"][..]),
            (Self::Carrefour, &["carrefour"][..]),
            (Self::Aldi, &["aldinord", "aldisüd", "aldisud", "aldi"][..]),
            (Self::Tesco, &["tesco"][..]),
            (Self::Aeon, &["aeon"][..]),
        ]
        .into_iter()
        .find_map(|(brand, aliases)| {
            aliases
                .iter()
                .any(|alias| has_brand_prefix(&normalized, alias))
                .then_some(brand)
        })
    }
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn has_brand_prefix(value: &str, alias: &str) -> bool {
    let Some(suffix) = value.strip_prefix(alias) else {
        return false;
    };
    if suffix.is_empty() || suffix.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return true;
    }
    [
        "restaurant",
        "store",
        "supercenter",
        "supermarket",
        "superstore",
        "extra",
        "metro",
        "mall",
        "market",
        "express",
        "pizza",
        "nord",
        "sud",
        "north",
        "south",
    ]
    .iter()
    .any(|word| suffix.starts_with(word))
}
