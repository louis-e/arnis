//! Optional gameplay extras, shipped as one Java data pack inside the world.
//!
//! The base layer targets 1.21.4, the oldest version an Arnis world opens in. Later versions
//! renamed or added what some features need, so overlays carry those files per pack format:
//! 71 (1.21.5) the advancement background id, 80 (1.21.6) dialogs and the outbreak, 101
//! (26.1) the world clock, 107 (26.2) namespaced entity predicates and 121 (26.3) typed
//! conditions. A tag that names a function which failed to load is dropped whole, so every
//! function an overlay replaces also exists, as a stub if need be, in the base layer.

mod dialogs;
pub mod discoveries;
mod resources;

use crate::args::Args;
use crate::celestial::CelestialBody;
use discoveries::{CriteriaSchema, Place};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

pub const PACK_NAME: &str = "arnis_extras";

const BASE_FORMAT: u32 = 61;
const MAX_FORMAT: u32 = i32::MAX as u32;
/// Pack format each overlay starts at; later entries win where they overlap.
const OVERLAYS: [(u32, &str); 5] = [
    (71, "ov71"),
    (80, "ov80"),
    (101, "ov101"),
    (107, "ov107"),
    (121, "ov121"),
];
const MAX_DESTINATIONS: usize = 20;
/// Above any build height, so the search starts at the top.
const TOP_SEARCH_Y: i32 = 4096;

macro_rules! static_files {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_bytes!(concat!("../../assets/minecraft/datapack_extras/", $path)) as &[u8])),*]
    };
}

const CORE: &[(&str, &[u8])] = static_files![
    "data/arnis/function/load.mcfunction",
    "data/arnis/function/util/daytime.mcfunction",
    "data/arnis/function/util/ground.mcfunction",
    "data/arnis/function/util/ground_step.mcfunction",
    "ov101/data/arnis/function/util/daytime.mcfunction",
];

const GRAVITY: &[(&str, &[u8])] = static_files![
    "data/arnis/function/gravity/load.mcfunction",
    "data/arnis/function/gravity/tick.mcfunction",
    "data/arnis/function/gravity/sweep.mcfunction",
];

const TRAFFIC: &[(&str, &[u8])] = static_files![
    "data/arnis/function/traffic/load.mcfunction",
    "data/arnis/function/traffic/tick.mcfunction",
    "data/arnis/function/traffic/on.mcfunction",
    "data/arnis/function/traffic/off.mcfunction",
    "data/arnis/function/traffic/catch_up.mcfunction",
    "data/arnis/function/traffic/switched.mcfunction",
    "data/arnis/function/traffic/phase/a_go.mcfunction",
    "data/arnis/function/traffic/phase/a_amber.mcfunction",
    "data/arnis/function/traffic/phase/a_stop.mcfunction",
    "data/arnis/function/traffic/phase/a_ready.mcfunction",
    "data/arnis/function/traffic/phase/a_walk_end.mcfunction",
    "data/arnis/function/traffic/phase/b_go.mcfunction",
    "data/arnis/function/traffic/phase/b_amber.mcfunction",
    "data/arnis/function/traffic/phase/b_stop.mcfunction",
    "data/arnis/function/traffic/phase/b_ready.mcfunction",
    "data/arnis/function/traffic/phase/b_walk_end.mcfunction",
];

const MENU: &[(&str, &[u8])] = static_files![
    "data/arnis/function/menu/load.mcfunction",
    "data/arnis/function/menu/tick.mcfunction",
    "data/arnis/function/menu/second.mcfunction",
    "data/arnis/function/menu/travel.mcfunction",
    "data/arnis/function/menu/go.mcfunction",
];

const CARS: &[(&str, &[u8])] = static_files![
    "data/arnis/function/cars/load.mcfunction",
    "data/arnis/function/cars/tick.mcfunction",
    "data/arnis/function/cars/drive.mcfunction",
    "data/arnis/function/cars/step.mcfunction",
    "data/arnis/function/cars/claim.mcfunction",
    "data/arnis/function/cars/move.mcfunction",
    "data/arnis/function/cars/arrive.mcfunction",
    "data/arnis/function/cars/target.mcfunction",
    "data/arnis/function/cars/turn.mcfunction",
    "data/arnis/function/cars/pick.mcfunction",
    "data/arnis/function/cars/despawn.mcfunction",
    "data/arnis/function/cars/cap.mcfunction",
    "data/arnis/function/cars/light.mcfunction",
    "data/arnis/function/cars/brake_lights.mcfunction",
    "data/arnis/function/cars/brake_switch.mcfunction",
    "data/arnis/function/cars/enter.mcfunction",
    "data/arnis/function/cars/respawn.mcfunction",
    "data/arnis/function/cars/respawn_try.mcfunction",
    "data/arnis/function/cars/respawn_lane.mcfunction",
    "data/arnis/function/cars/respawn_at.mcfunction",
    "data/arnis/function/cars/respawn_colour.mcfunction",
    "data/arnis/function/cars/respawn_body.mcfunction",
    "data/arnis/function/cars/respawn_settle.mcfunction",
];

