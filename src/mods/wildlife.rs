//! Entity mods: Automobility cars on parking spaces, and Naturalist animals in zoos and
//! wherever they would live: woods, wetlands, grass, beaches, parks and ponds.

use crate::climate::Climate;
use crate::ecoregion::Realm;
use crate::world_editor::WorldEditor;
use fastnbt::Value;
use std::collections::HashMap;

/// A drivable car parked at a space centre, its nose along quarter-turn `rot`.
pub fn park_automobile(editor: &mut WorldEditor, x: i32, base_y: i32, z: i32, rot: u8, h: u64) {
    const PAINT: [&str; 10] = [
        "white",
        "black",
        "gray",
        "light_gray",
        "red",
        "blue",
        "green",
        "yellow",
        "brown",
        "cyan",
    ];
    let paint = PAINT[(h % PAINT.len() as u64) as usize];
    // A few classic motorcars among the everyday ones.
    let frame = match (h >> 12) % 40 {
        0 => "steel_motorcar".to_string(),
        1 => "copper_motorcar".to_string(),
        2 => "wooden_motorcar".to_string(),
        _ => format!("standard_{paint}"),
    };
    let wheels = if (h >> 20).is_multiple_of(3) {
        "street"
    } else {
        "standard"
    };
    automobile(editor, x, base_y, z, rot, &frame, wheels, "iron");
}

/// A tractor parked in a field.
pub fn park_tractor(editor: &mut WorldEditor, x: i32, base_y: i32, z: i32, rot: u8, h: u64) {
    let paint = ["red", "green", "blue", "yellow"][(h % 4) as usize];
    automobile(
        editor,
        x,
        base_y,
        z,
        rot,
        &format!("{paint}_tractor"),
        "tractor",
        "stone",
    );
}

/// A shopping trolley, pointing along quarter-turn `rot`.
pub fn place_trolley(editor: &mut WorldEditor, x: i32, base_y: i32, z: i32, rot: u8) {
    automobile(editor, x, base_y, z, rot, "shopping_cart", "steel", "empty");
}

#[allow(clippy::too_many_arguments)]
fn automobile(
    editor: &mut WorldEditor,
    x: i32,
    base_y: i32,
    z: i32,
    rot: u8,
    frame: &str,
    wheels: &str,
    engine: &str,
) {
    if !editor.owns(x, z) {
        return;
    }
    let yaw = f32::from(rot & 3) * 90.0;
    let extra = HashMap::from([
        (
            "frame".to_string(),
            Value::String(format!("automobility:{frame}")),
        ),
        (
            "wheels".to_string(),
            Value::String(format!("automobility:{wheels}")),
        ),
        (
            "engine".to_string(),
            Value::String(format!("automobility:{engine}")),
        ),
        (
            "Rotation".to_string(),
            Value::List(vec![Value::Float(yaw), Value::Float(0.0)]),
        ),
        ("PersistenceRequired".to_string(), Value::Byte(1)),
    ]);
    editor.add_entity_exact(
        "automobility:automobile",
        x as f64 + 0.5,
        base_y as f64,
        z as f64 + 0.5,
        i64::from(rot),
        extra,
    );
}

