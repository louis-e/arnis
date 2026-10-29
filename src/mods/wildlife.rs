//! Entity mods: Automobility cars on parking spaces and Naturalist animals in zoos and woods.

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

/// Zoo animals for an enclosure, or the odd wild animal for a wood or a wetland.
pub fn populate_way(
    editor: &mut WorldEditor,
    way: &crate::osm_parser::ProcessedWay,
    flood_fill_cache: &crate::floodfill_cache::FloodFillCache,
    timeout: Option<&std::time::Duration>,
) {
    let tags = &way.tags;
    let habitat = tags
        .get("natural")
        .or_else(|| {
            tags.get("landuse")
                .filter(|v| matches!(v.as_str(), "forest" | "meadow"))
        })
        .map(String::as_str);
    let species = enclosure_species(tags);
    // Fill only areas that get animals: no coastlines, and no wild ones in small patches.
    let pool = habitat.and_then(wild_pool);
    let (x0, x1) = way.nodes.iter().fold((i32::MAX, i32::MIN), |(lo, hi), n| {
        (lo.min(n.x), hi.max(n.x))
    });
    let (z0, z1) = way.nodes.iter().fold((i32::MAX, i32::MIN), |(lo, hi), n| {
        (lo.min(n.z), hi.max(n.z))
    });
    let roomy = x1 >= x0 && (x1 - x0 + 1) as i64 * (z1 - z0 + 1) as i64 >= WILD_CELLS as i64;
    if species.is_none() && !(pool.is_some() && roomy) {
        return;
    }
    let cells = flood_fill_cache.get_or_compute(way, timeout);
    if let Some(species) = species {
        populate_enclosure(editor, species, &cells, way.id);
    } else if let Some(pool) = pool {
        populate_wild(editor, pool, &cells, way.id);
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

/// Cells of habitat per wild animal.
const WILD_CELLS: usize = 2500;

/// The wild animals of a wood, a wetland or open grass.
fn wild_pool(habitat: &str) -> Option<&'static [&'static str]> {
    Some(match habitat {
        "forest" | "wood" => &["deer", "deer", "boar", "bird", "snail", "bird"],
        "wetland" | "marsh" | "swamp" | "reedbed" => &["duck", "duck", "dragonfly", "snail"],
        "meadow" | "grassland" | "heath" => &["butterfly", "butterfly", "bird"],
        _ => return None,
    })
}

/// Wildlife from `pool`, rarely: the mod also spawns its own.
pub fn populate_wild(editor: &mut WorldEditor, pool: &[&str], cells: &[(i32, i32)], seed: u64) {
    let count = cells.len() / WILD_CELLS;
    if count == 0 {
        return;
    }
    spawn_among(editor, pool, cells, count.min(6), seed);
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
        let yaw = ((h >> 24) % 360) as f32;
        let extra = HashMap::from([
            ("PersistenceRequired".to_string(), Value::Byte(1)),
            (
                "Rotation".to_string(),
                Value::List(vec![Value::Float(yaw), Value::Float(0.0)]),
            ),
        ]);
        editor.add_entity(&format!("naturalist:{kind}"), x, 1, z, Some(extra));
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
    fn only_habitats_with_animals_have_a_pool() {
        assert!(wild_pool("wood").is_some() && wild_pool("wetland").is_some());
        assert!(wild_pool("coastline").is_none() && wild_pool("water").is_none());
    }
}