const OUTBREAK: &[(&str, &[u8])] = static_files![
    "data/arnis/function/outbreak/load.mcfunction",
    "data/arnis/function/outbreak/tick.mcfunction",
    "data/arnis/function/outbreak/unsupported.mcfunction",
    "ov80/data/arnis/function/outbreak/load.mcfunction",
    "ov80/data/arnis/function/outbreak/tick.mcfunction",
    "ov80/data/arnis/function/outbreak/second.mcfunction",
    "ov80/data/arnis/function/outbreak/running.mcfunction",
    "ov80/data/arnis/function/outbreak/start.mcfunction",
    "ov80/data/arnis/function/outbreak/stop.mcfunction",
    "ov80/data/arnis/function/outbreak/clear.mcfunction",
    "ov80/data/arnis/function/outbreak/control.mcfunction",
    "ov80/data/arnis/function/outbreak/intro.mcfunction",
    "ov80/data/arnis/function/outbreak/night_start.mcfunction",
    "ov80/data/arnis/function/outbreak/dawn.mcfunction",
    "ov80/data/arnis/function/outbreak/dawn_check.mcfunction",
    "ov80/data/arnis/function/outbreak/no_sleep.mcfunction",
    "ov80/data/arnis/function/outbreak/time_set.mcfunction",
    "ov80/data/arnis/function/outbreak/sweep.mcfunction",
    "ov80/data/arnis/function/outbreak/infect.mcfunction",
    "ov80/data/arnis/function/outbreak/infect_at.mcfunction",
    "ov80/data/arnis/function/outbreak/victory.mcfunction",
    "ov80/data/arnis/function/outbreak/fireworks.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/budget.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/budget_set.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/budget_endless.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/second.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/spawn_batch.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/spawn_one.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/spot.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/pick.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/pick_2.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/pick_3.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/pick_4.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/pick_5.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/summon.mcfunction",
    "ov80/data/arnis/function/outbreak/wave/cleared.mcfunction",
    "ov80/data/arnis/function/outbreak/mob/setup.mcfunction",
    "ov80/data/arnis/function/outbreak/mob/brute.mcfunction",
    "ov80/data/arnis/function/outbreak/mob/turned.mcfunction",
    "ov80/data/arnis/function/outbreak/drop/spawn.mcfunction",
    "ov80/data/arnis/function/outbreak/drop/place.mcfunction",
    "ov80/data/arnis/function/outbreak/drop/land.mcfunction",
    "ov80/data/arnis/function/outbreak/drop/beacon.mcfunction",
    "ov80/data/arnis/function/outbreak/drop/remove.mcfunction",
    "ov80/data/arnis/function/outbreak/boss/spawn.mcfunction",
    "ov80/data/arnis/function/outbreak/boss/place.mcfunction",
    "ov80/data/arnis/function/outbreak/boss/spot.mcfunction",
    "ov80/data/arnis/function/outbreak/boss/rise.mcfunction",
    "ov80/data/arnis/function/outbreak/boss/setup.mcfunction",
    "ov80/data/arnis/function/outbreak/boss/second.mcfunction",
    "ov80/data/arnis/function/outbreak/boss/minions.mcfunction",
    "ov80/data/arnis/function/outbreak/boss/heart.mcfunction",
    "ov80/data/arnis/function/outbreak/hud/update.mcfunction",
    "ov80/data/arnis/function/outbreak/hud/day.mcfunction",
    "ov80/data/arnis/function/outbreak/hud/night.mcfunction",
    "ov80/data/arnis/loot_table/outbreak/infected_head.json",
    "ov80/data/arnis/loot_table/outbreak/supply_drop.json",
    "ov121/data/arnis/loot_table/outbreak/infected_head.json",
    "ov121/data/arnis/loot_table/outbreak/supply_drop.json",
];