/// Naturalist species an enclosure is named for, in the languages zoos are mapped in most.
const SPECIES: &[(&str, &[&str])] = &[
    (
        "lion",
        &[
            "lion", "löwe", "loewe", "leeuw", "león", "leone", "leão", "lew",
        ],
    ),
    ("tiger", &["tiger", "tigre", "tijger", "tygrys"]),
    (
        "elephant",
        &[
            "elephant",
            "elefant",
            "éléphant",
            "olifant",
            "elefante",
            "słoń",
        ],
    ),
    (
        "giraffe",
        &["giraffe", "girafe", "giraf", "jirafa", "giraffa", "żyrafa"],
    ),
    ("zebra", &["zebra", "zèbre", "cebra"]),
    (
        "hippo",
        &[
            "hippo",
            "flusspferd",
            "nilpferd",
            "nijlpaard",
            "hipopótamo",
            "ippopotamo",
        ],
    ),
    (
        "rhino",
        &["rhino", "nashorn", "neushoorn", "rinoceronte", "nosorożec"],
    ),
    // Before "bear", whose word would take "black bear" first.
    ("black_bear", &["black bear", "schwarzbär", "baribal"]),
    (
        "bear",
        &[
            "brown bear",
            "grizzly",
            "braunbär",
            "ours brun",
            "oso pardo",
            "bear",
            "bär",
            "ours",
            "oso",
            "orso",
            "niedźwiedź",
        ],
    ),
    (
        "deer",
        &[
            "deer", "hirsch", "reh", "cerf", "ciervo", "cervo", "hert", "jeleń", "elk", "wapiti",
        ],
    ),
    (
        "boar",
        &[
            "boar",
            "wildschwein",
            "sanglier",
            "jabalí",
            "cinghiale",
            "dzik",
        ],
    ),
    (
        "capybara",
        &[
            "capybara",
            "wasserschwein",
            "carpincho",
            "capivara",
            "kapibara",
        ],
    ),
    (
        "ostrich",
        &[
            "ostrich", "strauß", "strauss", "autruche", "avestruz", "struzzo", "struś",
        ],
    ),
    (
        "alligator",
        &[
            "alligator",
            "crocodile",
            "krokodil",
            "cocodrilo",
            "caimán",
            "kaiman",
        ],
    ),
    ("komodo_dragon", &["komodo"]),
    (
        "vulture",
        &["vulture", "geier", "vautour", "buitre", "avvoltoio", "sęp"],
    ),
    (
        "tortoise",
        &[
            "tortoise",
            "schildkröte",
            "tortue",
            "tortuga",
            "tartaruga",
            "żółw",
        ],
    ),
    (
        "snake",
        &["snake", "schlange", "serpent", "serpiente", "python", "boa"],
    ),
    (
        "lizard",
        &["lizard", "echse", "lézard", "lagarto", "iguana", "waran"],
    ),
    (
        "duck",
        &["duck", "ente", "canard", "pato", "anatra", "kaczka"],
    ),
    ("turkey", &["turkey", "truthahn", "pute", "dinde", "pavo"]),
    ("capybara", &["tapir"]),
];

/// The Naturalist animal an enclosure keeps, from its tags.
pub fn enclosure_species(tags: &HashMap<String, String>) -> Option<&'static str> {
    if !(tags.get("attraction").is_some_and(|v| v == "animal")
        || tags.get("zoo").is_some_and(|v| v == "enclosure"))
    {
        return None;
    }
    let text: String = [
        "animal",
        "species:en",
        "species",
        "name:en",
        "name",
        "description",
    ]
    .iter()
    .filter_map(|k| tags.get(*k))
    .map(|v| v.to_lowercase())
    .collect::<Vec<_>>()
    .join(" ");
    SPECIES
        .iter()
        .find(|(_, words)| {
            words.iter().any(|w| {
                text.split(|c: char| !c.is_alphanumeric())
                    .any(|token| token.starts_with(w))
                    || (w.contains(' ') && text.contains(w))
            })
        })
        .map(|(id, _)| *id)
}

/// Where wild animals live, from a way's tags.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Habitat {
    Wood,
    Wetland,
    Grass,
    Scrub,
    Beach,
    Park,
    /// A pond or lake, with ducks on it.
    Pond,
}

