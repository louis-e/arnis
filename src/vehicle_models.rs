//! The models the moving vehicles are drawn with. `assets/vehicles` builds them (see its
//! README) into `assets/vehicles/out`: per model a texture in a preview livery, a mask
//! naming each texel's livery slot and shade, Bedrock geometry and Java item models. Here
//! each is coloured for a livery and handed to the Java resource pack or the Bedrock
//! add-on, and placed on the display entity that carries it.
//!
//! Models face -z with their right side at +x, the ground at y 0, in sixteenths of a
//! metre. Bedrock draws an entity's -z as its front. Java's item display renderer turns
//! the item half a circle about y before drawing it, so there too the model's -z ends up
//! ahead; a model that has to sit the other way round, as a cab at the back of a train,
//! gets a half turn of its own.

use fastnbt::Value as Nbt;
use image::{ImageFormat, Rgba, RgbaImage};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::sync::OnceLock;

/// The item a display carries. Its own model is never drawn, the `item_model` component
/// replaces it; a block item puts the model in the opaque cutout pass, as for the facade
/// panels (see `mapillary::displays`).
const CARRIER: &str = "minecraft:stone";
/// Namespace and folders of the models, textures and item definitions in the pack.
const NAMESPACE: &str = "arnis";
/// Texel flags in the mask's blue channel, as `kit.py` writes them.
const LIT: u8 = 1;

/// One model as built: its files, embedded.
pub struct Model {
    pub id: &'static str,
    png: &'static [u8],
    mask: &'static [u8],
    geo: &'static str,
    java: &'static str,
    brake: Option<&'static str>,
    /// A car's body and dim brake lamps, nothing glowing: how it stands parked.
    parked: Option<&'static str>,
    meta: &'static str,
}

macro_rules! model {
    ($id:literal) => {
        Model {
            id: $id,
            png: include_bytes!(concat!("../assets/vehicles/out/", $id, ".png")),
            mask: include_bytes!(concat!("../assets/vehicles/out/", $id, ".mask.png")),
            geo: include_str!(concat!("../assets/vehicles/out/", $id, ".geo.json")),
            java: include_str!(concat!("../assets/vehicles/out/", $id, ".java.json")),
            brake: None,
            parked: None,
            meta: include_str!(concat!("../assets/vehicles/out/", $id, ".meta.json")),
        }
    };
    ($id:literal, brake) => {
        Model {
            brake: Some(include_str!(concat!(
                "../assets/vehicles/out/",
                $id,
                "_brake.java.json"
            ))),
            parked: Some(include_str!(concat!(
                "../assets/vehicles/out/",
                $id,
                "_parked.java.json"
            ))),
            ..model!($id)
        }
    };
}

/// Every model, as `assets/vehicles/build.py` writes them. Cars have brake lights.
static MODELS: &[Model] = &[
    model!("car_hatchback", brake),
    model!("car_sedan", brake),
    model!("car_estate", brake),
    model!("car_suv", brake),
    model!("car_city", brake),
    model!("car_van", brake),
    model!("car_pickup", brake),
    model!("car_truck", brake),
    model!("car_bus", brake),
    model!("train_tram_cab"),
    model!("train_tram_mid"),
    model!("train_tram_short"),
    model!("train_metro_cab"),
    model!("train_metro_mid"),
    model!("train_sbahn_cab"),
    model!("train_sbahn_cab_third_rail"),
    model!("train_sbahn_mid"),
    model!("train_regio_cab"),
    model!("train_regio_mid"),
    model!("train_deck_cab"),
    model!("train_deck_mid"),
    model!("train_ice_cab"),
    model!("train_ice_mid"),
    model!("train_loco"),
    model!("freight_container_40"),
    model!("freight_container_2x20"),
    model!("freight_flat"),
    model!("freight_tank"),
    model!("freight_boxcar"),
    model!("ship_motorboat"),
    model!("ship_sailboat"),
    model!("ship_river_ferry"),
    model!("ship_water_bus"),
    model!("ship_tour_boat"),
    model!("ship_tour_boat_low"),
    model!("ship_peniche"),
    model!("ship_peniche_low"),
    model!("ship_car_ferry"),
    model!("ship_sea_ferry"),
    model!("ship_barge"),
    model!("ship_barge_low"),
    model!("ship_container_barge"),
    model!("ship_container_barge_low"),
    model!("ship_tank_barge"),
    model!("ship_tank_barge_low"),
    model!("ship_river_cruise"),
    model!("ship_river_cruise_low"),
    // Traffic signal heads, a model per state; drawn at 32 units to the metre.
    model!("signal_car_red"),
    model!("signal_car_amber"),
    model!("signal_car_green"),
    model!("signal_car_red_amber"),
    model!("signal_walk_red"),
    model!("signal_walk_green"),
];

