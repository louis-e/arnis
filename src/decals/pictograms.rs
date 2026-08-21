//! 128x128 pictograms from `assets/decorations/pictograms/`, replaceable by hand, plus the
//! tag table deciding which POIs are a business worth a name plate.

use std::collections::HashMap;

macro_rules! pictograms {
    ($($name:literal),* $(,)?) => {
        /// Every bundled pictogram name.
        #[allow(dead_code)]
        pub const NAMES: &[&str] = &[$($name),*];

        /// PNG bytes of a bundled pictogram.
        pub fn asset(name: &str) -> Option<&'static [u8]> {
            match name {
                $($name => Some(include_bytes!(concat!(
                    "../../assets/decorations/pictograms/", $name, ".png"
                ))),)*
                _ => None,
            }
        }
    };
}

pictograms!(
    "atm",
    "bus_stop",
    "hydrant",
    "information",
    "metro_m",
    "metro_s",
    "metro_u",
    "parking",
    "recycling",
    "train",
    "tram",
    "vending_machine",
);

/// Category for a `shop=*` value.
fn for_shop(value: &str) -> Option<&'static str> {
    Some(match value {
        "supermarket" | "department_store" | "mall" | "wholesale" | "general" | "variety_store" => {
            "supermarket"
        }
        "convenience" | "grocery" | "frozen_food" | "health_food" | "organic" | "farm" => {
            "convenience"
        }
        "bakery" | "pastry" => "bakery",
        "butcher" | "seafood" | "cheese" | "deli" => "butcher",
        "greengrocer" => "greengrocer",
        "confectionery" | "chocolate" | "candy" => "sweets",
        "alcohol" | "wine" | "beverages" | "brewing_supplies" => "alcohol",
        "coffee" | "tea" => "coffee_shop",
        "clothes"
        | "boutique"
        | "fashion"
        | "fashion_accessories"
        | "tailor"
        | "fabric"
        | "second_hand"
        | "charity"
        | "baby_goods"
        | "sewing"
        | "leather" => "clothes",
        "shoes" | "shoe_repair" => "shoes",
        "books" | "stationery" | "newsagent" | "copyshop" | "printing" => "books",
        "electronics" | "computer" | "hifi" | "video" | "video_games" | "appliance"
        | "electrical" | "radiotechnics" | "vacuum_cleaner" | "camera" => "electronics",
        "mobile_phone" | "telecommunication" => "mobile_phone",
        "optician" => "optician",
        "jewelry" | "watches" | "gold_buyer" => "jewelry",
        "florist" => "florist",
        "garden_centre" | "agrarian" | "landscaping" => "garden",
        "hardware"
        | "doityourself"
        | "paint"
        | "tiles"
        | "trade"
        | "locksmith"
        | "tool_hire"
        | "glaziery"
        | "houseware"
        | "bathroom_furnishing"
        | "flooring"
        | "kitchen"
        | "lighting"
        | "curtain"
        | "window_blind"
        | "energy"
        | "fireplace"
        | "security" => "hardware",
        "furniture" | "interior_decoration" | "bed" | "antiques" | "carpet" | "frame" => {
            "furniture"
        }
        "toys" => "toys",
        "gift" | "party" | "craft" | "art" | "hobby" | "model" | "collector" | "games"
        | "anime" => "gift",
        "pet" | "pet_grooming" => "pet",
        "bicycle" | "e-bike" | "scooter" | "motorcycle" | "motorcycle_repair" => "bicycle",
        "sports" | "outdoor" | "fishing" | "hunting" | "golf" | "ski" | "swimming_pool"
        | "weapons" | "military_surplus" | "boat" => "sports",
        "hairdresser" | "beauty" | "cosmetics" | "perfumery" | "massage" | "tattoo"
        | "hairdresser_supply" | "nails" | "erotic" => "hairdresser",
        "laundry" | "dry_cleaning" => "laundry",
        "kiosk" | "tobacco" | "e-cigarette" | "cannabis" | "lottery" | "bookmaker" | "vacant" => {
            "kiosk"
        }
        "chemist" | "medical_supply" | "hearing_aids" | "nutrition_supplements" | "herbalist" => {
            "chemist"
        }
        "music" | "musical_instrument" => "music",
        "car" | "car_parts" | "tyres" | "caravan" | "trailer" | "truck" | "atv" | "snowmobile" => {
            "car"
        }
        "car_repair" => "car_repair",
        "photo" | "photo_studio" => "photo",
        "travel_agency"
        | "estate_agent"
        | "insurance"
        | "money_lender"
        | "pawnbroker"
        | "funeral_directors"
        | "storage_rental"
        | "outpost"
        | "ticket"
        | "religion"
        | "rental"
        | "laundry_service"
        | "dry_cleaning_service" => "office",
        "bag" | "boutique_bag" => "bag",
        _ => return None,
    })
}