impl Habitat {
    fn of(tags: &HashMap<String, String>) -> Option<Habitat> {
        let tag = |k: &str| tags.get(k).map(String::as_str);
        Some(match (tag("natural"), tag("landuse"), tag("leisure")) {
            (Some("wood"), ..) | (_, Some("forest"), _) => Habitat::Wood,
            (Some("wetland" | "marsh" | "swamp" | "reedbed"), ..) => Habitat::Wetland,
            (Some("grassland" | "fell"), ..) | (_, Some("meadow"), _) => Habitat::Grass,
            (Some("heath" | "scrub"), ..) => Habitat::Scrub,
            (Some("beach"), ..) => Habitat::Beach,
            (Some("water"), ..)
                if matches!(
                    tag("water"),
                    None | Some("pond" | "lake" | "basin" | "oxbow")
                ) =>
            {
                Habitat::Pond
            }
            (_, _, Some("park" | "garden")) | (_, Some("village_green"), _) => Habitat::Park,
            _ => return None,
        })
    }

    /// Cells of habitat per animal.
    fn cells_per_animal(self) -> usize {
        match self {
            Habitat::Park => 350,
            Habitat::Beach | Habitat::Pond => 400,
            Habitat::Wetland => 500,
            Habitat::Wood => 700,
            Habitat::Grass | Habitat::Scrub => 900,
        }
    }
}

/// The wild animals of a habitat where it lies: by biogeographic realm and climate, so a
/// Florida swamp has alligators and a Spessart wood boars.
fn wild_pool(
    habitat: Habitat,
    realm: Option<Realm>,
    climate: Climate,
    lat: f64,
) -> &'static [&'static str] {
    let tropical = lat.abs() < 23.5;
    let dry = matches!(
        climate,
        Climate::HotDesert | Climate::ColdDesert | Climate::HotSteppe
    );
    let cold = matches!(climate, Climate::Boreal | Climate::Tundra | Climate::IceCap);
    match habitat {
        // Palm groves and park trees in a desert keep desert animals, not deer and boars.
        Habitat::Wood | Habitat::Park if dry => &["bird", "bird", "lizard", "snake"],
        Habitat::Wood => match realm {
            _ if cold => &["deer", "deer", "bear", "bird", "bird"],
            Some(Realm::Nearctic) => &["deer", "deer", "black_bear", "turkey", "bird", "snail"],
            Some(Realm::Neotropic) if tropical => &["bird", "bird", "snake", "capybara", "lizard"],
            _ if tropical => &["bird", "bird", "snake", "lizard"],
            _ => &["deer", "deer", "boar", "bird", "bird", "snail", "hedgehog"],
        },
        Habitat::Wetland => match realm {
            Some(Realm::Nearctic) if lat.abs() < 35.0 => {
                &["alligator", "duck", "duck", "dragonfly", "firefly"]
            }
            Some(Realm::Neotropic) => &["capybara", "duck", "duck", "dragonfly"],
            _ => &["duck", "duck", "dragonfly", "snail", "firefly"],
        },
        Habitat::Grass => match realm {
            Some(Realm::Afrotropic) if dry || climate == Climate::TropicalSavanna => &[
                "zebra", "zebra", "giraffe", "elephant", "ostrich", "vulture",
            ],
            _ if dry => &["bird", "lizard", "butterfly"],
            Some(Realm::Nearctic) => &["turkey", "butterfly", "butterfly", "bird"],
            _ => &["butterfly", "butterfly", "bird", "hedgehog"],
        },
        Habitat::Scrub if dry => &["lizard", "snake", "desert_scorpion", "vulture", "tortoise"],
        Habitat::Scrub => &["butterfly", "bird", "lizard", "snake"],
        // Crabs on warm shores; a beach on a northern lake or river has birds.
        Habitat::Beach if !cold && lat.abs() < 40.0 => &["crab", "crab", "bird"],
        Habitat::Beach => &["bird"],
        Habitat::Park => match realm {
            Some(Realm::Palearctic) | None if !tropical => {
                &["bird", "bird", "butterfly", "hedgehog", "snail"]
            }
            _ => &["bird", "bird", "butterfly", "lizard"],
        },
        Habitat::Pond => &["duck"],
    }
}