/// The model of that id. The ids are fixed in the code that picks them, and a test
/// checks every one of those is here.
pub fn model(id: &str) -> &'static Model {
    MODELS
        .iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("no vehicle model {id}"))
}

#[derive(Deserialize)]
struct JavaFrame {
    centre: [f64; 3],
    scale: f64,
}

#[derive(Deserialize)]
pub struct Meta {
    java: JavaFrame,
    /// Pivots of the moving bones, in units.
    pub bones: BTreeMap<String, [f64; 3]>,
    /// Lowest and highest corner, in units.
    pub bounds: [[f64; 3]; 2],
    pub texture: [u32; 2],
}

fn metas() -> &'static HashMap<&'static str, Meta> {
    static METAS: OnceLock<HashMap<&'static str, Meta>> = OnceLock::new();
    METAS.get_or_init(|| {
        MODELS
            .iter()
            .map(|m| {
                let meta: Meta =
                    serde_json::from_str(m.meta).unwrap_or_else(|e| panic!("{} meta: {e}", m.id));
                (m.id, meta)
            })
            .collect()
    })
}

/// The paint each livery block stands for, from `assets/vehicles/paints.json`.
fn paint(block: &str) -> [u8; 3] {
    static PAINTS: OnceLock<HashMap<String, [u8; 3]>> = OnceLock::new();
    let paints = PAINTS.get_or_init(|| {
        let raw: HashMap<String, Value> =
            serde_json::from_str(include_str!("../assets/vehicles/paints.json"))
                .expect("paints.json is valid");
        raw.into_iter()
            .filter_map(|(k, v)| {
                let a = v.as_array()?;
                Some((
                    k,
                    [0, 1, 2].map(|i| a.get(i).and_then(Value::as_u64).unwrap_or(0) as u8),
                ))
            })
            .collect()
    });
    *paints
        .get(block)
        .unwrap_or_else(|| panic!("no paint for {block}"))
}

/// Towards white for k > 0, towards black for k < 0, as `kit.shade`.
fn shade(c: [u8; 3], k: f64) -> [u8; 3] {
    c.map(|v| {
        let v = f64::from(v);
        let out = if k >= 0.0 {
            v + (255.0 - v) * k
        } else {
            v * (1.0 + k)
        };
        out.round().clamp(0.0, 255.0) as u8
    })
}

/// What a vehicle body looks like: its model, the colours of its livery and which way
/// round the model sits.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Look {
    pub model: &'static str,
    /// Names this colouring of the model, unique among the model's colourings.
    pub livery: String,
    /// The livery's blocks by slot: body, lower, stripe, door, roof.
    pub colours: [&'static str; 5],
    /// The model's front is the vehicle's back, as a cab at the rear of a train.
    pub backward: bool,
}

impl Look {
    /// A look in one colour throughout.
    pub fn plain(model: &'static str, livery: impl Into<String>, colour: &'static str) -> Self {
        Look {
            model,
            livery: livery.into(),
            colours: [colour; 5],
            backward: false,
        }
    }

