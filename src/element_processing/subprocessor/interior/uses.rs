//! Building uses read from OSM tags, and how they spread over the floors.

use std::collections::HashMap;

/// What a shop sells, which decides its shelves and counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Goods {
    General,
    Bakery,
    Butcher,
    Grocery,
    Books,
    Clothes,
    Electronics,
    Hardware,
    Pharmacy,
    Florist,
    Jewelry,
    Toys,
    Furniture,
    Drinks,
    Salon,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Eatery {
    Restaurant,
    Cafe,
    Bar,
    FastFood,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Faith {
    Christian,
    Jewish,
    Muslim,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Use {
    Home,
    Shop(Goods),
    Supermarket,
    Food(Eatery),
    Office,
    Bank,
    Workshop,
    School,
    Kindergarten,
    Library,
    Clinic,
    Hospital,
    Hotel,
    Museum,
    Station,
    Worship(Faith),
    SportsHall,
    Gym,
    Auditorium,
    Warehouse,
    Factory,
    Barn,
}

impl Use {
    /// Rents a street-level unit, leaving the floors above to the building's own use.
    pub fn is_street_tenant(self) -> bool {
        matches!(
            self,
            Use::Shop(_)
                | Use::Supermarket
                | Use::Food(_)
                | Use::Bank
                | Use::Workshop
                | Use::Clinic
                | Use::Gym
        )
    }

    /// One tall volume rather than stacked storeys.
    pub fn is_hall(self) -> bool {
        matches!(
            self,
            Use::Worship(_)
                | Use::SportsHall
                | Use::Auditorium
                | Use::Warehouse
                | Use::Factory
                | Use::Barn
                | Use::Station
        )
    }

    /// Halls that stay one room even with a level count, usually a height estimate.
    fn always_open(self) -> bool {
        matches!(self, Use::Worship(_) | Use::SportsHall | Use::Auditorium)
    }

    /// Buildings whose ground floor is commonly let to shops and cafes.
    fn hosts_tenants(self) -> bool {
        matches!(
            self,
            Use::Home | Use::Office | Use::Hotel | Use::Shop(_) | Use::Supermarket
        )
    }

    /// Which use wins a storey too small to split between several.
    fn rank(self) -> u8 {
        match self {
            Use::Supermarket => 9,
            Use::Food(_) => 7,
            Use::Shop(_) => 6,
            Use::Bank => 5,
            Use::Clinic | Use::Gym => 4,
            Use::Workshop => 3,
            Use::Office => 2,
            _ => 1,
        }
    }

    /// Offices and practices without a level tag usually sit above the shop floor.
    fn prefers_upper_floor(self) -> bool {
        matches!(self, Use::Office | Use::Clinic)
    }
}

fn tag<'a>(tags: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    tags.get(key).map(String::as_str)
}

fn goods_for_shop(value: &str) -> Goods {
    match value {
        "bakery" | "pastry" | "confectionery" | "chocolate" | "deli" | "cheese" | "coffee"
        | "tea" => Goods::Bakery,
        "butcher" | "seafood" => Goods::Butcher,
        "greengrocer" | "farm" | "health_food" | "organic" | "frozen_food" | "pet" => {
            Goods::Grocery
        }
        "books" | "stationery" | "music" | "video" | "art" | "anime" | "musical_instrument" => {
            Goods::Books
        }
        "clothes"
        | "shoes"
        | "boutique"
        | "fashion"
        | "fashion_accessories"
        | "bag"
        | "leather"
        | "fabric"
        | "tailor"
        | "sewing"
        | "second_hand"
        | "charity"
        | "wool"
        | "curtain"
        | "sports"
        | "outdoor"
        | "baby_goods" => Goods::Clothes,
        "electronics" | "computer" | "mobile_phone" | "hifi" | "telecommunication"
        | "appliance" | "camera" | "video_games" | "radiotechnics" | "electrical" | "games" => {
            Goods::Electronics
        }
        "hardware" | "doityourself" | "trade" | "paint" | "tool_hire" | "building_materials"
        | "agrarian" | "weapons" | "hunting" | "fishing" | "locksmith" => Goods::Hardware,
        "chemist"
        | "cosmetics"
        | "perfumery"
        | "medical_supply"
        | "hearing_aids"
        | "optician"
        | "herbalist"
        | "drugstore"
        | "nutrition_supplements" => Goods::Pharmacy,
        "florist" | "garden_centre" | "plant" => Goods::Florist,
        "jewelry" | "watches" | "gold_buyer" | "pawnbroker" => Goods::Jewelry,
        "toys" | "gift" | "party" | "craft" | "hobby" | "model" | "souvenir" => Goods::Toys,
        "furniture"
        | "interior_decoration"
        | "bed"
        | "kitchen"
        | "lighting"
        | "houseware"
        | "carpet"
        | "antiques"
        | "frame"
        | "flooring"
        | "bathroom_furnishing"
        | "doors" => Goods::Furniture,
        "alcohol" | "beverages" | "wine" => Goods::Drinks,
        "hairdresser" | "beauty" | "massage" | "tattoo" | "nails" | "hairdresser_supply" => {
            Goods::Salon
        }
        _ => Goods::General,
    }
}

/// The `religion` tag, else what the building type implies.
fn faith(tags: &HashMap<String, String>) -> Faith {
    match tag(tags, "religion") {
        Some("muslim") => Faith::Muslim,
        Some("jewish") => Faith::Jewish,
        Some("christian") => Faith::Christian,
        Some(_) => Faith::Other,
        None => match tag(tags, "building") {
            Some("mosque") => Faith::Muslim,
            Some("synagogue") => Faith::Jewish,
            Some("temple" | "shrine") => Faith::Other,
            _ => Faith::Christian,
        },
    }
}

/// The use a POI or tagged outline names; `None` for anything not an indoor occupant.
pub fn use_from_tags(tags: &HashMap<String, String>) -> Option<Use> {
    if let Some(amenity) = tag(tags, "amenity") {
        let found = match amenity {
            "restaurant" | "food_court" => Some(Use::Food(Eatery::Restaurant)),
            "cafe" | "ice_cream" => Some(Use::Food(Eatery::Cafe)),
            "bar" | "pub" | "biergarten" | "nightclub" => Some(Use::Food(Eatery::Bar)),
            "fast_food" => Some(Use::Food(Eatery::FastFood)),
            "bank" | "bureau_de_change" | "money_transfer" | "post_office" => Some(Use::Bank),
            "pharmacy" => Some(Use::Shop(Goods::Pharmacy)),
            "school" | "college" | "university" | "language_school" | "music_school"
            | "driving_school" | "training" | "prep_school" => Some(Use::School),
            "kindergarten" | "childcare" => Some(Use::Kindergarten),
            "library" => Some(Use::Library),
            "doctors" | "dentist" | "clinic" | "veterinary" => Some(Use::Clinic),
            "hospital" | "nursing_home" => Some(Use::Hospital),
            "place_of_worship" | "monastery" => Some(Use::Worship(faith(tags))),
            "cinema" | "theatre" | "arts_centre" | "concert_hall" | "events_venue"
            | "conference_centre" | "community_centre" => Some(Use::Auditorium),
            "townhall" | "courthouse" | "police" | "fire_station" | "public_building"
            | "embassy" | "coworking_space" | "social_facility" => Some(Use::Office),
            "marketplace" => Some(Use::Shop(Goods::Grocery)),
            "car_wash" | "car_rental" | "vehicle_inspection" => Some(Use::Workshop),
            "gym" => Some(Use::Gym),
            "bus_station" => Some(Use::Station),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    if let Some(shop) = tag(tags, "shop") {
        return Some(match shop {
            "no" | "vacant" => return None,
            "supermarket" | "wholesale" | "hypermarket" => Use::Supermarket,
            "car" | "car_repair" | "car_parts" | "bicycle" | "motorcycle" | "tyres" | "boat" => {
                Use::Workshop
            }
            other => Use::Shop(goods_for_shop(other)),
        });
    }
    if let Some(tourism) = tag(tags, "tourism") {
        match tourism {
            "hotel" | "hostel" | "guest_house" | "motel" | "apartment" => return Some(Use::Hotel),
            "museum" | "gallery" => return Some(Use::Museum),
            _ => {}
        }
    }
    if let Some(leisure) = tag(tags, "leisure") {
        match leisure {
            "fitness_centre" | "fitness_station" => return Some(Use::Gym),
            "sports_hall" | "sports_centre" | "dance" | "bowling_alley" => {
                return Some(Use::SportsHall)
            }
            _ => {}
        }
    }
    if let Some(healthcare) = tag(tags, "healthcare") {
        return match healthcare {
            "hospital" => Some(Use::Hospital),
            "pharmacy" => Some(Use::Shop(Goods::Pharmacy)),
            "no" => None,
            _ => Some(Use::Clinic),
        };
    }
    if let Some(craft) = tag(tags, "craft") {
        return match craft {
            "bakery" | "confectionery" => Some(Use::Shop(Goods::Bakery)),
            "brewery" | "distillery" | "winery" | "metal_construction" => Some(Use::Factory),
            "no" => None,
            _ => Some(Use::Workshop),
        };
    }
    if let Some(office) = tag(tags, "office") {
        if office != "no" {
            return Some(Use::Office);
        }
    }
    None
}

/// The use the `building=*` value implies on its own.
pub fn use_from_building_type(building_type: &str, tags: &HashMap<String, String>) -> Option<Use> {
    Some(match building_type {
        "house" | "detached" | "semidetached_house" | "terrace" | "bungalow" | "villa"
        | "cabin" | "residential" | "apartments" | "dormitory" | "static_caravan" | "farm"
        | "houseboat" => Use::Home,
        "retail" | "shop" | "kiosk" => Use::Shop(Goods::General),
        "supermarket" => Use::Supermarket,
        "commercial" | "office" | "civic" | "public" | "government" | "townhall"
        | "fire_station" | "police" => Use::Office,
        "hotel" => Use::Hotel,
        "industrial" | "factory" | "manufacture" => Use::Factory,
        "warehouse" | "hangar" | "storage" | "depot" => Use::Warehouse,
        "school" | "college" | "university" => Use::School,
        "kindergarten" => Use::Kindergarten,
        "hospital" => Use::Hospital,
        "church" | "cathedral" | "chapel" | "mosque" | "synagogue" | "temple" | "religious"
        | "shrine" | "monastery" => Use::Worship(faith(tags)),
        "sports_hall" | "sports_centre" | "gymnasium" | "riding_hall" => Use::SportsHall,
        "train_station" | "transportation" => Use::Station,
        "barn" | "stable" | "cowshed" | "sty" | "farm_auxiliary" | "sheepfold" => Use::Barn,
        "museum" => Use::Museum,
        "library" => Use::Library,
        _ => return None,
    })
}

/// The use a surrounding area lends the plain `building=yes` outlines inside it.
pub fn use_from_area(tags: &HashMap<String, String>) -> Option<Use> {
    if let Some(amenity) = tag(tags, "amenity") {
        match amenity {
            "school" | "college" | "university" => return Some(Use::School),
            "kindergarten" | "childcare" => return Some(Use::Kindergarten),
            "hospital" => return Some(Use::Hospital),
            "place_of_worship" => return Some(Use::Worship(faith(tags))),
            "bus_station" => return Some(Use::Station),
            _ => {}
        }
    }
    if tag(tags, "leisure") == Some("sports_centre") {
        return Some(Use::SportsHall);
    }
    if tag(tags, "tourism") == Some("museum") {
        return Some(Use::Museum);
    }
    match tag(tags, "landuse") {
        Some("industrial") => Some(Use::Factory),
        Some("farmyard") => Some(Use::Barn),
        Some("retail") => Some(Use::Shop(Goods::General)),
        _ => None,
    }
}

/// Uses an area may only lend to buildings big enough to plausibly be the main hall.
pub fn area_use_fits(use_: Use, footprint: usize) -> bool {
    match use_ {
        Use::SportsHall => footprint >= 300,
        Use::Factory => footprint >= 80,
        _ => true,
    }
}

/// A POI inside the footprint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tenant {
    pub use_: Use,
    pub x: i32,
    pub z: i32,
    pub level: Option<i32>,
}

/// One unit of a storey: the whole floor alone, else the cells nearest its anchor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Unit {
    pub use_: Use,
    pub anchor: (i32, i32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct InteriorPlan {
    pub floors: Vec<Vec<Unit>>,
    /// The building is one tall room: no intermediate ceilings.
    pub open_hall: bool,
}

pub struct PlanInputs<'a> {
    pub tags: &'a HashMap<String, String>,
    pub building_type: &'a str,
    pub floors: usize,
    /// Lowest level of this outline; 0 for anything standing on the ground.
    pub min_level: i32,
    /// Starts above the ground, by level or by height.
    pub elevated: bool,
    pub footprint: usize,
    pub center: (i32, i32),
    pub tenants: &'a [Tenant],
    pub area: Option<Use>,
}

/// First number of a `level` value such as "1", "0;1" or "-1".
pub fn parse_level(value: &str) -> Option<i32> {
    let first = value.split([';', ',']).next()?.trim();
    first.parse::<f64>().ok().map(|l| l.round() as i32)
}

/// Least footprint each unit of a split storey should get.
const CELLS_PER_UNIT: usize = 36;
/// Cells per tenant above which the host keeps a unit of its own on that floor.
const HOST_SHARE_CELLS: usize = 140;
/// Tenants this close and of one use are one business mapped twice.
const SAME_TENANT_DIST: i32 = 4;

/// Spread the building's uses over its floors.
pub fn plan_interior(input: &PlanInputs) -> InteriorPlan {
    let floors = input.floors.max(1);
    let own = use_from_tags(input.tags);
    // A hotel tagged on a plain outline that also names its restaurant is still a hotel.
    let lodging = matches!(
        tag(input.tags, "tourism"),
        Some("hotel" | "hostel" | "guest_house" | "motel")
    )
    .then_some(Use::Hotel);
    let typed = use_from_building_type(input.building_type, input.tags).or(lodging);
    let area = input
        .area
        .filter(|u| typed.is_none() && area_use_fits(*u, input.footprint));
    let base = typed.or(area);

    // What a floor without its own tenant is used for.
    let (ground_default, upper_default) = match (own, base) {
        (Some(o), Some(b)) if o.is_street_tenant() && o != b && b.hosts_tenants() => (o, b),
        (Some(o), None) if o.is_street_tenant() && floors >= 2 => (o, Use::Home),
        (Some(o), _) => (o, o),
        (None, Some(b)) => (b, b),
        (None, None) => (Use::Home, Use::Home),
    };
    let host = base.or(own).unwrap_or(Use::Home);
    // A POI naming the building's own use is the building itself, not a tenant.
    let tenants: Vec<&Tenant> = input
        .tenants
        .iter()
        .filter(|t| Some(t.use_) != typed && Some(t.use_) != own)
        .collect();

    // One tall room when nothing divides it.
    let levels_tagged = input
        .tags
        .get("building:levels")
        .and_then(|l| l.trim().parse::<f64>().ok())
        .is_some_and(|l| l >= 2.0);
    let open_hall = tenants.is_empty()
        && ground_default == upper_default
        && ground_default.is_hall()
        && (ground_default.always_open() || !levels_tagged);
    if open_hall {
        return InteriorPlan {
            floors: vec![vec![Unit {
                use_: ground_default,
                anchor: input.center,
            }]],
            open_hall: true,
        };
    }

    let mut per_floor: Vec<Vec<Unit>> = vec![Vec::new(); floors];
    let top = floors as i32 - 1;
    // A guessed storey count is often too low, so a higher level is only rejected when
    // the outline's own height is mapped.
    let height_mapped = ["building:levels", "height"]
        .iter()
        .any(|k| input.tags.contains_key(*k));
    // Tenants of an elevated part only count when their level says they are in it.
    let floor_of = |level: Option<i32>| -> Option<usize> {
        match level {
            _ if input.elevated && input.min_level == 0 => None,
            Some(l) if l < input.min_level => None,
            Some(l) if height_mapped && l - input.min_level > top => None,
            Some(l) => Some((l - input.min_level).min(top) as usize),
            None if input.elevated => None,
            None => Some(0),
        }
    };
    let push = |floor: usize, unit: Unit, per_floor: &mut Vec<Vec<Unit>>| {
        let dup = per_floor[floor].iter().any(|u| {
            u.use_ == unit.use_
                && (u.anchor.0 - unit.anchor.0).abs() <= SAME_TENANT_DIST
                && (u.anchor.1 - unit.anchor.1).abs() <= SAME_TENANT_DIST
        });
        if !dup {
            per_floor[floor].push(unit);
        }
    };
    // Shops first, so an office without a level can see whether the ground floor is taken.
    let mut ordered = tenants;
    ordered.sort_by_key(|t| t.use_.prefers_upper_floor() && t.level.is_none());
    for t in ordered {
        let Some(mut floor) = floor_of(t.level) else {
            continue;
        };
        if t.level.is_none()
            && t.use_.prefers_upper_floor()
            && floors >= 2
            && per_floor[0].iter().any(|u| u.use_.is_street_tenant())
        {
            floor = 1;
        }
        let unit = Unit {
            use_: t.use_,
            anchor: (t.x, t.z),
        };
        push(floor, unit, &mut per_floor);
    }

    let max_units = (input.footprint / CELLS_PER_UNIT).max(1);
    for (i, units) in per_floor.iter_mut().enumerate() {
        if units.is_empty() {
            let use_ = if i == 0 && !input.elevated {
                ground_default
            } else {
                upper_default
            };
            units.push(Unit {
                use_,
                anchor: input.center,
            });
            continue;
        }
        // A large building keeps part of the tenant floor for its own use.
        let explicit_host = base.is_some() || floors >= 3;
        if explicit_host
            && input.footprint >= HOST_SHARE_CELLS * (units.len() + 1)
            && !units.iter().any(|u| u.use_ == host)
        {
            units.push(Unit {
                use_: host,
                anchor: input.center,
            });
        }
        if units.len() > max_units {
            // Stable, so equal ranks keep their mapping order.
            units.sort_by_key(|u| std::cmp::Reverse(u.use_.rank()));
            units.truncate(max_units);
        }
    }

    InteriorPlan {
        floors: per_floor,
        open_hall: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn plan(
        pairs: &[(&str, &str)],
        floors: usize,
        footprint: usize,
        tenants: &[Tenant],
    ) -> InteriorPlan {
        let t = tags(pairs);
        let building_type = t.get("building").cloned().unwrap_or_else(|| "yes".into());
        plan_interior(&PlanInputs {
            tags: &t,
            building_type: &building_type,
            floors,
            min_level: 0,
            elevated: false,
            footprint,
            center: (0, 0),
            tenants,
            area: None,
        })
    }

    fn uses(plan: &InteriorPlan) -> Vec<Vec<Use>> {
        plan.floors
            .iter()
            .map(|f| f.iter().map(|u| u.use_).collect())
            .collect()
    }

    fn tenant(use_: Use, x: i32, level: Option<i32>) -> Tenant {
        Tenant {
            use_,
            x,
            z: 0,
            level,
        }
    }

    #[test]
    fn poi_tags_name_their_use() {
        assert_eq!(
            use_from_tags(&tags(&[("shop", "bakery")])),
            Some(Use::Shop(Goods::Bakery))
        );
        assert_eq!(
            use_from_tags(&tags(&[("amenity", "cafe")])),
            Some(Use::Food(Eatery::Cafe))
        );
        assert_eq!(
            use_from_tags(&tags(&[
                ("amenity", "place_of_worship"),
                ("religion", "muslim")
            ])),
            Some(Use::Worship(Faith::Muslim))
        );
        assert_eq!(
            use_from_tags(&tags(&[("shop", "supermarket")])),
            Some(Use::Supermarket)
        );
        assert_eq!(use_from_tags(&tags(&[("amenity", "bench")])), None);
        assert_eq!(use_from_tags(&tags(&[("shop", "vacant")])), None);
        assert_eq!(
            use_from_tags(&tags(&[("office", "lawyer")])),
            Some(Use::Office)
        );
    }

    #[test]
    fn synagogues_and_mosques_keep_their_faith_without_a_religion_tag() {
        assert_eq!(
            use_from_building_type("synagogue", &tags(&[("building", "synagogue")])),
            Some(Use::Worship(Faith::Jewish))
        );
        assert_eq!(
            use_from_building_type("mosque", &tags(&[("building", "mosque")])),
            Some(Use::Worship(Faith::Muslim))
        );
        assert_eq!(
            use_from_tags(&tags(&[
                ("amenity", "place_of_worship"),
                ("religion", "jewish")
            ])),
            Some(Use::Worship(Faith::Jewish))
        );
    }

    #[test]
    fn levels_parse_from_their_first_value() {
        assert_eq!(parse_level("1"), Some(1));
        assert_eq!(parse_level("0;1"), Some(0));
        assert_eq!(parse_level("-1"), Some(-1));
        assert_eq!(parse_level("2.5"), Some(3));
        assert_eq!(parse_level("ground"), None);
    }

    #[test]
    fn a_shop_in_an_apartment_block_takes_only_the_ground_floor() {
        let p = plan(
            &[("building", "apartments"), ("shop", "bakery")],
            4,
            200,
            &[],
        );
        assert_eq!(
            uses(&p),
            vec![
                vec![Use::Shop(Goods::Bakery)],
                vec![Use::Home],
                vec![Use::Home],
                vec![Use::Home],
            ]
        );
        assert!(!p.open_hall);
    }

    #[test]
    fn a_shop_poi_in_a_plain_building_keeps_homes_above_it() {
        let shop = [tenant(Use::Shop(Goods::Clothes), 3, None)];
        let p = plan(&[("building", "yes")], 3, 150, &shop);
        assert_eq!(
            uses(&p),
            vec![
                vec![Use::Shop(Goods::Clothes)],
                vec![Use::Home],
                vec![Use::Home],
            ]
        );
    }

    #[test]
    fn a_single_storey_supermarket_is_the_whole_building() {
        let p = plan(
            &[("building", "retail"), ("shop", "supermarket")],
            1,
            2000,
            &[],
        );
        assert_eq!(uses(&p), vec![vec![Use::Supermarket]]);
    }

    #[test]
    fn several_shops_split_the_ground_floor() {
        let shops = [
            tenant(Use::Shop(Goods::Bakery), 2, None),
            tenant(Use::Food(Eatery::Cafe), 20, None),
            tenant(Use::Shop(Goods::Pharmacy), 40, Some(0)),
        ];
        let p = plan(&[("building", "yes")], 2, 300, &shops);
        assert_eq!(p.floors[0].len(), 3);
        assert_eq!(uses(&p)[1], vec![Use::Home]);
    }

    #[test]
    fn an_office_without_level_moves_above_the_shops() {
        let pois = [
            tenant(Use::Shop(Goods::Books), 2, None),
            tenant(Use::Office, 10, None),
        ];
        let p = plan(&[("building", "yes")], 3, 160, &pois);
        assert_eq!(uses(&p)[0], vec![Use::Shop(Goods::Books)]);
        assert_eq!(uses(&p)[1], vec![Use::Office]);
        assert_eq!(uses(&p)[2], vec![Use::Home]);
    }

    #[test]
    fn level_tags_place_tenants_on_their_floor() {
        let pois = [tenant(Use::Clinic, 2, Some(2))];
        let p = plan(&[("building", "apartments")], 4, 200, &pois);
        assert_eq!(uses(&p)[2], vec![Use::Clinic]);
        assert_eq!(uses(&p)[0], vec![Use::Home]);
    }

    #[test]
    fn churches_and_halls_open_up() {
        let p = plan(&[("building", "church")], 3, 400, &[]);
        assert!(p.open_hall);
        assert_eq!(uses(&p), vec![vec![Use::Worship(Faith::Christian)]]);

        // A warehouse mapped with real levels keeps them.
        let p = plan(
            &[("building", "warehouse"), ("building:levels", "3")],
            3,
            400,
            &[],
        );
        assert!(!p.open_hall);
        assert_eq!(p.floors.len(), 3);
    }

    #[test]
    fn a_poi_naming_the_building_itself_is_no_tenant() {
        let node = [tenant(Use::Worship(Faith::Christian), 4, None)];
        let p = plan(&[("building", "church")], 3, 400, &node);
        assert!(p.open_hall, "the church stays one room");
    }

    #[test]
    fn a_part_lifted_by_height_takes_no_street_shop() {
        let t = tags(&[("building:part", "yes"), ("min_height", "20")]);
        let shop = [tenant(Use::Shop(Goods::Clothes), 3, None)];
        let p = plan_interior(&PlanInputs {
            tags: &t,
            building_type: "yes",
            floors: 3,
            min_level: 0,
            elevated: true,
            footprint: 200,
            center: (0, 0),
            tenants: &shop,
            area: None,
        });
        assert!(p.floors.iter().flatten().all(|u| u.use_ == Use::Home));
    }

    #[test]
    fn a_level_above_a_mapped_outline_belongs_elsewhere() {
        let high = [tenant(Use::Clinic, 2, Some(5))];
        let p = plan(
            &[("building", "apartments"), ("building:levels", "2")],
            2,
            200,
            &high,
        );
        assert!(p.floors.iter().flatten().all(|u| u.use_ == Use::Home));
        // Without mapped levels the storey count is a guess, and the tenant stays.
        let p = plan(&[("building", "apartments")], 2, 200, &high);
        assert_eq!(uses(&p)[1], vec![Use::Clinic]);
    }

    #[test]
    fn a_big_host_keeps_room_beside_a_small_tenant() {
        let kiosk = [tenant(Use::Shop(Goods::General), 2, None)];
        let p = plan(&[("building", "apartments")], 5, 900, &kiosk);
        assert_eq!(
            uses(&p)[0],
            vec![Use::Shop(Goods::General), Use::Home],
            "the rest of the ground floor stays flats"
        );
    }

    #[test]
    fn a_crowded_small_floor_keeps_its_strongest_tenant() {
        let pois = [
            tenant(Use::Office, 1, Some(0)),
            tenant(Use::Supermarket, 3, Some(0)),
        ];
        let p = plan(&[("building", "yes")], 1, 40, &pois);
        assert_eq!(uses(&p), vec![vec![Use::Supermarket]]);
    }

    #[test]
    fn untagged_buildings_stay_homes() {
        let p = plan(&[("building", "yes")], 2, 150, &[]);
        assert_eq!(uses(&p), vec![vec![Use::Home], vec![Use::Home]]);
    }

    #[test]
    fn an_area_lends_its_use_to_plain_buildings_only() {
        let t = tags(&[("building", "yes")]);
        let p = plan_interior(&PlanInputs {
            tags: &t,
            building_type: "yes",
            floors: 2,
            min_level: 0,
            elevated: false,
            footprint: 300,
            center: (0, 0),
            tenants: &[],
            area: Some(Use::School),
        });
        assert_eq!(uses(&p), vec![vec![Use::School], vec![Use::School]]);

        let t = tags(&[("building", "house")]);
        let p = plan_interior(&PlanInputs {
            tags: &t,
            building_type: "house",
            floors: 1,
            min_level: 0,
            elevated: false,
            footprint: 100,
            center: (0, 0),
            tenants: &[],
            area: Some(Use::School),
        });
        assert_eq!(uses(&p), vec![vec![Use::Home]]);
    }
}