/// Zoo animals for an enclosure, or wild ones for a wood, a wetland, grass, a beach, a
/// park or a pond.
pub fn populate_way(
    editor: &mut WorldEditor,
    way: &crate::osm_parser::ProcessedWay,
    flood_fill_cache: &crate::floodfill_cache::FloodFillCache,
    timeout: Option<&std::time::Duration>,
) {
    let species = enclosure_species(&way.tags);
    let habitat = Habitat::of(&way.tags);
    // Fill only areas that get animals: no coastlines, and no patch too small for one.
    let (x0, x1) = way.nodes.iter().fold((i32::MAX, i32::MIN), |(lo, hi), n| {
        (lo.min(n.x), hi.max(n.x))
    });
    let (z0, z1) = way.nodes.iter().fold((i32::MAX, i32::MIN), |(lo, hi), n| {
        (lo.min(n.z), hi.max(n.z))
    });
    let area = if x1 >= x0 {
        (x1 - x0 + 1) as usize * (z1 - z0 + 1) as usize
    } else {
        0
    };
    let roomy = habitat.is_some_and(|h| area * 2 >= h.cells_per_animal());
    if species.is_none() && !roomy {
        return;
    }
    let cells = flood_fill_cache.get_or_compute(way, timeout);
    if let Some(species) = species {
        populate_enclosure(editor, species, &cells, way.id);
    } else if let Some(habitat) = habitat {
        populate_wild(editor, habitat, &cells, way.id);
    }
}

/// Animals inside an enclosure, about one per 60 cells, at most eight.
pub fn populate_enclosure(
    editor: &mut WorldEditor,
    species: &str,
    cells: &[(i32, i32)],
    seed: u64,
) {
    let count = (cells.len() / 60).clamp(1, 8);
    spawn_among(editor, &[species], cells, count, seed);
}

/// Wildlife for a habitat, often enough to meet on a walk.
fn populate_wild(editor: &mut WorldEditor, habitat: Habitat, cells: &[(i32, i32)], seed: u64) {
    let Some(&(x, z)) = cells.first() else {
        return;
    };
    let per = habitat.cells_per_animal();
    let count = ((cells.len() + per / 2) / per).min(16);
    if count == 0 {
        return;
    }
    let realm = editor.ecoregion(x, z).map(|e| e.realm);
    let pool = wild_pool(habitat, realm, editor.climate(), editor.geo_anchor().0);
    if habitat == Habitat::Pond {
        float_on_water(editor, pool, cells, count.min(8), seed);
    } else {
        spawn_among(editor, pool, cells, count, seed);
    }
}

