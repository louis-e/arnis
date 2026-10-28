//! Moving vehicles for Bedrock worlds. A behavior pack holds each vehicle body as an
//! entity and a script that drives them by the lanes, routes and timetables planned for
//! the Java pack; a resource pack draws them. Both go into the .mcworld. They are made for
//! that one world, so each world gets pack ids of its own.

mod models;

#[cfg(test)]
pub use models::knows as palette_has;

use crate::element_processing::cars::CarNetwork;
use crate::element_processing::trains::TrainNetwork;
use serde_json::{json, Value};

/// The newest Script API module 1.21.120 has, the oldest version an Arnis world opens in.
const SERVER_MODULE: &str = "2.3.0";
const MIN_ENGINE: [u32; 3] = [1, 21, 120];
const ENTITY_FORMAT: &str = "1.21.90";
const TEXTURE: &str = "textures/entity/arnis_vehicles";

const SCRIPTS: [(&str, &[u8]); 4] = [
    (
        "scripts/main.js",
        include_bytes!("../../assets/minecraft/bedrock_addon/scripts/main.js"),
    ),
    (
        "scripts/cars.js",
        include_bytes!("../../assets/minecraft/bedrock_addon/scripts/cars.js"),
    ),
    (
        "scripts/trains.js",
        include_bytes!("../../assets/minecraft/bedrock_addon/scripts/trains.js"),
    ),
    (
        "scripts/util.js",
        include_bytes!("../../assets/minecraft/bedrock_addon/scripts/util.js"),
    ),
];

/// One box of a vehicle body in blocks: its block, size and lowest corner in the vehicle's
/// frame (+z ahead, the ground or rail at 0), a turn about x round that corner in radians,
/// and how it is lit.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub block: &'static str,
    pub size: [f32; 3],
    pub corner: [f32; 3],
    pub tilt: f32,
    pub lamp: Lamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lamp {
    Off,
    On,
    /// Lit only while the car brakes or stands.
    Brake,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fleet {
    Car,
    Train,
}

impl Fleet {
    fn family(self) -> &'static str {
        match self {
            Fleet::Car => "arnis_car",
            Fleet::Train => "arnis_train",
        }
    }
}

/// A vehicle body, the entity `arnis:<name>`.
pub struct Body {
    pub name: String,
    pub fleet: Fleet,
    pub shapes: Vec<Shape>,
}

pub struct Pack {
    pub id: String,
    pub folder: &'static str,
    /// (path in the pack, contents)
    pub files: Vec<(String, Vec<u8>)>,
}

pub struct Addon {
    pub behavior: Pack,
    pub resources: Pack,
}

/// Numbers as one comma-separated string. The script keeps a long list as text until a
/// vehicle needs it, which takes a quarter of the memory a list of numbers does.
pub fn list<T: std::fmt::Display>(values: impl IntoIterator<Item = T>) -> String {
    let mut out = String::new();
    for (k, v) in values.into_iter().enumerate() {
        if k > 0 {
            out.push(',');
        }
        out.push_str(&v.to_string());
    }
    out
}

/// The add-on for a world's cars and trains, or None when nothing would drive.
/// `rail_y` gives the block a way's rail lies in at a column.
pub fn vehicles(
    cars: Option<&CarNetwork>,
    trains: Option<&TrainNetwork>,
    rail_y: impl Fn(u64, i32, i32) -> Option<i32>,
) -> Option<Addon> {
    let cars = cars.filter(|net| net.car_count() > 0);
    let trains = trains
        .map(|net| net.bedrock_data(rail_y))
        .filter(|(data, _)| data["services"].as_array().is_some_and(|s| !s.is_empty()));
    if cars.is_none() && trains.is_none() {
        return None;
    }
    let mut bodies = Vec::new();
    let mut data = json!({ "cars": null, "trains": null });
    if let Some(net) = cars {
        data["cars"] = net.bedrock_data();
        bodies.extend(net.bedrock_bodies());
    }
    if let Some((train_data, train_bodies)) = trains {
        data["trains"] = train_data;
        bodies.extend(train_bodies);
    }
    Some(build(&data, &bodies, new_uuid))
}