    /// The model coloured: the name its texture and Java item model go by.
    pub fn name(&self) -> String {
        format!("{}_{}", self.model, self.livery)
    }

    pub fn meta(&self) -> &'static Meta {
        &metas()[self.model]
    }
}

/// A model's texture and mask, decoded.
pub struct Decoded {
    img: RgbaImage,
    mask: RgbaImage,
}

impl Decoded {
    /// The texture for `colours`, as `Model::texture`.
    pub fn texture(&self, colours: &[&str; 5], bedrock: bool) -> Vec<u8> {
        let mut img = self.img.clone();
        recolour(&mut img, &self.mask, colours, bedrock);
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png)
            .expect("encoding a PNG in memory cannot fail");
        out.into_inner()
    }
}

impl Model {
    /// The texture for `colours`. Bedrock's has the lamps transparent, which its
    /// emissive material reads as fully lit; Java's is opaque throughout.
    #[cfg(test)]
    pub fn texture(&self, colours: &[&str; 5], bedrock: bool) -> Vec<u8> {
        self.decode().texture(colours, bedrock)
    }

    /// The texture and its mask, decoded once to colour for several looks.
    pub fn decode(&self) -> Decoded {
        Decoded {
            img: image::load_from_memory(self.png)
                .expect("vehicle textures are valid PNGs")
                .to_rgba8(),
            mask: image::load_from_memory(self.mask)
                .expect("vehicle masks are valid PNGs")
                .to_rgba8(),
        }
    }

    pub fn meta(&self) -> &'static Meta {
        &metas()[self.id]
    }

    /// Whether it has brake lights, drawn by a display of their own on Java.
    #[cfg(test)]
    pub fn has_brake(&self) -> bool {
        self.brake.is_some()
    }

    /// Height of the model's highest point in metres at 1:1.
    pub fn top_m(&self) -> f64 {
        self.meta().bounds[1][1] / 16.0
    }

    /// The wheel bones Bedrock turns, with each wheel's radius in units: its pivot's
    /// height, since a road wheel stands on the ground.
    pub fn wheels(&self) -> Vec<(&'static str, f64)> {
        self.meta()
            .bones
            .iter()
            .filter(|(name, _)| name.starts_with("wheel"))
            .map(|(name, p)| (name.as_str(), p[1]))
            .collect()
    }

    /// Bedrock geometry named `identifier` at `size` blocks per metre, turned half a
    /// circle when `backward`.
    pub fn bedrock_geometry(&self, identifier: &str, size: f64, backward: bool) -> Value {
        let mut geo: Value = serde_json::from_str(self.geo).expect("vehicle geometry is JSON");
        let g = &mut geo["minecraft:geometry"][0];
        g["description"]["identifier"] = json!(identifier);
        for key in ["visible_bounds_width", "visible_bounds_height"] {
            let v = g["description"][key].as_f64().unwrap_or(1.0);
            g["description"][key] = json!((v * size).ceil());
        }
        scale_list(&mut g["description"]["visible_bounds_offset"], size);
        if let Some(bones) = g["bones"].as_array_mut() {
            for bone in bones.iter_mut() {
                scale_list(&mut bone["pivot"], size);
                if let Some(cubes) = bone["cubes"].as_array_mut() {
                    for cube in cubes {
                        for key in ["origin", "size", "pivot"] {
                            scale_list(&mut cube[key], size);
                        }
                    }
                }
                if backward {
                    bone["parent"] = json!("turn");
                }
            }
            if backward {
                bones.insert(
                    0,
                    json!({ "name": "turn", "pivot": [0, 0, 0], "rotation": [0, 180, 0] }),
                );
            }
        }
        geo
    }
}

fn scale_list(v: &mut Value, k: f64) {
    if let Some(list) = v.as_array_mut() {
        for x in list {
            if let Some(f) = x.as_f64() {
                *x = json!((f * k * 1000.0).round() / 1000.0);
            }
        }
    }
}