/// Swimmers at the water line of a pond.
fn float_on_water(
    editor: &mut WorldEditor,
    pool: &[&str],
    cells: &[(i32, i32)],
    count: usize,
    seed: u64,
) {
    for i in 0..count {
        let h = stable_hash(seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let (x, z) = cells[(h % cells.len() as u64) as usize];
        if !editor.owns(x, z) {
            continue;
        }
        let kind = pool[((h >> 16) % pool.len() as u64) as usize];
        let y = editor.get_water_level(x, z) + 1;
        editor.add_entity_exact(
            &format!("naturalist:{kind}"),
            x as f64 + 0.5,
            y as f64,
            z as f64 + 0.5,
            h as i64,
            animal_tags(h),
        );
    }
}

fn animal_tags(h: u64) -> HashMap<String, Value> {
    let yaw = ((h >> 24) % 360) as f32;
    HashMap::from([
        ("PersistenceRequired".to_string(), Value::Byte(1)),
        (
            "Rotation".to_string(),
            Value::List(vec![Value::Float(yaw), Value::Float(0.0)]),
        ),
    ])
}

fn spawn_among(
    editor: &mut WorldEditor,
    pool: &[&str],
    cells: &[(i32, i32)],
    count: usize,
    seed: u64,
) {
    if cells.is_empty() {
        return;
    }
    for i in 0..count {
        let h = stable_hash(seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let (x, z) = cells[(h % cells.len() as u64) as usize];
        if !editor.owns(x, z) || editor.is_lc_water(x, z) {
            continue;
        }
        // Room to stand, not inside a trunk or a wall.
        let ground = editor.get_absolute_y(x, 0, z);
        if (1..=2).any(|dy| editor.get_block_absolute(x, ground + dy, z).is_some()) {
            continue;
        }
        let kind = pool[((h >> 16) % pool.len() as u64) as usize];
        editor.add_entity(&format!("naturalist:{kind}"), x, 1, z, Some(animal_tags(h)));
    }
}

fn stable_hash(mut h: u64) -> u64 {
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
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

    #[test]
    fn enclosures_name_their_animal() {
        let t = tags(&[("attraction", "animal"), ("name", "Löwenanlage")]);
        assert_eq!(enclosure_species(&t), Some("lion"));
        let t = tags(&[("attraction", "animal"), ("name", "Elefantenhaus")]);
        assert_eq!(enclosure_species(&t), Some("elephant"));
        let t = tags(&[("zoo", "enclosure"), ("animal", "giraffe")]);
        assert_eq!(enclosure_species(&t), Some("giraffe"));
        assert_eq!(enclosure_species(&tags(&[("name", "Lion Bar")])), None);
        let t = tags(&[("attraction", "animal"), ("name", "Ziegen")]);
        assert_eq!(enclosure_species(&t), None);
        let t = tags(&[("attraction", "animal"), ("name", "Black Bear Exhibit")]);
        assert_eq!(enclosure_species(&t), Some("black_bear"));
        let t = tags(&[("attraction", "animal"), ("name", "Bärenwald")]);
        assert_eq!(enclosure_species(&t), Some("bear"));
    }

    #[test]
    fn habitats_come_from_the_tags_that_map_them() {
        assert_eq!(
            Habitat::of(&tags(&[("natural", "wood")])),
            Some(Habitat::Wood)
        );
        assert_eq!(
            Habitat::of(&tags(&[("leisure", "park")])),
            Some(Habitat::Park)
        );
        let pond = tags(&[("natural", "water"), ("water", "pond")]);
        assert_eq!(Habitat::of(&pond), Some(Habitat::Pond));
        let river = tags(&[("natural", "water"), ("water", "river")]);
        assert_eq!(Habitat::of(&river), None);
        assert_eq!(Habitat::of(&tags(&[("natural", "coastline")])), None);
    }

    #[test]
    fn wildlife_follows_the_region() {
        let t = Climate::Temperate;
        let europe = wild_pool(Habitat::Wood, Some(Realm::Palearctic), t, 50.0);
        assert!(europe.contains(&"boar") && !europe.contains(&"black_bear"));
        let florida = wild_pool(Habitat::Wetland, Some(Realm::Nearctic), t, 26.0);
        assert!(florida.contains(&"alligator"));
        let maine = wild_pool(Habitat::Wetland, Some(Realm::Nearctic), t, 45.0);
        assert!(!maine.contains(&"alligator"));
        let serengeti = wild_pool(
            Habitat::Grass,
            Some(Realm::Afrotropic),
            Climate::TropicalSavanna,
            -2.5,
        );
        assert!(serengeti.contains(&"zebra"));
        let boreal = wild_pool(
            Habitat::Wood,
            Some(Realm::Palearctic),
            Climate::Boreal,
            62.0,
        );
        assert!(boreal.contains(&"bear"));
        // Egypt is Palearctic too, but its palm groves and parks hold no deer or hedgehogs.
        let luxor = Some(Realm::Palearctic);
        for habitat in [Habitat::Wood, Habitat::Park, Habitat::Grass] {
            let pool = wild_pool(habitat, luxor, Climate::HotDesert, 25.7);
            assert!(
                !pool.contains(&"deer") && !pool.contains(&"hedgehog"),
                "{habitat:?}"
            );
        }
        let lapland = wild_pool(Habitat::Beach, luxor, Climate::Boreal, 66.5);
        assert!(!lapland.contains(&"crab"));
    }
}