fn build(data: &Value, bodies: &[Body], mut uuid: impl FnMut() -> String) -> Addon {
    let (bp, rp) = (uuid(), uuid());
    let pretty = |v: &Value| serde_json::to_vec_pretty(v).expect("JSON values always serialise");
    let icon = include_bytes!("../../assets/minecraft/icon.png").to_vec();

    let mut behavior: Vec<(String, Vec<u8>)> = vec![
        (
            "manifest.json".to_string(),
            pretty(&json!({
                "format_version": 2,
                "header": {
                    "name": "Arnis Vehicles",
                    "description": "Cars and trains that drive this world's streets and tracks.",
                    "uuid": bp,
                    "version": [1, 0, 0],
                    "min_engine_version": MIN_ENGINE,
                },
                "modules": [
                    { "type": "data", "uuid": uuid(), "version": [1, 0, 0] },
                    {
                        "type": "script",
                        "language": "javascript",
                        "uuid": uuid(),
                        "version": [1, 0, 0],
                        "entry": "scripts/main.js",
                    },
                ],
                "dependencies": [
                    { "module_name": "@minecraft/server", "version": SERVER_MODULE },
                    { "uuid": rp, "version": [1, 0, 0] },
                ],
            })),
        ),
        ("pack_icon.png".to_string(), icon.clone()),
        (
            "scripts/data.js".to_string(),
            data_module(data).into_bytes(),
        ),
    ];
    behavior.extend(SCRIPTS.iter().map(|(p, b)| (p.to_string(), b.to_vec())));

    let mut resources: Vec<(String, Vec<u8>)> = vec![
        (
            "manifest.json".to_string(),
            pretty(&json!({
                "format_version": 2,
                "header": {
                    "name": "Arnis Vehicles",
                    "description": "What this world's cars and trains look like.",
                    "uuid": rp,
                    "version": [1, 0, 0],
                    "min_engine_version": MIN_ENGINE,
                },
                "modules": [{ "type": "resources", "uuid": uuid(), "version": [1, 0, 0] }],
            })),
        ),
        ("pack_icon.png".to_string(), icon),
        (format!("{TEXTURE}.png"), models::texture()),
        (
            "render_controllers/arnis_vehicles.render_controllers.json".to_string(),
            pretty(&render_controllers()),
        ),
        (
            "texts/languages.json".to_string(),
            pretty(&json!(["en_US"])),
        ),
    ];
    let mut lang = String::new();
    for body in bodies {
        behavior.push((
            format!("entities/{}.json", body.name),
            pretty(&behavior_entity(body)),
        ));
        resources.push((
            format!("entity/{}.entity.json", body.name),
            pretty(&client_entity(body)),
        ));
        // Most of the packs by size, so not indented.
        resources.push((
            format!("models/entity/{}.geo.json", body.name),
            serde_json::to_vec(&models::geometry(&body.name, &body.shapes))
                .expect("JSON values always serialise"),
        ));
        let label = match body.fleet {
            Fleet::Car => "Car",
            Fleet::Train => "Train",
        };
        lang.push_str(&format!("entity.arnis:{}.name={label}\n", body.name));
    }
    resources.push(("texts/en_US.lang".to_string(), lang.into_bytes()));

    Addon {
        behavior: Pack {
            id: bp,
            folder: "arnis_vehicles",
            files: behavior,
        },
        resources: Pack {
            id: rp,
            folder: "arnis_vehicles",
            files: resources,
        },
    }
}

/// The data as a module the script imports. JSON.parse reads a big table faster than the
/// same table written as a literal.
fn data_module(data: &Value) -> String {
    let text = serde_json::to_string(data).expect("JSON values always serialise");
    let mut quoted = String::with_capacity(text.len() + 2);
    for c in text.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '\'' => quoted.push_str("\\'"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\u{2028}' => quoted.push_str("\\u2028"),
            '\u{2029}' => quoted.push_str("\\u2029"),
            c => quoted.push(c),
        }
    }
    format!("export const DATA = JSON.parse('{quoted}');\n")
}