/// Pictogram for an `amenity=*` value.
fn for_amenity(value: &str) -> Option<&'static str> {
    Some(match value {
        "restaurant" | "food_court" | "biergarten" | "canteen" => "restaurant",
        "cafe" | "internet_cafe" => "cafe",
        "bar" => "bar",
        "nightclub" | "stripclub" | "hookah_lounge" | "love_hotel" => "nightclub",
        "pub" => "pub",
        "fast_food" => "fast_food",
        "ice_cream" => "ice_cream",
        "pharmacy" => "pharmacy",
        "dentist" => "dentist",
        "doctors" | "clinic" | "nursing_home" | "social_facility" | "healthcare" => "doctors",
        "hospital" => "hospital",
        "veterinary" | "animal_shelter" | "animal_boarding" => "veterinary",
        "bank" => "bank",
        "bureau_de_change" | "money_transfer" | "payment_centre" => "money_exchange",
        "atm" => "atm",
        "post_office" | "post_box" | "post_depot" | "parcel_locker" | "courier" => "post",
        "police" | "prison" | "customs" | "ranger_station" => "police",
        "fire_station" | "rescue_station" => "fire_station",
        "townhall" | "courthouse" | "public_building" | "register_office" | "archive" => "townhall",
        "coworking_space" | "conference_centre" | "exhibition_centre" | "events_venue"
        | "studio" | "public_bath" => "office",
        "school" | "college" | "university" | "language_school" | "music_school"
        | "prep_school" | "research_institute" | "training" => "school",
        "kindergarten" | "childcare" => "kindergarten",
        "driving_school" => "driving_school",
        "library" | "public_bookcase" | "toy_library" => "library",
        "cinema" => "cinema",
        "theatre" | "arts_centre" | "concert_hall" | "planetarium" => "theatre",
        "community_centre" | "social_centre" | "youth_centre" | "place_of_mourning"
        | "senior_centre" | "shelter_home" => "community",
        "place_of_worship" | "monastery" | "crematorium" => "place_of_worship",
        "toilets" | "shower" => "toilets",
        "parking" | "parking_entrance" | "motorcycle_parking" => "parking",
        "charging_station" => "charging_station",
        "fuel" | "compressed_air" => "fuel",
        "car_wash" => "car_wash",
        "car_rental" | "car_sharing" | "car_pooling" | "vehicle_inspection" => "car",
        "bicycle_rental" | "bicycle_repair_station" => "bicycle",
        "taxi" => "taxi",
        "vending_machine" => "vending_machine",
        "marketplace" => "marketplace",
        "casino" | "gambling" => "casino",
        "embassy" | "consulate" => "embassy",
        "swimming_pool" | "public_bath_house" => "swimming",
        "gym" | "fitness_centre" | "dojo" => "gym",
        _ => return None,
    })
}

/// Pictogram for a `tourism=*` value.
fn for_tourism(value: &str) -> Option<&'static str> {
    Some(match value {
        "hotel" | "hostel" | "guest_house" | "motel" | "apartment" | "chalet" | "alpine_hut"
        | "wilderness_hut" | "camp_site" | "caravan_site" => "hotel",
        "museum" | "gallery" | "aquarium" => "museum",
        "attraction" | "theme_park" | "zoo" | "artwork" => "attraction",
        "viewpoint" => "viewpoint",
        "information" => "information",
        _ => return None,
    })
}

/// Pictogram for a `leisure=*` value (only the ones that read as a business).
fn for_leisure(value: &str) -> Option<&'static str> {
    Some(match value {
        "fitness_centre" | "sports_centre" | "sports_hall" | "fitness_station" => "gym",
        "swimming_pool" | "water_park" => "swimming",
        "bowling_alley"
        | "amusement_arcade"
        | "escape_game"
        | "adult_gaming_centre"
        | "miniature_golf"
        | "trampoline_park"
        | "dance" => "attraction",
        _ => return None,
    })
}

/// Business category of a POI's tags, if it is one (shops, food, services, civic, lodging).
/// The category names double as pictogram names for a possible future icon set.
pub fn business_kind(tags: &HashMap<String, String>) -> Option<&'static str> {
    if let Some(v) = tags.get("shop") {
        if let Some(p) = for_shop(v) {
            return Some(p);
        }
        // Any other shop value is still a shop.
        return Some("shop");
    }
    if let Some(v) = tags.get("amenity") {
        if let Some(p) = for_amenity(v) {
            return Some(p);
        }
    }
    if let Some(v) = tags.get("tourism") {
        if let Some(p) = for_tourism(v) {
            return Some(p);
        }
    }
    if let Some(v) = tags.get("leisure") {
        if let Some(p) = for_leisure(v) {
            return Some(p);
        }
    }
    if let Some(v) = tags.get("healthcare") {
        return Some(match v.as_str() {
            "dentist" => "dentist",
            "pharmacy" => "pharmacy",
            "hospital" => "hospital",
            _ => "doctors",
        });
    }
    if tags.contains_key("office") {
        return Some("office");
    }
    if tags.contains_key("craft") {
        return Some("hardware");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_asset_decodes_as_rgba_128() {
        for name in NAMES {
            let bytes = asset(name).unwrap();
            let img = image::load_from_memory(bytes).unwrap();
            assert_eq!((img.width(), img.height()), (128, 128), "{name}");
        }
    }

    #[test]
    fn business_kind_classifies_shops_and_amenities() {
        for shop in [
            "bakery",
            "clothes",
            "kiosk",
            "unknown_thing",
            "toys",
            "gift",
        ] {
            let mut t = HashMap::new();
            t.insert("shop".to_string(), shop.to_string());
            assert!(business_kind(&t).is_some(), "{shop}");
        }
        for (amenity, is_business) in [
            ("cafe", true),
            ("bank", true),
            ("townhall", true),
            ("pharmacy", true),
            ("nightclub", true),
            ("bench", false),
            ("waste_basket", false),
        ] {
            let mut t = HashMap::new();
            t.insert("amenity".to_string(), amenity.to_string());
            assert_eq!(business_kind(&t).is_some(), is_business, "{amenity}");
        }
    }
}