/// Colours a texture in place: livery texels take their slot's paint, shaded as the mask
/// says, and on Bedrock lit texels turn transparent.
fn recolour(img: &mut RgbaImage, mask: &RgbaImage, colours: &[&str; 5], bedrock: bool) {
    // Every shade of every slot's paint, by the mask's green.
    let shades: Vec<[[u8; 3]; 256]> = colours
        .iter()
        .map(|c| {
            let base = paint(c);
            std::array::from_fn(|k| shade(base, k as f64 / 127.5 - 1.0))
        })
        .collect();
    for (p, m) in img.pixels_mut().zip(mask.pixels()) {
        let [slot, k, flags, used] = m.0;
        if used == 0 {
            *p = Rgba([0, 0, 0, 0]);
            continue;
        }
        if (1..=5).contains(&slot) {
            let c = shades[slot as usize - 1][k as usize];
            *p = Rgba([c[0], c[1], c[2], 255]);
        } else {
            p.0[3] = 255;
        }
        if bedrock && flags & LIT != 0 {
            p.0[3] = 0;
        }
    }
}

/// The Java item model id a look is drawn by, and the one of its model's brake lights.
pub fn item_model(look: &Look) -> String {
    format!("{NAMESPACE}:vehicle/{}", look.name())
}

/// The item model id of `model` in the livery named `livery`, which may be a macro
/// variable the pack fills in.
pub fn item_model_named(model: &str, livery: &str) -> String {
    format!("{NAMESPACE}:vehicle/{model}_{livery}")
}

pub fn brake_item_model(model: &str) -> String {
    format!("{NAMESPACE}:vehicle/{model}_brake")
}

/// The item model id of a look parked: its headlights off.
pub fn parked_item_model(look: &Look) -> String {
    format!("{NAMESPACE}:vehicle/{}_parked", look.name())
}

/// How the display has to place the model so that a point of the vehicle at 1:1 lands
/// `size` times as far out from the entity: (translation, left_rotation, scale).
///
/// The game draws model unit u at (u - 8) / 16 blocks, turned half a circle about y, and
/// the model is the vehicle fitted into Java's -16..32: u = (v - centre) * fit + 8. So the
/// scale is size / fit, and the translation puts the vehicle's own origin back on the
/// entity: the centre, turned the way the model is.
pub fn display_transform(model: &str, size: f64, backward: bool) -> ([f64; 3], [f64; 4], f64) {
    let frame = &metas()[model].java;
    let c = frame.centre.map(|v| v * size / 16.0);
    if backward {
        // The half turn here undoes the renderer's: the model's front faces the back.
        (c, [0.0, 1.0, 0.0, 0.0], size / frame.scale)
    } else {
        (
            [-c[0], c[1], -c[2]],
            [0.0, 0.0, 0.0, 1.0],
            size / frame.scale,
        )
    }
}

/// The culling box of a display drawing `model` at `size`, in blocks: a square round the
/// entity wide enough for the vehicle at any heading, and as tall as it. The game skips a
/// display whose box is off screen; a zero box would be drawn wherever it is.
pub fn culling_box(model: &str, size: f64) -> (f64, f64) {
    let [lo, hi] = metas()[model].bounds;
    let reach = [lo[0].abs(), hi[0].abs()]
        .iter()
        .flat_map(|x| [lo[2].abs(), hi[2].abs()].map(|z| x.hypot(z)))
        .fold(0.0, f64::max);
    (
        (2.0 * reach / 16.0 + 0.5) * size,
        (hi[1] / 16.0 + 0.5) * size,
    )
}