fn behavior_entity(body: &Body) -> Value {
    let mut description = json!({
        "identifier": format!("arnis:{}", body.name),
        "is_spawnable": false,
        "is_summonable": true,
    });
    if body.fleet == Fleet::Car {
        description["properties"] = json!({
            "arnis:brake": { "type": "bool", "default": false, "client_sync": true },
        });
    }
    json!({
        "format_version": ENTITY_FORMAT,
        "minecraft:entity": {
            "description": description,
            "components": {
                "minecraft:type_family": { "family": [body.fleet.family(), "inanimate"] },
                // Nothing to bump into or hit, like the Java displays.
                "minecraft:collision_box": { "width": 0.0001, "height": 0.0001 },
                "minecraft:custom_hit_test": {
                    "hitboxes": [{ "width": 0.0, "height": 0.0, "pivot": [0.0, 100.0, 0.0] }],
                },
                "minecraft:physics": { "has_gravity": false, "has_collision": false },
                "minecraft:pushable": { "is_pushable": false, "is_pushable_by_piston": false },
                "minecraft:damage_sensor": { "triggers": { "cause": "all", "deals_damage": "no" } },
                "minecraft:fire_immune": {},
                "minecraft:persistent": {},
                // The script turns a vehicle by teleporting it; the body has to turn with it.
                "minecraft:body_rotation_always_follows_head": {},
            },
        },
    })
}

fn client_entity(body: &Body) -> Value {
    let controller = match body.fleet {
        Fleet::Car => "controller.render.arnis.car",
        Fleet::Train => "controller.render.arnis.vehicle",
    };
    json!({
        "format_version": "1.10.0",
        "minecraft:client_entity": {
            "description": {
                "identifier": format!("arnis:{}", body.name),
                "materials": { "default": "entity_alphatest", "lit": "entity_emissive" },
                "textures": { "default": TEXTURE },
                "geometry": { "default": format!("geometry.arnis.{}", body.name) },
                "render_controllers": [controller],
            },
        },
    })
}

fn render_controllers() -> Value {
    let materials = json!([{ "*": "Material.default" }, { "lit*": "Material.lit" }]);
    json!({
        "format_version": "1.8.0",
        "render_controllers": {
            "controller.render.arnis.vehicle": {
                "geometry": "Geometry.default",
                "materials": materials,
                "textures": ["Texture.default"],
            },
            "controller.render.arnis.car": {
                "geometry": "Geometry.default",
                "materials": materials,
                "textures": ["Texture.default"],
                "part_visibility": [
                    { "brake_off": "!q.property('arnis:brake')" },
                    { "lit_brake": "q.property('arnis:brake')" },
                ],
            },
        },
    })
}