/// Which extras a world gets. All of them are opt-in.
#[derive(Clone, Copy, Debug, Default)]
pub struct Extras {
    pub gravity: bool,
    pub traffic_lights: bool,
    pub discoveries: bool,
    pub pause_menu: bool,
    pub outbreak: bool,
    pub cars: bool,
}

impl Extras {
    pub fn from_args(args: &Args) -> Self {
        // Each One World area would write its pack over the last one's.
        if args.one_world {
            return Self::default();
        }
        Self {
            gravity: args.body_gravity && gravity_ratio(args.body).is_some(),
            traffic_lights: args.traffic_lights,
            discoveries: args.discoveries,
            pause_menu: args.pause_menu,
            outbreak: args.zombie_outbreak,
            cars: args.moving_cars,
        }
    }

    pub fn any(&self) -> bool {
        self.gravity
            || self.traffic_lights
            || self.discoveries
            || self.pause_menu
            || self.outbreak
            || self.cars
    }

    /// Whether the area's name and places are needed at all.
    pub fn wants_places(&self) -> bool {
        self.discoveries || self.pause_menu || self.outbreak
    }
}

/// Surface gravity relative to Earth's.
fn gravity_ratio(body: CelestialBody) -> Option<f64> {
    match body {
        CelestialBody::Earth => None,
        CelestialBody::Moon => Some(1.62 / 9.81),
        CelestialBody::Mars => Some(3.721 / 9.81),
    }
}

/// Which data sources the credits name.
#[derive(Clone, Copy, Debug)]
pub struct Credits {
    pub body_earth: bool,
    pub overture: bool,
    pub canopy: bool,
    pub mapillary: bool,
}