/// The SNBT of a display drawing `item_model` (which may hold macro variables) for
/// `model`, for a `summon` command; `extra` goes in as it is.
pub fn display_snbt(
    model: &str,
    item_model: &str,
    size: f64,
    backward: bool,
    extra: &str,
) -> String {
    let (t, q, s) = display_transform(model, size, backward);
    let (w, h) = culling_box(model, size);
    format!(
        "item:{{id:\"{CARRIER}\",count:1,components:{{\"minecraft:item_model\":\"{item_model}\"}}}},\
         transformation:{{left_rotation:[{:.4}f,{:.4}f,{:.4}f,{:.4}f],right_rotation:[0f,0f,0f,1f],\
         translation:[{:.4}f,{:.4}f,{:.4}f],scale:[{s:.4}f,{s:.4}f,{s:.4}f]}},width:{w:.2}f,height:{h:.2}f{extra}",
        q[0], q[1], q[2], q[3], t[0], t[1], t[2],
    )
}

/// The same display as entity NBT, for one placed in the world.
pub fn display_nbt(
    model: &str,
    item_model: &str,
    size: f64,
    backward: bool,
) -> HashMap<String, Nbt> {
    let (t, q, s) = display_transform(model, size, backward);
    let floats = |v: &[f64]| Nbt::List(v.iter().map(|f| Nbt::Float(*f as f32)).collect());
    let components = HashMap::from([(
        "minecraft:item_model".to_string(),
        Nbt::String(item_model.to_string()),
    )]);
    let item = HashMap::from([
        ("id".to_string(), Nbt::String(CARRIER.to_string())),
        ("count".to_string(), Nbt::Int(1)),
        ("components".to_string(), Nbt::Compound(components)),
    ]);
    let (w, h) = culling_box(model, size);
    HashMap::from([
        ("item".to_string(), Nbt::Compound(item)),
        ("width".to_string(), Nbt::Float(w as f32)),
        ("height".to_string(), Nbt::Float(h as f32)),
        (
            "transformation".to_string(),
            Nbt::Compound(HashMap::from([
                ("left_rotation".to_string(), floats(&q)),
                ("right_rotation".to_string(), floats(&[0.0, 0.0, 0.0, 1.0])),
                ("translation".to_string(), floats(&t)),
                ("scale".to_string(), floats(&[s, s, s])),
            ])),
        ),
    ])
}