/// A random (version 4) UUID.
fn new_uuid() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|v| format!("{v:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(name: &str, fleet: Fleet) -> Body {
        Body {
            name: name.to_string(),
            fleet,
            shapes: vec![Shape {
                block: "minecraft:red_concrete",
                size: [1.0, 1.0, 1.0],
                corner: [-0.5, 0.0, -0.5],
                tilt: 0.0,
                lamp: Lamp::Brake,
            }],
        }
    }

    fn file<'a>(pack: &'a Pack, path: &str) -> &'a [u8] {
        &pack
            .files
            .iter()
            .find(|(p, _)| p == path)
            .unwrap_or_else(|| panic!("{path} missing"))
            .1
    }

    fn json(pack: &Pack, path: &str) -> Value {
        serde_json::from_slice(file(pack, path)).unwrap()
    }

    #[test]
    fn the_packs_name_each_other_and_every_body_is_an_entity_drawn_by_its_model() {
        let mut n = 0;
        let addon = build(
            &json!({ "cars": null, "trains": null }),
            &[body("car_x", Fleet::Car), body("train_y", Fleet::Train)],
            || {
                n += 1;
                format!("00000000-0000-4000-8000-{n:012}")
            },
        );
        let (bp, rp) = (&addon.behavior, &addon.resources);
        let bp_manifest = json(bp, "manifest.json");
        let rp_manifest = json(rp, "manifest.json");
        assert_eq!(bp_manifest["header"]["uuid"], bp.id.as_str());
        assert_eq!(rp_manifest["header"]["uuid"], rp.id.as_str());
        let deps = bp_manifest["dependencies"].as_array().unwrap();
        assert!(deps.iter().any(|d| d["uuid"] == rp.id.as_str()));
        assert!(deps
            .iter()
            .any(|d| d["module_name"] == "@minecraft/server" && d["version"] == SERVER_MODULE));
        let mut ids: Vec<String> = [&bp_manifest, &rp_manifest]
            .iter()
            .flat_map(|m| {
                let mut v = vec![m["header"]["uuid"].as_str().unwrap().to_string()];
                v.extend(
                    m["modules"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x["uuid"].as_str().unwrap().to_string()),
                );
                v
            })
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 5, "every pack and module has an id of its own");

        for (name, controller) in [
            ("car_x", "controller.render.arnis.car"),
            ("train_y", "controller.render.arnis.vehicle"),
        ] {
            let entity = json(bp, &format!("entities/{name}.json"));
            assert_eq!(
                entity["minecraft:entity"]["description"]["identifier"],
                format!("arnis:{name}")
            );
            let client = json(rp, &format!("entity/{name}.entity.json"));
            let desc = &client["minecraft:client_entity"]["description"];
            assert_eq!(desc["render_controllers"][0], controller);
            let geo = json(rp, &format!("models/entity/{name}.geo.json"));
            assert_eq!(
                geo["minecraft:geometry"][0]["description"]["identifier"],
                desc["geometry"]["default"]
            );
        }
        // Only cars have the brake light the car controller asks for.
        let car = json(bp, "entities/car_x.json");
        assert!(car["minecraft:entity"]["description"]["properties"]["arnis:brake"].is_object());
        let train = json(bp, "entities/train_y.json");
        assert!(train["minecraft:entity"]["description"]["properties"].is_null());

        let rc = json(
            rp,
            "render_controllers/arnis_vehicles.render_controllers.json",
        );
        assert!(
            rc["render_controllers"]["controller.render.arnis.car"]["part_visibility"].is_array()
        );
        let lang = String::from_utf8(file(rp, "texts/en_US.lang").to_vec()).unwrap();
        assert!(lang.contains("entity.arnis:car_x.name=Car"));
        for (path, _) in SCRIPTS {
            assert!(!file(bp, path).is_empty());
        }
        assert_eq!(
            json(bp, "manifest.json")["modules"][1]["entry"],
            "scripts/main.js"
        );
    }

    #[test]
    fn the_data_module_survives_quotes_and_line_breaks() {
        let data = json!({ "name": "O'Brien \\ \"x\"\n\u{2028}" });
        let text = data_module(&data);
        assert!(text.starts_with("export const DATA = JSON.parse('"));
        let inner = text
            .trim_start_matches("export const DATA = JSON.parse('")
            .trim_end_matches("');\n");
        // Undo the JS string escapes, then it must be the JSON again.
        let mut unescaped = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                assert_ne!(c, '\'', "an unescaped quote ends the string");
                unescaped.push(c);
                continue;
            }
            match chars.next().unwrap() {
                'n' => unescaped.push('\n'),
                'r' => unescaped.push('\r'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    unescaped.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
                }
                other => unescaped.push(other),
            }
        }
        assert_eq!(serde_json::from_str::<Value>(&unescaped).unwrap(), data);
        assert!(!inner.contains('\n') && !inner.contains('\u{2028}'));
    }

    #[test]
    fn uuids_are_version_4() {
        let id = new_uuid();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(id, new_uuid());
    }
}