impl Credits {
    pub fn from_args(args: &Args) -> Self {
        Self {
            body_earth: args.body.is_earth(),
            overture: args.overture && !args.skip_objects(),
            canopy: args.canopy_height,
            mapillary: args.mapillary_facades_on(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Destination {
    pub name: String,
    pub label: String,
    pub x: i32,
    pub z: i32,
    /// Ground height there; unknown sends the player to the highest block instead.
    pub ground_y: Option<i32>,
}

/// Everything the pack is written from.
pub struct PackContext {
    pub extras: Extras,
    pub area: String,
    pub body: CelestialBody,
    pub places: Vec<Place>,
    pub destinations: Vec<Destination>,
    pub credits: Credits,
    /// Traffic shows red and amber together before green.
    pub red_amber: bool,
    /// The functions that load the car lanes and set what drivers check ahead, as
    /// (path in the pack, text).
    pub car_functions: Vec<(String, String)>,
}

impl PackContext {
    /// Whether there is a discoveries tab, which an area without named places lacks.
    pub fn has_discoveries(&self) -> bool {
        self.extras.discoveries && !self.places.is_empty()
    }

    /// Spawn first, then the best places.
    pub fn destinations_from(
        places: &[Place],
        spawn: Option<(i32, i32, Option<i32>)>,
    ) -> Vec<Destination> {
        let mut out = Vec::new();
        if let Some((x, z, ground_y)) = spawn {
            out.push(Destination {
                name: "Spawn".to_string(),
                label: "Where you started".to_string(),
                x,
                z,
                ground_y,
            });
        }
        out.extend(places.iter().take(MAX_DESTINATIONS).map(|p| Destination {
            name: p.name.clone(),
            label: p.label.clone(),
            x: p.anchor.0,
            z: p.anchor.1,
            ground_y: p.ground_y,
        }));
        out
    }
}

/// Writes the pack into the world and enables it in `level.dat`.
/// A coordinate for a macro to pass on, to the centimetre and never whole: a macro writes
/// 48.0 as 48, and Minecraft puts a whole x or z in the middle of its block, half a block off.
pub(crate) fn macro_coord(v: f64) -> f64 {
    let r = (v * 100.0).round() / 100.0;
    if r.fract() == 0.0 {
        r + 0.01
    } else {
        r
    }
}

pub fn install(world: &Path, ctx: &PackContext) -> Result<(), String> {
    let root = world.join("datapacks").join(PACK_NAME);
    for (path, bytes) in files(ctx) {
        let dest = root.join(&path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create {parent:?}: {e}"))?;
        }
        fs::write(&dest, bytes).map_err(|e| format!("Failed to write {dest:?}: {e}"))?;
    }
    crate::world_utils::enable_datapack_in_level_dat(world, PACK_NAME)?;
    if ctx.extras.pause_menu {
        // Only the logo needs it, so a failure costs the logo and nothing else.
        if let Err(e) = resources::install_banner(world) {
            eprintln!("Warning: the pause menu logo was left out of resources.zip: {e}");
        }
    }
    if ctx.extras.outbreak {
        crate::world_utils::start_in_survival(world)?;
    }
    Ok(())
}

fn json_text(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON values always serialise")
}

fn pretty(value: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(value).expect("JSON values always serialise")
}

/// Every file of the pack, as (path in the pack, contents).
fn files(ctx: &PackContext) -> Vec<(String, Vec<u8>)> {
    let ex = ctx.extras;
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut add_static = |set: &[(&str, &[u8])]| {
        out.extend(set.iter().map(|(p, b)| (p.to_string(), b.to_vec())));
    };
    add_static(CORE);
    if ex.gravity {
        add_static(GRAVITY);
    }
    if ex.traffic_lights {
        add_static(TRAFFIC);
    }
    if ex.pause_menu {
        add_static(MENU);
    }
    if ex.outbreak {
        add_static(OUTBREAK);
    }
    if ex.cars {
        add_static(CARS);
    }

    out.push(("pack.mcmeta".to_string(), pretty(&pack_mcmeta(ex))));
    out.push((
        "pack.png".to_string(),
        include_bytes!("../../assets/minecraft/icon.png").to_vec(),
    ));

    // Each ticking feature keeps its functions in a folder of its own name.
    let ticking: Vec<&str> = [
        (ex.gravity, "gravity"),
        (ex.traffic_lights, "traffic"),
        (ex.pause_menu, "menu"),
        (ex.outbreak, "outbreak"),
        (ex.cars, "cars"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    let mut load = vec!["arnis:load".to_string()];
    load.extend(ticking.iter().map(|f| format!("arnis:{f}/load")));
    let tick: Vec<String> = ticking.iter().map(|f| format!("arnis:{f}/tick")).collect();
    out.push((
        "data/minecraft/tags/function/load.json".to_string(),
        pretty(&json!({ "values": load })),
    ));
    out.push((
        "data/minecraft/tags/function/tick.json".to_string(),
        pretty(&json!({ "values": tick })),
    ));

    let mut config = format!(
        "data modify storage arnis:world area set value {}\n",
        json_text(&json!(ctx.area))
    );
    config.push_str(&format!(
        "scoreboard players set #red_amber arnis {}\n",
        u8::from(ctx.red_amber)
    ));
    out.push((
        "data/arnis/function/config.mcfunction".to_string(),
        config.into_bytes(),
    ));

    if ex.cars {
        out.extend(
            ctx.car_functions
                .iter()
                .map(|(path, text)| (path.clone(), text.clone().into_bytes())),
        );
    }

    if ex.gravity {
        if let Some(r) = gravity_ratio(ctx.body) {
            out.push((
                "data/arnis/function/gravity/apply.mcfunction".to_string(),
                gravity_apply(r).into_bytes(),
            ));
        }
    }

    if ex.pause_menu {
        out.push((
            "data/arnis/function/menu/destinations.mcfunction".to_string(),
            destinations(&ctx.destinations).into_bytes(),
        ));
        out.push((
            "data/arnis/function/menu/welcome.mcfunction".to_string(),
            welcome(ctx, false).into_bytes(),
        ));
        out.push((
            "ov80/data/arnis/function/menu/welcome.mcfunction".to_string(),
            welcome(ctx, true).into_bytes(),
        ));
    }

    if ctx.has_discoveries() {
        let dir = format!("data/arnis/advancement/discoveries/{}", discoveries::FOLDER);
        out.push((
            format!("{dir}/root.json"),
            pretty(&discoveries::root(&ctx.area, ctx.places.len(), false)),
        ));
        out.push((
            format!("ov71/{dir}/root.json"),
            pretty(&discoveries::root(&ctx.area, ctx.places.len(), true)),
        ));
        for (prefix, schema) in [
            ("", CriteriaSchema::Legacy),
            ("ov107/", CriteriaSchema::Namespaced),
            ("ov121/", CriteriaSchema::Typed),
        ] {
            for (path, adv) in discoveries::tree(&ctx.area, &ctx.places, schema) {
                out.push((format!("{prefix}{dir}/{path}.json"), pretty(&adv)));
            }
        }
    }

    if ex.pause_menu || ex.outbreak {
        let ov = "ov80/data/arnis/dialog";
        let pause = if ex.pause_menu {
            "arnis:menu"
        } else {
            "arnis:outbreak"
        };
        out.push((
            "ov80/data/minecraft/tags/dialog/pause_screen_additions.json".to_string(),
            pretty(&json!({ "values": [pause] })),
        ));
        if ex.pause_menu {
            out.push((format!("{ov}/menu.json"), pretty(&dialogs::menu(ctx))));
            out.push((format!("{ov}/credits.json"), pretty(&dialogs::credits(ctx))));
            if !ctx.destinations.is_empty() {
                out.push((format!("{ov}/travel.json"), pretty(&dialogs::travel(ctx))));
            }
            if ctx.has_discoveries() {
                out.push((
                    format!("{ov}/discoveries.json"),
                    pretty(&dialogs::discoveries(ctx)),
                ));
            }
        }
        if ex.outbreak {
            out.push((
                format!("{ov}/outbreak.json"),
                pretty(&dialogs::outbreak(ctx)),
            ));
        }
    }

    if ex.outbreak {
        let subtitle = |target: &str, text: String, color: &str| {
            format!(
                "title {target} subtitle {}\n",
                json_text(&json!({ "text": text, "color": color }))
            )
            .into_bytes()
        };
        out.push((
            "ov80/data/arnis/function/outbreak/text/intro.mcfunction".to_string(),
            subtitle("@s", format!("Survive five nights in {}", ctx.area), "gray"),
        ));
        out.push((
            "ov80/data/arnis/function/outbreak/text/victory.mcfunction".to_string(),
            subtitle("@a", format!("{} survived", ctx.area), "yellow"),
        ));
    }
    out
}

fn pack_mcmeta(ex: Extras) -> Value {
    let mut features = Vec::new();
    for (on, name) in [
        (ex.gravity, "gravity"),
        (ex.traffic_lights, "traffic lights"),
        (ex.discoveries, "discoveries"),
        (ex.pause_menu, "pause menu"),
        (ex.outbreak, "zombie outbreak"),
        (ex.cars, "moving cars"),
    ] {
        if on {
            features.push(name);
        }
    }
    // Every overlay names `formats` too: 1.21.9+ requires it of all overlays as soon as one
    // reaches below format 82, which these do.
    let overlays: Vec<Value> = OVERLAYS
        .iter()
        .map(|&(min, dir)| {
            json!({
                "formats": { "min_inclusive": min, "max_inclusive": MAX_FORMAT },
                "min_format": [min, 0],
                "max_format": [MAX_FORMAT, MAX_FORMAT],
                "directory": dir,
            })
        })
        .collect();
    json!({
        "pack": {
            "description": format!("Arnis extras: {}", features.join(", ")),
            "pack_format": BASE_FORMAT,
            "supported_formats": { "min_inclusive": BASE_FORMAT, "max_inclusive": MAX_FORMAT },
            "min_format": [BASE_FORMAT, 0],
            "max_format": [MAX_FORMAT, MAX_FORMAT],
        },
        "overlays": { "entries": overlays },
    })
}

/// Scales gravity by `r` and fall damage with it, so a fall hurts as it would on Earth from
/// `r` of the height.
fn gravity_apply(r: f64) -> String {
    let lower = r - 1.0;
    let safe = 1.0 / r - 1.0;
    format!(
        "tag @s add arnis.gravity\n\
         attribute @s minecraft:gravity modifier add arnis:body_gravity {lower:.4} add_multiplied_base\n\
         attribute @s minecraft:safe_fall_distance modifier add arnis:body_gravity {safe:.4} add_multiplied_base\n\
         attribute @s minecraft:fall_damage_multiplier modifier add arnis:body_gravity {lower:.4} add_multiplied_base\n"
    )
}

fn destinations(list: &[Destination]) -> String {
    let mut out = String::new();
    for (i, d) in list.iter().enumerate() {
        let id = i + 1;
        // A little above the ground, so the search down lands in the street and not on
        // anything built over it.
        let y = d.ground_y.map_or(TOP_SEARCH_Y, |g| g + 4);
        out.push_str(&format!(
            "execute if score @s arnis.tp matches {id} run function arnis:menu/go {{x:{},y:{y},z:{}}}\n",
            d.x, d.z
        ));
        out.push_str(&format!(
            "execute if score @s arnis.tp matches {id} run title @s actionbar {}\n",
            json_text(&json!({ "text": d.name, "color": "gold" }))
        ));
    }
    out
}

fn welcome(ctx: &PackContext, dialogs: bool) -> String {
    let line = |v: Value| format!("tellraw @s {}\n", json_text(&v));
    let mut out = String::from("scoreboard players set @s arnis.seen 1\n");
    out.push_str(&line(json!([
        "",
        { "text": "Welcome to ", "color": "gray" },
        { "text": ctx.area, "color": "gold" },
        { "text": ". This world was generated by Arnis from real map data.", "color": "gray" },
    ])));
    if dialogs {
        out.push_str(&line(json!({
            "text": "Open the pause menu and choose Arnis for travel, discoveries and credits.",
            "color": "gray",
        })));
    } else if ctx.has_discoveries() {
        out.push_str(&line(json!({
            "text": format!("Your discoveries are in the Advancements screen (L), under {}.", ctx.area),
            "color": "gray",
        })));
    }
    if dialogs {
        out.push_str(&line(json!({
            "text": "Map data © OpenStreetMap contributors (ODbL).",
            "color": "dark_gray",
        })));
    } else {
        // Older versions have no dialogs, so the credits go to chat.
        for credit in ctx.credits.lines() {
            out.push_str(&line(json!({ "text": credit, "color": "dark_gray" })));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(extras: Extras) -> PackContext {
        PackContext {
            extras,
            area: "Kreuzberg \"Süd\"".to_string(),
            body: CelestialBody::Moon,
            places: Vec::new(),
            destinations: vec![Destination {
                name: "Spawn".into(),
                label: "Start".into(),
                x: 1,
                z: 2,
                ground_y: Some(70),
            }],
            credits: Credits {
                body_earth: true,
                overture: false,
                canopy: false,
                mapillary: false,
            },
            red_amber: true,
            car_functions: Vec::new(),
        }
    }

    fn all() -> Extras {
        Extras {
            gravity: true,
            traffic_lights: true,
            discoveries: true,
            pause_menu: true,
            outbreak: true,
            cars: true,
        }
    }

    #[test]
    fn every_function_a_tag_names_exists_in_the_base_layer() {
        let files = files(&ctx(all()));
        let has = |p: &str| files.iter().any(|(f, _)| f == p);
        for tag in ["load", "tick"] {
            let (_, bytes) = files
                .iter()
                .find(|(f, _)| *f == format!("data/minecraft/tags/function/{tag}.json"))
                .unwrap();
            let v: Value = serde_json::from_slice(bytes).unwrap();
            for id in v["values"].as_array().unwrap() {
                let path = id.as_str().unwrap().replace("arnis:", "");
                assert!(
                    has(&format!("data/arnis/function/{path}.mcfunction")),
                    "{path}"
                );
            }
        }
    }

    #[test]
    fn the_clock_overlay_only_replaces_base_functions() {
        let files = files(&ctx(all()));
        for (path, _) in files.iter().filter(|(p, _)| p.starts_with("ov101/")) {
            let base = path.trim_start_matches("ov101/");
            assert!(files.iter().any(|(p, _)| p == base), "{path}");
        }
    }

    #[test]
    fn every_bundled_file_is_shipped_by_some_feature() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/minecraft/datapack_extras");
        let listed: std::collections::HashSet<&str> =
            [CORE, GRAVITY, TRAFFIC, MENU, OUTBREAK, CARS]
                .iter()
                .flat_map(|set| set.iter().map(|(p, _)| *p))
                .collect();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let rel = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                assert!(listed.contains(rel.as_str()), "{rel} is never shipped");
            }
        }
    }

    #[test]
    fn json_files_parse_and_area_names_are_escaped() {
        for (path, bytes) in files(&ctx(all())) {
            if path.ends_with(".json") || path.ends_with(".mcmeta") {
                serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
            }
            if path.ends_with("config.mcfunction") {
                let text = String::from_utf8(bytes).unwrap();
                assert!(text.contains(r#""Kreuzberg \"Süd\"""#), "{text}");
            }
        }
    }

    #[test]
    fn moon_gravity_keeps_a_fall_as_harmful_as_on_earth_from_a_sixth_of_the_height() {
        let text = gravity_apply(1.62 / 9.81);
        assert!(text.contains("minecraft:gravity modifier add arnis:body_gravity -0.8349"));
        assert!(text.contains("safe_fall_distance modifier add arnis:body_gravity 5.0556"));
    }

    #[test]
    fn disabled_features_ship_nothing_but_the_core() {
        let only = Extras {
            traffic_lights: true,
            ..Extras::default()
        };
        let files = files(&ctx(only));
        assert!(files.iter().any(|(p, _)| p.contains("traffic/tick")));
        assert!(!files
            .iter()
            .any(|(p, _)| p.contains("outbreak") || p.contains("dialog")));
    }
}