/// The resource pack files that draw `looks` on Java: per model its geometry and, for
/// cars, its brake lights; per look a texture, a model naming it and an item definition.
pub fn java_pack<'a>(
    looks: impl IntoIterator<Item = &'a Look>,
    parked: &'a [Look],
) -> Vec<(String, Vec<u8>)> {
    let mut by_model: BTreeMap<&str, Vec<&Look>> = BTreeMap::new();
    for look in looks.into_iter().chain(parked) {
        let list = by_model.entry(look.model).or_default();
        if !list.iter().any(|l| l.livery == look.livery) {
            list.push(look);
        }
    }
    let parks = |look: &Look| {
        parked
            .iter()
            .any(|p| p.model == look.model && p.livery == look.livery)
    };
    let pretty = |v: &Value| serde_json::to_vec(v).expect("JSON values always serialise");
    let mut out = Vec::new();
    for (id, looks) in by_model {
        let m = model(id);
        // The model's own file names the first look's texture, so it is whole by itself.
        let first = format!("{NAMESPACE}:block/vehicle/{}", looks[0].name());
        for (suffix, text) in [("", Some(m.java)), ("_brake", m.brake)] {
            let Some(text) = text else { continue };
            let mut v: Value = serde_json::from_str(text).expect("vehicle models are JSON");
            v["textures"] = json!({ "t": first, "particle": first });
            out.push((
                format!("assets/{NAMESPACE}/models/item/vehicle/{id}{suffix}.json"),
                pretty(&v),
            ));
        }
        if m.brake.is_some() {
            out.push((
                format!("assets/{NAMESPACE}/items/vehicle/{id}_brake.json"),
                pretty(&item_definition(&format!(
                    "{NAMESPACE}:item/vehicle/{id}_brake"
                ))),
            ));
        }
        // A parked car keeps its lamps dark, its brake lamps on its body.
        if looks.iter().any(|l| parks(l)) {
            let text = m.parked.expect("every car that parks has a parked model");
            let mut v: Value = serde_json::from_str(text).expect("vehicle models are JSON");
            v["textures"] = json!({ "t": first, "particle": first });
            out.push((
                format!("assets/{NAMESPACE}/models/item/vehicle/{id}_parked.json"),
                pretty(&v),
            ));
        }
        let decoded = m.decode();
        for look in looks {
            let name = look.name();
            let texture = format!("{NAMESPACE}:block/vehicle/{name}");
            out.push((
                format!("assets/{NAMESPACE}/textures/block/vehicle/{name}.png"),
                decoded.texture(&look.colours, false),
            ));
            out.push((
                format!("assets/{NAMESPACE}/models/item/vehicle/{name}.json"),
                pretty(&json!({
                    "parent": format!("{NAMESPACE}:item/vehicle/{id}"),
                    "textures": { "t": texture, "particle": texture },
                })),
            ));
            out.push((
                format!("assets/{NAMESPACE}/items/vehicle/{name}.json"),
                pretty(&item_definition(&format!(
                    "{NAMESPACE}:item/vehicle/{name}"
                ))),
            ));
            if parks(look) {
                out.push((
                    format!("assets/{NAMESPACE}/models/item/vehicle/{name}_parked.json"),
                    pretty(&json!({
                        "parent": format!("{NAMESPACE}:item/vehicle/{id}_parked"),
                        "textures": { "t": texture, "particle": texture },
                    })),
                ));
                out.push((
                    format!("assets/{NAMESPACE}/items/vehicle/{name}_parked.json"),
                    pretty(&item_definition(&format!(
                        "{NAMESPACE}:item/vehicle/{name}_parked"
                    ))),
                ));
            }
        }
    }
    out
}

fn item_definition(model: &str) -> Value {
    json!({ "model": { "type": "minecraft:model", "model": model } })
}

/// Block atlas pixels the textures of `looks` take, one per distinct look.
pub fn atlas_px<'a>(looks: impl IntoIterator<Item = &'a Look>) -> u64 {
    let mut seen = std::collections::HashSet::new();
    looks
        .into_iter()
        .filter(|l| seen.insert((l.model, l.livery.clone())))
        .map(|l| {
            let [w, h] = l.meta().texture;
            u64::from(w) * u64::from(h)
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_quat_turn(q: [f64; 4], p: [f64; 3]) -> [f64; 3] {
        // Rotation of p by the unit quaternion q = [x, y, z, w].
        let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
        let m = [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - z * w),
                2.0 * (x * z + y * w),
            ],
            [
                2.0 * (x * y + z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - x * w),
            ],
            [
                2.0 * (x * z - y * w),
                2.0 * (y * z + x * w),
                1.0 - 2.0 * (x * x + y * y),
            ],
        ];
        [0, 1, 2].map(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2])
    }

    /// Where the game draws a vehicle point `v` (units, model frame), relative to the
    /// display entity: translation + left_rotation * scale * (half turn of the model unit).
    fn drawn(model: &str, v: [f64; 3], size: f64, backward: bool) -> [f64; 3] {
        let frame = &metas()[model].java;
        let u = [0, 1, 2].map(|i| (v[i] - frame.centre[i]) * frame.scale + 8.0);
        let local = [0, 1, 2].map(|i| (u[i] - 8.0) / 16.0);
        let turned = [-local[0], local[1], -local[2]];
        let (t, q, s) = display_transform(model, size, backward);
        let r = unit_quat_turn(q, turned.map(|c| c * s));
        [0, 1, 2].map(|i| t[i] + r[i])
    }

    #[test]
    fn a_display_puts_the_front_ahead_and_the_right_side_right() {
        // The entity's own frame: +z ahead, and its right side at -x (Minecraft's east is
        // +x, and facing south, +z, east is on the left).
        let size = 1.5;
        let nose = drawn("car_hatchback", [0.0, 8.0, -32.0], size, false);
        assert!((nose[2] - 2.0 * size).abs() < 1e-6, "{nose:?}");
        assert!((nose[1] - 0.5 * size).abs() < 1e-6, "{nose:?}");
        let right = drawn("car_hatchback", [16.0, 0.0, 0.0], size, false);
        assert!((right[0] + size).abs() < 1e-6, "{right:?}");
        // Backward, the model's front lands behind the entity.
        let nose = drawn("car_hatchback", [0.0, 8.0, -32.0], size, true);
        assert!((nose[2] + 2.0 * size).abs() < 1e-6, "{nose:?}");
    }

    #[test]
    fn a_livery_colours_its_slots_and_bedrock_lamps_turn_transparent() {
        let m = model("car_hatchback");
        let mask = image::load_from_memory(m.mask).unwrap().to_rgba8();
        let red = ["minecraft:red_concrete"; 5];
        let blue = ["minecraft:blue_concrete"; 5];
        let a = image::load_from_memory(&m.texture(&red, false))
            .unwrap()
            .to_rgba8();
        let b = image::load_from_memory(&m.texture(&blue, true))
            .unwrap()
            .to_rgba8();
        let (mut painted, mut lit) = (0, 0);
        for ((pa, pb), pm) in a.pixels().zip(b.pixels()).zip(mask.pixels()) {
            if pm.0[3] == 0 {
                continue;
            }
            if pm.0[0] == 1 && pm.0[1] == 128 {
                painted += 1;
                assert_eq!(&pa.0[..3], &shade(paint(red[0]), 128.0 / 127.5 - 1.0));
                assert_eq!(&pb.0[..3], &shade(paint(blue[0]), 128.0 / 127.5 - 1.0));
            }
            assert_eq!(pa.0[3], 255, "Java's texture is opaque");
            if pm.0[2] & LIT != 0 {
                lit += 1;
                assert_eq!(pb.0[3], 0);
            }
        }
        assert!(painted > 100 && lit > 10, "{painted} painted, {lit} lit");
    }

    #[test]
    fn geometry_scales_and_a_backward_model_turns_round() {
        let m = model("car_hatchback");
        let one = m.bedrock_geometry("geometry.t", 1.0, false);
        let two = m.bedrock_geometry("geometry.t", 2.0, true);
        let cube = |g: &Value| {
            g["minecraft:geometry"][0]["bones"]
                .as_array()
                .unwrap()
                .iter()
                .find(|b| b["name"] == "body")
                .unwrap()["cubes"][0]
                .clone()
        };
        let (a, b) = (cube(&one), cube(&two));
        for k in 0..3 {
            let (x, y) = (
                a["size"][k].as_f64().unwrap(),
                b["size"][k].as_f64().unwrap(),
            );
            assert!((y - 2.0 * x).abs() < 1e-3);
        }
        let bones = two["minecraft:geometry"][0]["bones"].as_array().unwrap();
        assert_eq!(bones[0]["name"], "turn");
        assert!(bones[1..].iter().all(|b| b["parent"] == "turn"));
        assert_eq!(
            one["minecraft:geometry"][0]["description"]["identifier"],
            "geometry.t"
        );
    }

    /// A parked car is the same model with its lamps dark and its brake lamps on it,
    /// in a look of its own that shares the moving car's texture.
    #[test]
    fn a_parked_look_is_its_model_with_no_lamp_lit() {
        let moving = [Look::plain("car_sedan", "red", "minecraft:red_concrete")];
        let parked = [
            Look::plain("car_sedan", "red", "minecraft:red_concrete"),
            Look::plain("car_sedan", "blue", "minecraft:blue_concrete"),
        ];
        let files = java_pack(&moving, &parked);
        let file = |p: &str| -> Value {
            let (_, bytes) = files
                .iter()
                .find(|(f, _)| f == p)
                .unwrap_or_else(|| panic!("{p}"));
            serde_json::from_slice(bytes).unwrap()
        };
        let lit = |v: &Value| {
            v["elements"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e.get("light_emission").is_some())
                .count()
        };
        let base = file("assets/arnis/models/item/vehicle/car_sedan.json");
        let dark = file("assets/arnis/models/item/vehicle/car_sedan_parked.json");
        let brake = file("assets/arnis/models/item/vehicle/car_sedan_brake.json");
        let count = |v: &Value| v["elements"].as_array().unwrap().len();
        assert!(lit(&base) > 0);
        assert_eq!(lit(&dark), 0);
        // Its body and its brake lamps, which a moving car draws as a display of their own.
        assert_eq!(count(&dark), count(&base) + count(&brake));
        for name in ["car_sedan_red", "car_sedan_blue"] {
            let item = file(&format!("assets/arnis/items/vehicle/{name}_parked.json"));
            assert_eq!(
                item["model"]["model"],
                format!("arnis:item/vehicle/{name}_parked")
            );
            let child = file(&format!(
                "assets/arnis/models/item/vehicle/{name}_parked.json"
            ));
            assert_eq!(child["parent"], "arnis:item/vehicle/car_sedan_parked");
            assert_eq!(
                child["textures"]["t"],
                format!("arnis:block/vehicle/{name}")
            );
            let png = format!("assets/arnis/textures/block/vehicle/{name}.png");
            assert!(files.iter().any(|(f, _)| *f == png), "{png}");
        }
        assert_eq!(
            parked_item_model(&parked[1]),
            "arnis:vehicle/car_sedan_blue_parked"
        );
    }

    #[test]
    fn the_java_pack_has_a_texture_model_and_item_for_every_look_and_one_brake() {
        let looks = [
            Look::plain("car_hatchback", "red", "minecraft:red_concrete"),
            Look::plain("car_hatchback", "blue", "minecraft:blue_concrete"),
        ];
        let files = java_pack(&looks, &[]);
        let has = |p: &str| files.iter().any(|(f, _)| f == p);
        assert!(!files.iter().any(|(f, _)| f.contains("_parked")));
        for name in ["car_hatchback_red", "car_hatchback_blue"] {
            assert!(has(&format!(
                "assets/arnis/textures/block/vehicle/{name}.png"
            )));
            assert!(has(&format!(
                "assets/arnis/models/item/vehicle/{name}.json"
            )));
            assert!(has(&format!("assets/arnis/items/vehicle/{name}.json")));
        }
        assert!(has("assets/arnis/models/item/vehicle/car_hatchback.json"));
        assert!(has(
            "assets/arnis/models/item/vehicle/car_hatchback_brake.json"
        ));
        assert!(has("assets/arnis/items/vehicle/car_hatchback_brake.json"));
        let (_, base) = files
            .iter()
            .find(|(f, _)| f.ends_with("vehicle/car_hatchback.json") && f.contains("models"))
            .unwrap();
        let base: Value = serde_json::from_slice(base).unwrap();
        for el in base["elements"].as_array().unwrap() {
            for k in 0..3 {
                for key in ["from", "to"] {
                    let v = el[key][k].as_f64().unwrap();
                    assert!(
                        (-16.0..=32.0).contains(&v),
                        "{v} outside Java's model space"
                    );
                }
            }
            if let Some(r) = el.get("rotation") {
                assert!(r["angle"].as_f64().unwrap().abs() <= 45.0);
            }
        }
        let [w, h] = looks[0].meta().texture;
        assert_eq!(atlas_px(&looks), 2 * u64::from(w) * u64::from(h));
    }
}
