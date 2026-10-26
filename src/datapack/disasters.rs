//! Disasters: one after another hits the area, in random order, picked from those the place
//! can have. A flood or a tsunami needs the terrain, so both are worked out here as the fills
//! that raise the water a level at a time; everything else runs from the static functions in
//! `assets/minecraft/datapack_extras/ov80/data/arnis/function/disaster`, which also hold the
//! scores and storage this writes.

use super::json_text;
use crate::celestial::CelestialBody;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::land_cover::LC_TREE_COVER;
use crate::osm_parser::ProcessedElement;
use crate::world_editor::common::Section;
use rayon::prelude::*;
use serde_json::json;
use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fmt::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

const DIR: &str = "ov80/data/arnis/function/disaster";

/// Levels a flood or a tsunami can rise to, in blocks. The pack picks how far each one goes.
const FLOOD_LEVELS: u8 = 8;
const TSUNAMI_LEVELS: u8 = 7;
/// Side of the cells the pack brings up to date one at a time, in blocks.
const CELL: i32 = 64;
/// Most columns sampled for a water map; a larger area is sampled every 2, 4, ... blocks.
const MAX_SAMPLES: u64 = 16_000_000;
/// Most fills a water map may hold before it gives up its highest level.
const FILL_BUDGET: usize = 120_000;
/// How far open water beside flooded land rises with it, in blocks.
const SHORE_REACH: i32 = 48;
/// Furthest below its own surface the water from a source spreads, in blocks. Spilling
/// further down, it would run off to lower water rather than fill the land up to its level:
/// a canal held between quays floods the quays, not the low town below them.
const MAX_DROP: i32 = 2;
/// The most blocks one fill may change in a world with default game rules.
const MAX_FILL_VOLUME: i64 = 32_768;
/// Smallest body of water a flood rises from, in blocks, unless it runs off the area: a
/// river or a lake, never a pond.
const RIVER_AREA: i64 = 6_000;
/// Smallest body of water at sea level that counts as sea, in blocks.
const SEA_AREA: i64 = 20_000;
/// Flood water is held in unlit light blocks: it looks and swims like water, and a drain can
/// take out exactly what a flood put in, never a pond or a pool that was there before. It goes
/// in strict, without block updates, so it does not start to flow; the drain makes them, so
/// whatever did spill dries up behind it.
const FLOOD_WATER: &str = "minecraft:light[level=0,waterlogged=true]";
const FLOOD_DRAIN: &str = "minecraft:light[level=0]";
/// The widest a sinkhole gets, in blocks from its middle.
const SINK_RINGS: i32 = 15;
/// Snowy biomes that stand in for others while a snowstorm passes, so it snows there. One the
/// world already has can't, as turning it back afterwards would change that one too.
const SNOW_BIOMES: [&str; 8] = [
    "minecraft:snowy_plains",
    "minecraft:snowy_taiga",
    "minecraft:snowy_slopes",
    "minecraft:snowy_beach",
    "minecraft:grove",
    "minecraft:ice_spikes",
    "minecraft:frozen_peaks",
    "minecraft:jagged_peaks",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Disaster {
    Meteors = 1,
    Storm,
    Hail,
    Sinkholes,
    Tornado,
    Quake,
    Flood,
    Wildfire,
    Tsunami,
    Kaiju,
    Snowstorm,
}

impl Disaster {
    const ALL: [Disaster; 11] = [
        Disaster::Meteors,
        Disaster::Storm,
        Disaster::Hail,
        Disaster::Sinkholes,
        Disaster::Tornado,
        Disaster::Quake,
        Disaster::Flood,
        Disaster::Wildfire,
        Disaster::Tsunami,
        Disaster::Kaiju,
        Disaster::Snowstorm,
    ];

    fn name(self, body: CelestialBody) -> &'static str {
        match (self, body) {
            (Self::Meteors, _) => "Meteor Shower",
            (Self::Storm, _) => "Thunderstorm",
            (Self::Hail, _) => "Hailstorm",
            (Self::Sinkholes, _) => "Sinkholes",
            (Self::Tornado, CelestialBody::Mars) => "Dust Devil",
            (Self::Tornado, _) => "Tornado",
            (Self::Quake, CelestialBody::Moon) => "Moonquake",
            (Self::Quake, CelestialBody::Mars) => "Marsquake",
            (Self::Quake, _) => "Earthquake",
            (Self::Flood, _) => "Flood",
            (Self::Wildfire, _) => "Wildfire",
            (Self::Tsunami, _) => "Tsunami",
            (Self::Kaiju, _) => "Kaiju",
            (Self::Snowstorm, _) => "Snowstorm",
        }
    }

    /// The subtitle under the warning: what to do about it.
    fn advice(self) -> &'static str {
        match self {
            Self::Meteors => "Keep out of the red rings",
            Self::Storm => "Stay off the rooftops",
            Self::Hail => "Get under a roof",
            Self::Sinkholes => "Move when the ground cracks",
            Self::Tornado => "Stay out of its path",
            Self::Quake => "Get out of the buildings",
            Self::Flood => "Get to high ground",
            Self::Wildfire => "Keep ahead of the fire",
            Self::Tsunami => "Get away from the coast",
            Self::Kaiju => "Run, it can't be stopped",
            Self::Snowstorm => "Get inside and keep warm",
        }
    }

    fn over(self, body: CelestialBody) -> &'static str {
        match (self, body) {
            (Self::Meteors, _) => "The meteor shower is over.",
            (Self::Storm, _) => "The storm has passed.",
            (Self::Hail, _) => "The hail has stopped.",
            (Self::Sinkholes, _) => "The ground has settled.",
            (Self::Tornado, CelestialBody::Mars) => "The dust devil is gone.",
            (Self::Tornado, _) => "The tornado is gone.",
            (Self::Quake, _) => "The shaking has stopped.",
            (Self::Flood, _) => "The water has gone down.",
            (Self::Wildfire, _) => "The fire front has passed.",
            (Self::Tsunami, _) => "The sea has pulled back.",
            (Self::Kaiju, _) => "The kaiju went back underground.",
            (Self::Snowstorm, _) => "The snow has stopped.",
        }
    }

    /// As it reads in a list of what is coming.
    fn phrase(self, body: CelestialBody) -> &'static str {
        match (self, body) {
            (Self::Meteors, _) => "a meteor shower",
            (Self::Storm, _) => "a thunderstorm",
            (Self::Hail, _) => "a hailstorm",
            (Self::Sinkholes, _) => "sinkholes",
            (Self::Tornado, CelestialBody::Mars) => "a dust devil",
            (Self::Tornado, _) => "a tornado",
            (Self::Quake, CelestialBody::Moon) => "a moonquake",
            (Self::Quake, CelestialBody::Mars) => "a marsquake",
            (Self::Quake, _) => "an earthquake",
            (Self::Flood, _) => "a flood",
            (Self::Wildfire, _) => "a wildfire",
            (Self::Tsunami, _) => "a tsunami",
            (Self::Kaiju, _) => "a kaiju",
            (Self::Snowstorm, _) => "a snowstorm",
        }
    }
}

/// Which disasters an area gets, and the water maps for those that need one.
pub struct Plan {
    body: CelestialBody,
    kinds: Vec<Disaster>,
    flood: Option<WaterMap>,
    tsunami: Option<WaterMap>,
    /// The bedrock plane, which nothing may dig into.
    floor: i32,
    /// Which biome turns into which snowy one while a snowstorm passes.
    snow: Vec<(&'static str, &'static str)>,
    /// The highest the biomes need to change for it, above the tallest roof.
    snow_top: i32,
}

impl Plan {
    /// From the world as it was written; without a recording of it nothing can flood.
    pub fn build(
        surface: Option<&Surface>,
        ground: &Ground,
        world: &XZBBox,
        body: CelestialBody,
        wooded: bool,
    ) -> Self {
        let surface = surface.filter(|_| body.is_earth());
        // Sampled once for both maps: on a large area that is the bulk of the work.
        let grid = surface.map(|s| Grid::sample(s, world));
        let flood = grid
            .as_ref()
            .and_then(|g| WaterMap::from_grid(g, FLOOD_LEVELS, None));
        // Water at sea level counts as sea only with real heights to tell it by.
        let sea_y = ground
            .elevation_affine()
            .map(|a| a.y_for_metres(0.0).round() as i32);
        let tsunami = grid
            .as_ref()
            .zip(sea_y)
            .and_then(|(g, y)| WaterMap::from_grid(g, TSUNAMI_LEVELS, Some(y)));
        drop(grid);
        let floor = crate::world_editor::terrain_floor_y();
        let mut plan = Self::with_maps(body, wooded, flood, tsunami, floor);
        plan.snow = surface.map_or_else(Vec::new, |s| snow_swaps(&s.biomes()));
        let roofs = surface.and_then(Surface::highest).map_or(0, |y| y + 32);
        plan.snow_top = (floor + 383)
            .max(roofs)
            .min(crate::world_editor::world_max_y());
        plan
    }

    #[cfg(test)]
    pub(crate) fn for_tests(body: CelestialBody) -> Self {
        Self::with_maps(body, true, None, None, -64)
    }

    /// Without a recording of the world: no water maps and no woods to burn.
    pub fn without_maps(body: CelestialBody) -> Self {
        Self::with_maps(
            body,
            false,
            None,
            None,
            crate::world_editor::terrain_floor_y(),
        )
    }

    fn with_maps(
        body: CelestialBody,
        wooded: bool,
        flood: Option<WaterMap>,
        tsunami: Option<WaterMap>,
        floor: i32,
    ) -> Self {
        let kinds = Disaster::ALL
            .into_iter()
            .filter(|d| match body {
                CelestialBody::Earth => match d {
                    Disaster::Flood => flood.is_some(),
                    Disaster::Tsunami => tsunami.is_some(),
                    Disaster::Wildfire => wooded,
                    _ => true,
                },
                CelestialBody::Moon => {
                    matches!(d, Disaster::Meteors | Disaster::Sinkholes | Disaster::Quake)
                }
                CelestialBody::Mars => matches!(
                    d,
                    Disaster::Meteors | Disaster::Sinkholes | Disaster::Quake | Disaster::Tornado
                ),
            })
            .collect();
        Self {
            body,
            kinds,
            flood,
            tsunami,
            floor,
            snow: Vec::new(),
            snow_top: floor + 383,
        }
    }

    /// For a 64-block cell, the biome swaps that make it snow there and the ones that undo
    /// them, from the floor to above the tallest roof, 384 blocks at a time.
    fn snow_functions(&self) -> (String, String) {
        let (mut swap, mut thaw) = (String::new(), String::new());
        for y0 in (self.floor..=self.snow_top).step_by(384) {
            let y1 = (y0 + 383).min(self.snow_top);
            for (from, to) in &self.snow {
                let _ = writeln!(
                    swap,
                    "$fillbiome $(x0) {y0} $(z0) $(x1) {y1} $(z1) {to} replace {from}"
                );
                let _ = writeln!(
                    thaw,
                    "$fillbiome $(x0) {y0} $(z0) $(x1) {y1} $(z1) {from} replace {to}"
                );
            }
        }
        if self.snow.is_empty() {
            swap.push_str("# It snows here as it is.\n");
            thaw.push_str("# It snows here as it is.\n");
        }
        (swap, thaw)
    }

    /// One line for the log.
    pub fn summary(&self) -> String {
        let map = |m: &Option<WaterMap>| match m {
            Some(m) => format!("{} levels, {} fills", m.levels, m.fill_count()),
            None => "none".to_string(),
        };
        format!(
            "{} disasters; flood map {}; tsunami map {}",
            self.kinds.len(),
            map(&self.flood),
            map(&self.tsunami)
        )
    }

    /// What is coming, as a sentence fragment: "a flood, a tornado and a kaiju".
    pub fn coming(&self) -> String {
        let phrases: Vec<&str> = self.kinds.iter().map(|d| d.phrase(self.body)).collect();
        match phrases.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
            Some((last, _)) => last.to_string(),
            None => String::new(),
        }
    }

    /// The generated files of the pack, as (path in the pack, contents).
    pub fn files(&self, area: &str) -> Vec<(String, Vec<u8>)> {
        let mut out = vec![
            (format!("{DIR}/area.mcfunction"), self.area_function()),
            (format!("{DIR}/text/intro.mcfunction"), self.intro(area)),
            (
                format!("{DIR}/text/coming.mcfunction"),
                self.coming_line(area),
            ),
            (format!("{DIR}/text/victory.mcfunction"), self.victory(area)),
            (
                format!("{DIR}/tornado/funnel.mcfunction"),
                funnel(self.body),
            ),
            (format!("{DIR}/meteor/ring.mcfunction"), ring(3.0, 12)),
            (format!("{DIR}/meteor/ring_big.mcfunction"), ring(5.0, 18)),
        ];
        out.extend(
            (0..=SINK_RINGS).map(|r| (format!("{DIR}/sinkhole/ring/{r}.mcfunction"), sink_ring(r))),
        );
        let (swap, thaw) = self.snow_functions();
        out.push((format!("{DIR}/snow/swap.mcfunction"), swap));
        out.push((format!("{DIR}/snow/thaw.mcfunction"), thaw));
        for (name, map) in [("flood", &self.flood), ("tsunami", &self.tsunami)] {
            out.push((
                format!("{DIR}/{name}/tops.mcfunction"),
                map.as_ref().map_or_else(
                    || "# No water here to rise.\n".to_string(),
                    |m| m.tops(name),
                ),
            ));
            if let Some(m) = map {
                out.extend(m.cell_functions(name));
            }
        }
        out.into_iter().map(|(p, s)| (p, s.into_bytes())).collect()
    }

    fn area_function(&self) -> String {
        let mut out = String::new();
        let ids: Vec<String> = self.kinds.iter().map(|d| (*d as u8).to_string()).collect();
        let _ = writeln!(
            out,
            "data modify storage arnis:ds pool set value [{}]",
            ids.join(",")
        );
        let _ = writeln!(out, "scoreboard players set #floor arnis.ds {}", self.floor);
        let snows = self.kinds.contains(&Disaster::Snowstorm) && !self.snow.is_empty();
        let _ = writeln!(
            out,
            "scoreboard players set #has_snow arnis.ds {}",
            u8::from(snows)
        );
        let dust = if self.body == CelestialBody::Mars {
            "minecraft:dust_plume"
        } else {
            "minecraft:cloud"
        };
        let _ = writeln!(
            out,
            "data modify storage arnis:ds wind_p set value \"{dust}\""
        );
        // Indexed by the disaster's number, so the first entry is never read.
        let list = |text: &dyn Fn(Disaster) -> &'static str| {
            let mut v = vec![json!("")];
            v.extend(Disaster::ALL.iter().map(|d| json!(text(*d))));
            json_text(&json!(v))
        };
        let body = self.body;
        let _ = writeln!(
            out,
            "data modify storage arnis:ds names set value {}",
            list(&|d| d.name(body))
        );
        let _ = writeln!(
            out,
            "data modify storage arnis:ds subs set value {}",
            list(&|d| d.advice())
        );
        let _ = writeln!(
            out,
            "data modify storage arnis:ds ends set value {}",
            list(&|d| d.over(body))
        );
        for (name, map) in [("flood", &self.flood), ("tsunami", &self.tsunami)] {
            let _ = writeln!(
                out,
                "scoreboard players set #has_{name} arnis.ds {}",
                u8::from(map.is_some())
            );
            let _ = writeln!(
                out,
                "scoreboard players set #k_{name} arnis.ds {}",
                map.as_ref().map_or(0, |m| m.levels)
            );
            let _ = writeln!(
                out,
                "data modify storage arnis:ds {name}.cells set value {{}}"
            );
            if let Some(m) = map {
                for chunk in m.cells.chunks(400) {
                    let ids: Vec<String> = chunk
                        .iter()
                        .map(|((cx, cz), _)| format!("\"{cx}_{cz}\":1b"))
                        .collect();
                    let _ = writeln!(
                        out,
                        "data modify storage arnis:ds {name}.cells merge value {{{}}}",
                        ids.join(",")
                    );
                }
            }
        }
        out
    }

    fn intro(&self, area: &str) -> String {
        format!(
            "title @s subtitle {}\n",
            json_text(&json!({
                "text": format!("Survive {} in {area}", count(self.kinds.len(), "disaster")),
                "color": "gray",
            }))
        )
    }

    fn coming_line(&self, area: &str) -> String {
        format!(
            "tellraw @s {}\n",
            json_text(&json!({
                "text": format!("Coming to {area}, in no set order: {}.", self.coming()),
                "color": "gray",
            }))
        )
    }

    fn victory(&self, area: &str) -> String {
        format!(
            "title @a subtitle {}\n",
            json_text(&json!({
                "text": format!("{area} survived all {}", number(self.kinds.len())),
                "color": "yellow",
            }))
        )
    }
}

fn number(n: usize) -> String {
    const WORDS: [&str; 13] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        "eleven", "twelve",
    ];
    WORDS
        .get(n)
        .map_or_else(|| n.to_string(), |w| w.to_string())
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("one {noun}")
    } else {
        format!("{} {noun}s", number(n))
    }
}

/// Which biome turns into which snowy one for a snowstorm, the most common first and each into
/// its own snowy counterpart where that one is free. Water keeps its biome: in a snowy one it
/// would freeze over and stay frozen once the storm has passed.
fn snow_swaps(seen: &HashMap<&'static str, u64>) -> Vec<(&'static str, &'static str)> {
    let mut rainy: Vec<(&'static str, u64)> = seen
        .iter()
        .filter(|(b, _)| {
            !b.contains("ocean") && !b.contains("river") && crate::biome::winter_biome(b) != **b
        })
        .map(|(b, n)| (*b, *n))
        .collect();
    rainy.sort_by_key(|&(b, n)| (Reverse(n), b));
    let mut free: Vec<&'static str> = SNOW_BIOMES
        .into_iter()
        .filter(|s| !seen.contains_key(s))
        .collect();
    let mut out = Vec::new();
    for (biome, _) in rainy {
        if free.is_empty() {
            break;
        }
        let own = crate::biome::winter_biome(biome);
        let pick = free.iter().position(|s| *s == own).unwrap_or(0);
        out.push((biome, free.remove(pick)));
    }
    out
}

/// Whether the area has enough wood to burn: mapped forest, or tree cover seen from space.
pub fn wooded(elements: &[ProcessedElement], ground: &Ground, world: &XZBBox) -> bool {
    let mapped = elements.iter().any(|e| {
        let tags = e.tags();
        tags.get("landuse").is_some_and(|v| v == "forest")
            || tags
                .get("natural")
                .is_some_and(|v| matches!(v.as_str(), "wood" | "scrub" | "heath"))
    });
    mapped || tree_share(ground, world) >= 0.03
}

fn tree_share(ground: &Ground, world: &XZBBox) -> f64 {
    if !ground.has_land_cover() {
        return 0.0;
    }
    const SIDE: i32 = 64;
    let (w, h) = (world.max_x() - world.min_x(), world.max_z() - world.min_z());
    let mut trees = 0u32;
    for i in 0..SIDE {
        for j in 0..SIDE {
            let p = XZPoint::new(w * i / (SIDE - 1), h * j / (SIDE - 1));
            if ground.cover_class(p) == LC_TREE_COVER {
                trees += 1;
            }
        }
    }
    f64::from(trees) / f64::from((SIDE * SIDE) as u32)
}

/// The funnel, drawn around the executing entity and turned with it. The cone opens upward
/// into a ring of cloud.
fn funnel(body: CelestialBody) -> String {
    let (color, cloud, ground) = match body {
        CelestialBody::Mars => ("0.72,0.42,0.25", "0.62,0.36,0.22", "minecraft:red_sand"),
        _ => ("0.45,0.45,0.48", "0.32,0.32,0.36", "minecraft:dirt"),
    };
    let mut out = String::new();
    for level in 0..15 {
        let h = f64::from(level) * 3.0;
        let r = 1.0 + 0.1 * h + 0.0028 * h * h;
        let arms = if h < 24.0 { 3 } else { 4 };
        for arm in 0..arms {
            let angle = (level * 17 + arm * 360 / arms) % 360;
            let _ = writeln!(
                out,
                "execute rotated ~{angle} 0 run particle minecraft:dust{{color:[{color}],scale:2.5}} ^{r:.1} ^{h} ^ 0.15 0.2 0.15 0 1 force"
            );
        }
        if level % 2 == 1 {
            let _ = writeln!(
                out,
                "execute rotated ~{} 0 run particle minecraft:large_smoke ^{:.1} ^{h} ^ 0.2 0.2 0.2 0.01 1 force",
                (level * 17 + 60) % 360,
                r * 0.8
            );
        }
    }
    for arm in 0..8 {
        let _ = writeln!(
            out,
            "execute rotated ~{} 0 run particle minecraft:dust{{color:[{cloud}],scale:4.0}} ^12 ^45 ^ 1 0.6 1 0 2 force",
            arm * 45
        );
    }
    let _ = writeln!(
        out,
        "particle minecraft:block{{block_state:\"{ground}\"}} ~ ~0.3 ~ 2.5 0.2 2.5 0.15 6 force"
    );
    let _ = writeln!(
        out,
        "particle minecraft:cloud ~ ~0.5 ~ 2 0.3 2 0.05 3 force"
    );
    out
}

/// The ring a meteor's landing spot, or where the kaiju's blast lands, is marked with.
fn ring(radius: f64, points: i32) -> String {
    let mut out = String::new();
    for p in 0..points {
        let _ = writeln!(
            out,
            "execute rotated ~{} 0 run particle minecraft:dust{{color:[1.0,0.25,0.05],scale:1.6}} ^{radius} ^0.2 ^ 0 0 0 0 1 force",
            p * 360 / points
        );
    }
    let _ = writeln!(
        out,
        "particle minecraft:dust{{color:[1.0,0.55,0.1],scale:1.2}} ~ ~0.3 ~ 0.3 0.1 0.3 0 3 force"
    );
    out
}

/// The columns `r` blocks out from a sinkhole's middle, rounded, so the rings up to any radius
/// make a round hole with no column in two of them. Each drops `$(dd)` blocks, with whatever
/// stands on it, and the top it leaves is cleared from `$(cl)` up.
fn sink_ring(r: i32) -> String {
    let mut out = String::new();
    for dz in -r..=r {
        let mut runs: Vec<(i32, i32)> = Vec::new();
        for dx in -r..=r {
            if ring_of(dx, dz) != r {
                continue;
            }
            match runs.last_mut() {
                Some((_, end)) if *end == dx - 1 => *end = dx,
                _ => runs.push((dx, dx)),
            }
        }
        for (x0, x1) in runs {
            let _ = writeln!(
                out,
                "$clone ~{x0} ~-2 ~{dz} ~{x1} ~30 ~{dz} ~{x0} ~-$(dd) ~{dz} replace force"
            );
            let _ = writeln!(
                out,
                "$fill ~{x0} ~$(cl) ~{dz} ~{x1} ~30 ~{dz} minecraft:air"
            );
        }
    }
    out
}

fn ring_of(dx: i32, dz: i32) -> i32 {
    f64::from(dx * dx + dz * dz).sqrt().round() as i32
}

/// What a water map needs to know of a column, in world coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    /// The height of the ground block.
    Land(i32),
    /// The height of the water surface block.
    Water(i32),
    Outside,
}

trait Terrain: Sync {
    fn column(&self, x: i32, z: i32) -> Column;
}

/// The world as it was written: for each sampled column, the top of what water would rest
/// on and whether that is water itself. Noted chunk by chunk as the regions are saved, so it
/// sees the piers, quays, bridges and buildings over water that the terrain data does not.
pub struct Surface {
    x0: i32,
    z0: i32,
    step: i32,
    w: usize,
    h: usize,
    cells: Vec<AtomicU32>,
    /// Every biome written, by how many 4x4 columns have it.
    biomes: Mutex<HashMap<&'static str, u64>>,
}

const UNSEEN: u32 = u32::MAX;
const WET: u32 = 1 << 16;

static RECORDING: RwLock<Option<Arc<Surface>>> = RwLock::new(None);

impl Surface {
    /// Starts noting every chunk written from now on, or stops a recording left over. The
    /// recording ends when the guard is dropped, if `finish` has not taken it before.
    pub fn record(world: Option<&XZBBox>) -> Recording {
        let surface = world.map(|world| {
            let (x0, z0, step, w, h) = sample_grid(world);
            Arc::new(Self {
                x0,
                z0,
                step,
                w,
                h,
                cells: (0..w * h).map(|_| AtomicU32::new(UNSEEN)).collect(),
                biomes: Mutex::new(HashMap::new()),
            })
        });
        *RECORDING.write().unwrap_or_else(|p| p.into_inner()) = surface;
        Recording
    }

    /// Stops the recording and hands over what it saw.
    pub fn finish() -> Option<Arc<Self>> {
        RECORDING.write().unwrap_or_else(|p| p.into_inner()).take()
    }

    fn index(&self, x: i32, z: i32) -> Option<usize> {
        let (dx, dz) = (x - self.x0, z - self.z0);
        let at = self.step / 2;
        if dx < 0 || dz < 0 || dx % self.step != at || dz % self.step != at {
            return None;
        }
        let (gx, gz) = ((dx / self.step) as usize, (dz / self.step) as usize);
        (gx < self.w && gz < self.h).then(|| gz * self.w + gx)
    }

    /// The highest surface written, roofs included.
    fn highest(&self) -> Option<i32> {
        self.cells
            .iter()
            .map(|c| c.load(Ordering::Relaxed))
            .filter(|&v| v != UNSEEN)
            .map(|v| i32::from(v as u16 as i16))
            .max()
    }

    fn biomes(&self) -> HashMap<&'static str, u64> {
        self.biomes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn ingest(&self, chunk_x: i32, chunk_z: i32, sections: &[Section], biomes: &[&'static str]) {
        // Filler chunks around the area: nothing of them is kept.
        let (x, z) = (chunk_x * 16, chunk_z * 16);
        let (x_end, z_end) = (
            self.x0 + self.w as i32 * self.step,
            self.z0 + self.h as i32 * self.step,
        );
        if x + 16 <= self.x0 || z + 16 <= self.z0 || x >= x_end || z >= z_end {
            return;
        }
        {
            let mut seen = self.biomes.lock().unwrap_or_else(|p| p.into_inner());
            for b in biomes {
                *seen.entry(*b).or_insert(0) += 1;
            }
        }
        let mut layers: Vec<(&Section, Vec<u8>)> = sections
            .iter()
            .map(|s| {
                let classes = s
                    .block_states
                    .palette
                    .iter()
                    .map(|p| surface_class(&p.name));
                (s, classes.collect())
            })
            .collect();
        layers.sort_by_key(|(s, _)| Reverse(s.y));
        for lz in 0..16usize {
            for lx in 0..16usize {
                let Some(i) = self.index(chunk_x * 16 + lx as i32, chunk_z * 16 + lz as i32) else {
                    continue;
                };
                let top = column_top(&layers, lx, lz).map_or(UNSEEN, |(y, wet)| {
                    u32::from(y as u16) | if wet { WET } else { 0 }
                });
                self.cells[i].store(top, Ordering::Relaxed);
            }
        }
    }
}

/// Holds a recording open; dropping it ends the recording.
#[must_use]
pub struct Recording;

impl Drop for Recording {
    fn drop(&mut self) {
        Surface::finish();
    }
}

/// Notes the surface and the biomes of a chunk as it is written, while a recording runs.
pub(crate) fn record_chunk(
    chunk_x: i32,
    chunk_z: i32,
    sections: &[Section],
    biomes: &[&'static str],
) {
    let guard = RECORDING.read().unwrap_or_else(|p| p.into_inner());
    if let Some(surface) = guard.as_ref() {
        surface.ingest(chunk_x, chunk_z, sections, biomes);
    }
}

impl Terrain for Surface {
    fn column(&self, x: i32, z: i32) -> Column {
        let Some(i) = self.index(x, z) else {
            return Column::Outside;
        };
        match self.cells[i].load(Ordering::Relaxed) {
            UNSEEN => Column::Outside,
            v if v & WET != 0 => Column::Water(i32::from(v as u16 as i16)),
            v => Column::Land(i32::from(v as u16 as i16)),
        }
    }
}

const PASS: u8 = 0;
const SOLID: u8 = 1;
const FLUID: u8 = 2;

/// Whether water passes a block, rests on it, or is it. Plants, leaves, lamps, signs and
/// fences let it through; it fills in around them.
fn surface_class(name: &str) -> u8 {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    if matches!(
        n,
        "water" | "bubble_column" | "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass"
    ) {
        return FLUID;
    }
    // Parts of names; air is matched whole, as stairs would contain it.
    const PASSES: [&str; 32] = [
        "leaves",
        "sapling",
        "torch",
        "carpet",
        "rail",
        "sign",
        "banner",
        "button",
        "pressure_plate",
        "candle",
        "flower",
        "tulip",
        "orchid",
        "allium",
        "bluet",
        "daisy",
        "poppy",
        "dandelion",
        "lily",
        "rose",
        "peony",
        "lilac",
        "eyeblossom",
        "grass",
        "fern",
        "bush",
        "vine",
        "petals",
        "fence",
        "pane",
        "bars",
        "chain",
    ];
    let solid_kin = n.ends_with("_block") || n.ends_with("_stem");
    let passes = PASSES.iter().any(|p| n.contains(p))
        || n.ends_with("_wall")
        || matches!(
            n,
            "air"
                | "cave_air"
                | "void_air"
                | "light"
                | "snow"
                | "ladder"
                | "lever"
                | "tripwire"
                | "cobweb"
                | "sugar_cane"
                | "leaf_litter"
        );
    if passes && !solid_kin {
        PASS
    } else {
        SOLID
    }
}

/// The highest block of a column that water does not pass, and whether it is water.
fn column_top(layers: &[(&Section, Vec<u8>)], lx: usize, lz: usize) -> Option<(i32, bool)> {
    for (section, classes) in layers {
        let base = i32::from(section.y) * 16;
        let Some(data) = section
            .block_states
            .data
            .as_ref()
            .filter(|_| classes.len() > 1)
        else {
            match classes.first() {
                Some(&SOLID) => return Some((base + 15, false)),
                Some(&FLUID) => return Some((base + 15, true)),
                _ => continue,
            }
        };
        let mut bits = 4;
        while (1usize << bits) < classes.len() {
            bits += 1;
        }
        let per_long = 64 / bits;
        let mask = (1u64 << bits) - 1;
        for ly in (0..16usize).rev() {
            let idx = ly * 256 + lz * 16 + lx;
            let Some(&long) = data.get(idx / per_long) else {
                continue;
            };
            let palette = ((long as u64 >> ((idx % per_long) * bits)) & mask) as usize;
            match classes.get(palette) {
                Some(&SOLID) => return Some((base + ly as i32, false)),
                Some(&FLUID) => return Some((base + ly as i32, true)),
                _ => {}
            }
        }
    }
    None
}

/// The corner, spacing and size of the columns sampled across an area: every block, or
/// every 2, 4, ... blocks for a large one, aligned so a 64-block cell holds whole steps.
fn sample_grid(world: &XZBBox) -> (i32, i32, i32, usize, usize) {
    let blocks = (i64::from(world.max_x() - world.min_x()) + 1)
        * (i64::from(world.max_z() - world.min_z()) + 1);
    let mut step = 1i32;
    while step < 16 && blocks as u64 / (step * step) as u64 > MAX_SAMPLES {
        step *= 2;
    }
    let x0 = world.min_x().div_euclid(step) * step;
    let z0 = world.min_z().div_euclid(step) * step;
    let w = ((world.max_x() - x0) / step + 1) as usize;
    let h = ((world.max_z() - z0) / step + 1) as usize;
    (x0, z0, step, w, h)
}

const LAND: u8 = 0;
const WATER: u8 = 1;
const OUTSIDE: u8 = 2;
const UNREACHED: u8 = u8::MAX;

/// The area's columns, sampled every `step` blocks from a corner aligned to `step`.
struct Grid {
    x0: i32,
    z0: i32,
    /// The area's edges, which a coarse grid's outer samples reach past.
    x_min: i32,
    z_min: i32,
    x_max: i32,
    z_max: i32,
    step: i32,
    w: usize,
    h: usize,
    floor: Vec<i16>,
    kind: Vec<u8>,
}

impl Grid {
    fn sample(t: &impl Terrain, world: &XZBBox) -> Self {
        let (x0, z0, step, w, h) = sample_grid(world);
        let mut floor = vec![0i16; w * h];
        let mut kind = vec![OUTSIDE; w * h];
        floor
            .par_chunks_mut(w)
            .zip(kind.par_chunks_mut(w))
            .enumerate()
            .for_each(|(gz, (frow, krow))| {
                for gx in 0..w {
                    let x = x0 + gx as i32 * step + step / 2;
                    let z = z0 + gz as i32 * step + step / 2;
                    let clamp = |y: i32| y.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                    (frow[gx], krow[gx]) = match t.column(x, z) {
                        Column::Land(y) => (clamp(y), LAND),
                        Column::Water(y) => (clamp(y), WATER),
                        Column::Outside => (0, OUTSIDE),
                    };
                }
            });
        let mut grid = Self {
            x0,
            z0,
            x_min: world.min_x(),
            z_min: world.min_z(),
            x_max: world.max_x(),
            z_max: world.max_z(),
            step,
            w,
            h,
            floor,
            kind,
        };
        grid.drop_perched_water();
        grid
    }

    /// Water standing more than a block over land right beside it would have run off, so it
    /// is not water the world has there: a bridge deck or a building over a canal, as the
    /// heights see it. It counts as ground at that height.
    fn drop_perched_water(&mut self) {
        let r = (3 / self.step).max(1) as usize;
        let (w, h) = (self.w, self.h);
        let land: Vec<i16> = self
            .floor
            .iter()
            .zip(&self.kind)
            .map(|(&f, &k)| if k == LAND { f } else { i16::MAX })
            .collect();
        let mut rows = vec![i16::MAX; w * h];
        rows.par_chunks_mut(w).enumerate().for_each(|(z, row)| {
            for (x, cell) in row.iter_mut().enumerate() {
                *cell = (x.saturating_sub(r)..(x + r + 1).min(w))
                    .map(|i| land[z * w + i])
                    .min()
                    .unwrap_or(i16::MAX);
            }
        });
        let perched: Vec<usize> = (0..w * h)
            .into_par_iter()
            .filter(|&i| {
                if self.kind[i] != WATER {
                    return false;
                }
                let (x, z) = (i % w, i / w);
                let low = (z.saturating_sub(r)..(z + r + 1).min(h))
                    .map(|j| rows[j * w + x])
                    .min()
                    .unwrap_or(i16::MAX);
                low != i16::MAX && i32::from(self.floor[i]) > i32::from(low) + 1
            })
            .collect();
        for i in perched {
            self.kind[i] = LAND;
        }
    }

    fn neighbours(&self, i: usize) -> impl Iterator<Item = usize> {
        let (w, h) = (self.w, self.h);
        let (x, z) = (i % w, i / w);
        [
            (x > 0).then(|| i - 1),
            (x + 1 < w).then(|| i + 1),
            (z > 0).then(|| i - w),
            (z + 1 < h).then(|| i + w),
        ]
        .into_iter()
        .flatten()
    }

    /// The water a flood rises from: bodies of it large enough, or running off the area.
    /// With `sea_y`, only water about level with the sea, so a tsunami comes from the sea.
    fn sources(&self, sea_y: Option<i32>) -> Vec<bool> {
        let low = |i: usize| {
            self.kind[i] == WATER
                && sea_y.is_none_or(|y| (y - 3..=y + 1).contains(&i32::from(self.floor[i])))
        };
        let min_area = if sea_y.is_some() {
            SEA_AREA
        } else {
            RIVER_AREA
        };
        let mut found = vec![false; self.kind.len()];
        let mut seen = vec![false; self.kind.len()];
        let mut queue = VecDeque::new();
        let mut members = Vec::new();
        for start in 0..self.kind.len() {
            if seen[start] || !low(start) {
                continue;
            }
            seen[start] = true;
            queue.push_back(start);
            members.clear();
            let mut edge = false;
            while let Some(i) = queue.pop_front() {
                members.push(i);
                let (x, z) = (i % self.w, i / self.w);
                edge |= x == 0 || z == 0 || x + 1 == self.w || z + 1 == self.h;
                for n in self.neighbours(i) {
                    if !seen[n] && low(n) {
                        seen[n] = true;
                        queue.push_back(n);
                    }
                }
            }
            let area = members.len() as i64 * i64::from(self.step * self.step);
            if edge || area >= min_area {
                for &i in &members {
                    found[i] = true;
                }
            }
        }
        found
    }

    /// For every column, the lowest level at which water from a source reaches it, and the
    /// surface of the source it comes from. Water spills over the lowest ground on its way,
    /// so land behind a ridge stays dry until the ridge is under.
    fn spread(&self, sources: &[bool], levels: u8) -> (Vec<u8>, Vec<i16>) {
        let n = self.kind.len();
        let mut need = vec![UNREACHED; n];
        let mut src = vec![0i16; n];
        let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); usize::from(levels) + 1];
        for i in (0..n).filter(|&i| sources[i]) {
            need[i] = 0;
            src[i] = self.floor[i];
            buckets[0].push(i);
        }
        for b in 0..=levels {
            while let Some(i) = buckets[usize::from(b)].pop() {
                if need[i] != b {
                    continue;
                }
                for j in self.neighbours(i) {
                    let floor = i32::from(self.floor[j]);
                    if self.kind[j] == OUTSIDE || floor < i32::from(src[i]) - MAX_DROP {
                        continue;
                    }
                    let rise = floor - i32::from(src[i]) + 1;
                    let k = rise.max(i32::from(b));
                    if k <= i32::from(levels) && (k as u8) < need[j] {
                        need[j] = k as u8;
                        src[j] = src[i];
                        buckets[k as usize].push(j);
                    }
                }
            }
        }
        (need, src)
    }

    /// The columns a map up to `levels` fills: the land it reaches, and the water near that
    /// land, which rises with it. Open water further out is left at rest.
    fn wet(&self, need: &[u8], levels: u8) -> Vec<bool> {
        let n = self.kind.len();
        let mut wet = vec![false; n];
        let mut reach = vec![u16::MAX; n];
        let mut queue = VecDeque::new();
        for i in 0..n {
            if self.kind[i] == LAND && need[i] <= levels {
                wet[i] = true;
                reach[i] = 0;
                queue.push_back(i);
            }
        }
        let max = (SHORE_REACH / self.step).max(1) as u16;
        while let Some(i) = queue.pop_front() {
            if reach[i] >= max {
                continue;
            }
            for j in self.neighbours(i) {
                if self.kind[j] == WATER && need[j] <= levels && reach[j] == u16::MAX {
                    reach[j] = reach[i] + 1;
                    wet[j] = true;
                    queue.push_back(j);
                }
            }
        }
        wet
    }

    /// The fills of each cell, merged into rectangles of columns that fill alike.
    fn cells(&self, need: &[u8], src: &[i16], levels: u8) -> Vec<((i32, i32), Vec<Rect>)> {
        let wet = self.wet(need, levels);
        let per = (CELL / self.step) as usize;
        let cell_of = |g: i32, origin: i32| (origin + g * self.step).div_euclid(CELL);
        let (cx0, cz0) = (cell_of(0, self.x0), cell_of(0, self.z0));
        let (cx1, cz1) = (
            cell_of(self.w as i32 - 1, self.x0),
            cell_of(self.h as i32 - 1, self.z0),
        );
        let mut out = Vec::new();
        let mut used = vec![false; per * per];
        for cz in cz0..=cz1 {
            for cx in cx0..=cx1 {
                // The grid columns of this cell; the grid is aligned so none straddle it.
                let gx0 = (cx * CELL - self.x0) / self.step;
                let gz0 = (cz * CELL - self.z0) / self.step;
                let at = |lx: usize, lz: usize| -> Option<Fill> {
                    let (gx, gz) = (gx0 + lx as i32, gz0 + lz as i32);
                    if gx < 0 || gz < 0 || gx as usize >= self.w || gz as usize >= self.h {
                        return None;
                    }
                    let i = gz as usize * self.w + gx as usize;
                    if wet[i] {
                        Some(Fill::of(self.kind[i], self.floor[i], src[i], need[i]))
                    } else {
                        (self.kind[i] == LAND).then(|| Fill::dry(self.floor[i]))
                    }
                };
                used.iter_mut().for_each(|u| *u = false);
                let mut rects = Vec::new();
                for lz in 0..per {
                    for lx in 0..per {
                        if used[lz * per + lx] {
                            continue;
                        }
                        let Some(first) = at(lx, lz).filter(|f| !f.dry) else {
                            continue;
                        };
                        let mut merged = Merged::new(first, levels);
                        let mut x_end = lx + 1;
                        while x_end < per && !used[lz * per + x_end] {
                            match at(x_end, lz).filter(|f| merged.takes(&[*f])) {
                                Some(f) => merged.add(&[f]),
                                None => break,
                            }
                            x_end += 1;
                        }
                        let mut z_end = lz + 1;
                        while z_end < per {
                            let row: Option<Vec<Fill>> = (lx..x_end)
                                .map(|x| at(x, z_end).filter(|_| !used[z_end * per + x]))
                                .collect();
                            match row.filter(|r| merged.takes(r)) {
                                Some(r) => merged.add(&r),
                                None => break,
                            }
                            z_end += 1;
                        }
                        for z in lz..z_end {
                            for x in lx..x_end {
                                used[z * per + x] = true;
                            }
                        }
                        let bx = |l: usize| self.x0 + (gx0 + l as i32) * self.step;
                        let bz = |l: usize| self.z0 + (gz0 + l as i32) * self.step;
                        let rect = Rect {
                            x0: bx(lx).max(self.x_min),
                            z0: bz(lz).max(self.z_min),
                            x1: (bx(x_end) - 1).min(self.x_max),
                            z1: (bz(z_end) - 1).min(self.z_max),
                            lo: merged.lo_min,
                            src: i32::from(first.src),
                            need: merged.need,
                        };
                        split(rect, rect.src + i32::from(levels), &mut rects);
                    }
                }
                if !rects.is_empty() {
                    out.push(((cx, cz), rects));
                }
            }
        }
        out
    }
}

/// How one column fills: the water goes in from `lo` up, once it is `need` levels above `src`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fill {
    lo: i32,
    src: i16,
    need: u8,
    /// Land that a ridge keeps dry past the level its own height would flood at.
    held: bool,
    water: bool,
    /// Land the map never reaches. A rectangle may still take it in where it stands above
    /// every level: its fill stops at the ground, or floods the rooms of a building, which
    /// the water would get into anyway.
    dry: bool,
}

impl Fill {
    fn of(kind: u8, floor: i16, src: i16, need: u8) -> Self {
        let lo = i32::from(floor) + 1;
        if kind == WATER {
            return Self {
                lo,
                src,
                need,
                held: false,
                water: true,
                dry: false,
            };
        }
        // Land the water reaches before the first level still floods at it.
        let own = (lo - i32::from(src)).max(1);
        let need = need.max(1);
        Self {
            lo,
            src,
            need,
            held: i32::from(need) > own,
            water: false,
            dry: false,
        }
    }

    fn dry(floor: i16) -> Self {
        let lo = i32::from(floor) + 1;
        Self {
            lo,
            src: 0,
            need: u8::MAX,
            held: false,
            water: false,
            dry: true,
        }
    }
}

/// Columns merged into one rectangle so far. Land that its own height keeps dry until a
/// level goes in with lower land at that level's fill: the fill only replaces air, so it
/// stops at the ground. Columns a ridge holds back merge only with their own kind, and a
/// fill reaches no deeper below any column's ground than `LO_SPREAD`.
struct Merged {
    first: Fill,
    /// The highest the water gets over these columns.
    top: i32,
    lo_min: i32,
    lo_max: i32,
    need: u8,
}

const LO_SPREAD: i32 = 3;

impl Merged {
    fn new(first: Fill, levels: u8) -> Self {
        Self {
            first,
            top: i32::from(first.src) + i32::from(levels),
            lo_min: first.lo,
            lo_max: first.lo,
            need: first.need,
        }
    }

    fn takes(&self, fills: &[Fill]) -> bool {
        let (mut lo_min, mut lo_max) = (self.lo_min, self.lo_max);
        for f in fills {
            if f.dry {
                if f.lo <= self.top {
                    return false;
                }
                continue;
            }
            let alike = f.src == self.first.src
                && f.water == self.first.water
                && f.held == self.first.held
                && (!f.held || f.need == self.first.need);
            if !alike {
                return false;
            }
            lo_min = lo_min.min(f.lo);
            lo_max = lo_max.max(f.lo);
        }
        lo_max - lo_min <= LO_SPREAD
    }

    fn add(&mut self, fills: &[Fill]) {
        for f in fills.iter().filter(|f| !f.dry) {
            self.lo_min = self.lo_min.min(f.lo);
            self.lo_max = self.lo_max.max(f.lo);
            self.need = self.need.min(f.need);
        }
    }
}

/// A box of columns that fill alike: from `lo` up to `levels` above `src`, once the water
/// is at least `need` levels up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x0: i32,
    z0: i32,
    x1: i32,
    z1: i32,
    lo: i32,
    src: i32,
    need: u8,
}

/// Splits a rectangle until a fill of it up to `top` stays within one fill's limit.
fn split(r: Rect, top: i32, out: &mut Vec<Rect>) {
    let height = i64::from((top - r.lo + 1).max(1));
    let (dx, dz) = (r.x1 - r.x0, r.z1 - r.z0);
    let area = (i64::from(dx) + 1) * (i64::from(dz) + 1);
    if area * height <= MAX_FILL_VOLUME || (dx == 0 && dz == 0) {
        out.push(r);
    } else if dx >= dz {
        let mid = r.x0 + dx / 2;
        split(Rect { x1: mid, ..r }, top, out);
        split(Rect { x0: mid + 1, ..r }, top, out);
    } else {
        let mid = r.z0 + dz / 2;
        split(Rect { z1: mid, ..r }, top, out);
        split(Rect { z0: mid + 1, ..r }, top, out);
    }
}

/// The fills that raise the water around an area's rivers, lakes or sea, level by level.
struct WaterMap {
    levels: u8,
    cells: Vec<((i32, i32), Vec<Rect>)>,
}

impl WaterMap {
    #[cfg(test)]
    fn build(t: &impl Terrain, world: &XZBBox, levels: u8, sea_y: Option<i32>) -> Option<Self> {
        Self::from_grid(&Grid::sample(t, world), levels, sea_y)
    }

    /// From every body of water, or from the sea alone when `sea_y` is given. Gives up its
    /// highest levels when the fills would make too large a pack, and is `None` when nothing
    /// would flood.
    fn from_grid(grid: &Grid, levels: u8, sea_y: Option<i32>) -> Option<Self> {
        let sources = grid.sources(sea_y);
        if !sources.iter().any(|&s| s) {
            return None;
        }
        let (need, src) = grid.spread(&sources, levels);
        let mut levels = levels;
        loop {
            let cells = grid.cells(&need, &src, levels);
            let fills: usize = cells.iter().map(|(_, r)| r.len()).sum();
            if fills == 0 {
                return None;
            }
            if fills <= FILL_BUDGET {
                return Some(Self { levels, cells });
            }
            if levels <= 2 {
                return None;
            }
            levels -= 1;
        }
    }

    fn fill_count(&self) -> usize {
        self.cells.iter().map(|(_, r)| r.len()).sum()
    }

    /// The function that sets the top of the water above each source for the current level.
    /// Also the block above that top, where lowering the water starts.
    fn tops(&self, map: &str) -> String {
        let surfaces: BTreeSet<i32> = self
            .cells
            .iter()
            .flat_map(|(_, rects)| rects.iter().map(|r| r.src))
            .collect();
        let mut out = String::new();
        for s in surfaces {
            let _ = writeln!(out, "scoreboard players set #top arnis.ds {s}");
            let _ = writeln!(
                out,
                "scoreboard players operation #top arnis.ds += #lv_{map} arnis.ds"
            );
            let _ = writeln!(
                out,
                "execute store result storage arnis:ds {map}.t.{} int 1 run scoreboard players get #top arnis.ds",
                top_key(s)
            );
            let _ = writeln!(
                out,
                "execute store result storage arnis:ds {map}.t.{} int 1 run scoreboard players add #top arnis.ds 1",
                above_key(s)
            );
        }
        out
    }

    /// Per cell, the function that fills it up to the current level and the one that takes
    /// it down to it: off the top where the level still reaches, all of it where not.
    fn cell_functions(&self, map: &str) -> Vec<(String, String)> {
        let mut out = Vec::with_capacity(self.cells.len() * 2);
        for ((cx, cz), rects) in &self.cells {
            let mut fill = String::new();
            let mut drain = String::new();
            for r in rects {
                let reach = format!(
                    "fill {} {} {} {} $({}) {} {FLOOD_WATER} replace #arnis:floodable strict",
                    r.x0,
                    r.lo,
                    r.z0,
                    r.x1,
                    top_key(r.src),
                    r.z1
                );
                if r.need <= 1 {
                    let _ = writeln!(fill, "${reach}");
                } else {
                    let _ = writeln!(
                        fill,
                        "$execute if score #lv_{map} arnis.ds matches {}.. run {reach}",
                        r.need
                    );
                }
                let (top, need) = (r.src + i32::from(self.levels), r.need.max(1));
                let above = format!(
                    "fill {} $({}) {} {} {top} {} minecraft:air replace {FLOOD_DRAIN}",
                    r.x0,
                    above_key(r.src),
                    r.z0,
                    r.x1,
                    r.z1
                );
                // Below the level it floods at, the top of the water starts at or under the
                // ground, so the same fill takes all of it. Not so where a ridge held it back.
                if r.lo >= r.src + i32::from(need) {
                    let _ = writeln!(drain, "${above}");
                    continue;
                }
                let _ = writeln!(
                    drain,
                    "$execute if score #lv_{map} arnis.ds matches {need}.. run {above}"
                );
                let _ = writeln!(
                    drain,
                    "execute unless score #lv_{map} arnis.ds matches {need}.. run fill {} {} {} {} {top} {} minecraft:air replace {FLOOD_DRAIN}",
                    r.x0,
                    r.lo,
                    r.z0,
                    r.x1,
                    r.z1
                );
            }
            out.push((format!("{DIR}/{map}/c/{cx}_{cz}.mcfunction"), fill));
            out.push((format!("{DIR}/{map}/d/{cx}_{cz}.mcfunction"), drain));
        }
        out
    }
}

/// The storage key for the top of the water over a source at `surface`.
fn top_key(surface: i32) -> String {
    key("t", surface)
}

/// The storage key for the block above that top.
fn above_key(surface: i32) -> String {
    key("a", surface)
}

fn key(prefix: &str, surface: i32) -> String {
    if surface < 0 {
        format!("{prefix}m{}", -surface)
    } else {
        format!("{prefix}{surface}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A river along z = 0..4 at y 60, with land that climbs away from it a block every
    /// four blocks.
    struct Slope;

    impl Terrain for Slope {
        fn column(&self, _x: i32, z: i32) -> Column {
            if z < 5 {
                Column::Water(60)
            } else {
                Column::Land(60 + (z - 5) / 4)
            }
        }
    }

    /// The same river with flat land at 61 beside it, and a hollow down at `floor` inside a
    /// ring of ground at 62.
    struct Dike {
        floor: i32,
    }

    impl Terrain for Dike {
        fn column(&self, x: i32, z: i32) -> Column {
            if z < 5 {
                Column::Water(60)
            } else if (25..=95).contains(&x) && (10..=90).contains(&z) {
                Column::Land(self.floor)
            } else if (24..=96).contains(&x) && (9..=91).contains(&z) {
                Column::Land(62)
            } else {
                Column::Land(61)
            }
        }
    }

    fn world(w: i32, h: i32) -> XZBBox {
        XZBBox::rect_from_min_max(0, 0, w - 1, h - 1).unwrap()
    }

    fn rect_at(map: &WaterMap, x: i32, z: i32) -> Option<Rect> {
        map.cells
            .iter()
            .flat_map(|(_, r)| r.iter())
            .find(|r| (r.x0..=r.x1).contains(&x) && (r.z0..=r.z1).contains(&z))
            .copied()
    }

    /// The top of the water in a column once the map stands at `level`, as the fills place
    /// it: only over the ground, which a fill never replaces.
    fn water_top(map: &WaterMap, t: &impl Terrain, x: i32, z: i32, level: u8) -> Option<i32> {
        let ground = match t.column(x, z) {
            Column::Land(y) | Column::Water(y) => y,
            Column::Outside => return None,
        };
        map.cells
            .iter()
            .flat_map(|(_, r)| r.iter())
            .filter(|r| r.need <= level && (r.x0..=r.x1).contains(&x) && (r.z0..=r.z1).contains(&z))
            .map(|r| r.src + i32::from(level))
            .filter(|&top| top > ground)
            .max()
    }

    #[test]
    fn land_floods_a_level_for_every_block_it_stands_above_the_river() {
        let map = WaterMap::build(&Slope, &world(16, 64), 3, None).unwrap();
        // Ground at 60 next to the river is under at the first level, 61 at the second.
        assert_eq!(water_top(&map, &Slope, 3, 6, 1), Some(61));
        assert_eq!(water_top(&map, &Slope, 3, 10, 1), None);
        assert_eq!(water_top(&map, &Slope, 3, 10, 2), Some(62));
        // 63 is above the third level.
        assert_eq!(water_top(&map, &Slope, 3, 5 + 4 * 3, 3), None);
        // The river itself rises from its surface.
        assert_eq!(rect_at(&map, 3, 2).unwrap().lo, 61);
    }

    #[test]
    fn a_hollow_behind_a_dike_stays_dry_until_the_dike_is_over() {
        let dike = Dike { floor: 59 };
        let map = WaterMap::build(&dike, &world(128, 128), 4, None).unwrap();
        assert_eq!(water_top(&map, &dike, 10, 10, 1), None);
        assert_eq!(water_top(&map, &dike, 10, 10, 2), Some(62));
        // The hollow lies below the river, but the water has to get over 62 first.
        assert_eq!(water_top(&map, &dike, 50, 50, 2), None);
        assert_eq!(water_top(&map, &dike, 50, 50, 3), Some(63));
        assert_eq!(rect_at(&map, 50, 50).unwrap().lo, 60);
    }

    #[test]
    fn every_fill_stays_within_the_block_limit() {
        // Uneven land just around the river's level, under seven levels of water.
        struct Bumpy;
        impl Terrain for Bumpy {
            fn column(&self, x: i32, z: i32) -> Column {
                if z < 5 {
                    Column::Water(60)
                } else {
                    Column::Land(58 + (x + z) % 4)
                }
            }
        }
        let map = WaterMap::build(&Bumpy, &world(200, 200), 7, None).unwrap();
        assert!(rect_at(&map, 50, 50).is_some());
        let mut split = false;
        for (_, rects) in &map.cells {
            for r in rects {
                let area = i64::from(r.x1 - r.x0 + 1) * i64::from(r.z1 - r.z0 + 1);
                let height = i64::from(r.src + i32::from(map.levels) - r.lo + 1);
                assert!(area * height <= MAX_FILL_VOLUME, "{r:?}");
                assert_eq!(r.x0.div_euclid(CELL), r.x1.div_euclid(CELL), "{r:?}");
                split |= area < 64 * 59;
            }
        }
        assert!(split);
    }

    #[test]
    fn a_canal_above_the_town_floods_its_quays_and_not_the_town() {
        // A canal at 70 between quays at 71, above a town at 60 by a river at 59.
        struct Town;
        impl Terrain for Town {
            fn column(&self, x: i32, z: i32) -> Column {
                match (x, z) {
                    (_, z) if z < 5 => Column::Water(59),
                    (30..=33, 20..=63) => Column::Water(70),
                    (28..=35, 18..=63) => Column::Land(71),
                    _ => Column::Land(60),
                }
            }
        }
        let map = WaterMap::build(&Town, &world(64, 64), 3, None).unwrap();
        assert_eq!(water_top(&map, &Town, 10, 40, 1), None);
        assert_eq!(water_top(&map, &Town, 10, 40, 2), Some(61));
        assert_eq!(water_top(&map, &Town, 29, 40, 2), Some(72));
    }

    #[test]
    fn a_snowstorm_swaps_land_biomes_for_snowy_ones_the_world_does_not_have() {
        let seen: HashMap<&'static str, u64> = [
            ("minecraft:plains", 900),
            ("minecraft:forest", 400),
            ("minecraft:taiga", 300),
            ("minecraft:river", 200),
            ("minecraft:snowy_slopes", 50),
            ("minecraft:ocean", 20),
        ]
        .into_iter()
        .collect();
        let swaps = snow_swaps(&seen);
        assert_eq!(swaps[0], ("minecraft:plains", "minecraft:snowy_plains"));
        assert_eq!(swaps[1], ("minecraft:forest", "minecraft:snowy_taiga"));
        // Its own counterpart is taken, so it gets another free one.
        assert_eq!(swaps[2].0, "minecraft:taiga");
        assert!(SNOW_BIOMES.contains(&swaps[2].1));
        assert_eq!(swaps.len(), 3);
        let to: BTreeSet<&str> = swaps.iter().map(|s| s.1).collect();
        assert_eq!(to.len(), swaps.len());
        assert!(swaps.iter().all(|(_, s)| !seen.contains_key(s)));
    }

    #[test]
    fn sinkhole_rings_make_a_round_hole_without_dropping_a_column_twice() {
        let mut dropped = std::collections::HashMap::new();
        for r in 0..=SINK_RINGS {
            for line in sink_ring(r).lines().filter(|l| l.starts_with("$clone")) {
                let n: Vec<i32> = line
                    .split_whitespace()
                    .skip(1)
                    .take(6)
                    .map(|v| v.trim_start_matches('~').parse().unwrap())
                    .collect();
                for dx in n[0]..=n[3] {
                    *dropped.entry((dx, n[2])).or_insert(0) += 1;
                }
            }
        }
        assert!(dropped.values().all(|&n| n == 1));
        let edge = f64::from(SINK_RINGS) + 0.5;
        for dx in -SINK_RINGS - 1..=SINK_RINGS + 1 {
            for dz in -SINK_RINGS - 1..=SINK_RINGS + 1 {
                let inside = f64::from(dx * dx + dz * dz).sqrt() < edge;
                assert_eq!(dropped.contains_key(&(dx, dz)), inside, "{dx} {dz}");
            }
        }
    }

    #[test]
    fn cell_functions_fill_up_to_the_current_level_and_take_it_back_down() {
        let map = WaterMap::build(&Dike { floor: 59 }, &world(64, 64), 3, None).unwrap();
        let files = map.cell_functions("flood");
        let (path, fill) = &files[0];
        assert!(path.ends_with("flood/c/0_0.mcfunction"), "{path}");
        assert!(fill.lines().all(|l| l.starts_with('$')));
        assert!(fill.contains("$(t60) "));
        assert!(fill.contains("if score #lv_flood arnis.ds matches 3.. run fill"));
        let drain = &files[1].1;
        assert!(drain
            .lines()
            .all(|l| l.starts_with('$') == l.contains("$(")));
        assert!(drain.contains("matches 3.. run fill 25 $(a60) "));
        assert!(drain.contains("unless score #lv_flood arnis.ds matches 3.. run fill 25 60 "));
        // Outside the dike the ground floods at its own height, so one fill does.
        assert!(drain
            .lines()
            .any(|l| l.starts_with("$fill ") && l.contains("$(a60)")));
        assert!(
            drain.contains(" 63 ")
                && drain.contains("minecraft:air replace minecraft:light[level=0]")
        );
        let tops = map.tops("flood");
        assert!(tops.contains("storage arnis:ds flood.t.t60 int 1"));
        assert!(tops.contains("storage arnis:ds flood.t.a60 int 1 run scoreboard players add #top"));
        // Its own scratch score: #t is the clock of the disaster that is running.
        assert!(!tops.contains("#t arnis.ds"));
    }

    #[test]
    fn only_water_at_sea_level_feeds_a_tsunami() {
        struct Lake;
        impl Terrain for Lake {
            fn column(&self, _x: i32, z: i32) -> Column {
                if z < 5 {
                    Column::Water(90)
                } else {
                    Column::Land(91)
                }
            }
        }
        assert!(WaterMap::build(&Lake, &world(16, 32), 5, Some(62)).is_none());
        assert!(WaterMap::build(&Lake, &world(16, 32), 5, None).is_some());
    }

    #[test]
    fn water_perched_over_the_land_beside_it_is_no_source() {
        // Beside the river, a patch the heights call water at 70, on a bridge over land at 61:
        // it must not pour down onto that land.
        struct Bridge;
        impl Terrain for Bridge {
            fn column(&self, x: i32, z: i32) -> Column {
                if z < 5 {
                    Column::Water(60)
                } else if (20..24).contains(&x) && (20..24).contains(&z) {
                    Column::Water(70)
                } else {
                    Column::Land(61)
                }
            }
        }
        let map = WaterMap::build(&Bridge, &world(64, 64), 5, None).unwrap();
        assert_eq!(water_top(&map, &Bridge, 30, 30, 1), None);
        assert_eq!(water_top(&map, &Bridge, 30, 30, 2), Some(62));
        assert_eq!(water_top(&map, &Bridge, 21, 21, 5), None);
    }

    #[test]
    fn snow_reaches_above_the_tallest_roof_in_a_tall_world() {
        let mut plan = Plan::with_maps(CelestialBody::Earth, false, None, None, -1000);
        plan.snow = vec![("minecraft:plains", "minecraft:snowy_plains")];
        plan.snow_top = 500;
        let (swap, thaw) = plan.snow_functions();
        let spans: Vec<(i32, i32)> = swap
            .lines()
            .map(|l| {
                let n: Vec<&str> = l.split_whitespace().collect();
                (n[2].parse().unwrap(), n[5].parse().unwrap())
            })
            .collect();
        assert_eq!(
            spans,
            [(-1000, -617), (-616, -233), (-232, 151), (152, 500)]
        );
        assert!(thaw
            .lines()
            .all(|l| l.ends_with("replace minecraft:snowy_plains")));
    }

    #[test]
    fn water_rests_on_stairs_and_blocks_and_passes_air_and_plants() {
        for solid in [
            "minecraft:oak_stairs",
            "minecraft:stone_brick_stairs",
            "minecraft:grass_block",
        ] {
            assert_eq!(surface_class(solid), SOLID, "{solid}");
        }
        for pass in [
            "minecraft:air",
            "minecraft:cave_air",
            "minecraft:short_grass",
            "minecraft:oak_fence",
        ] {
            assert_eq!(surface_class(pass), PASS, "{pass}");
        }
        assert_eq!(surface_class("minecraft:water"), FLUID);
    }

    #[test]
    fn negative_surfaces_get_their_own_key() {
        assert_eq!(top_key(-12), "tm12");
        assert_eq!(top_key(64), "t64");
    }

    #[test]
    fn the_moon_gets_only_what_it_can_have() {
        let plan = Plan::with_maps(CelestialBody::Moon, true, None, None, -64);
        assert_eq!(
            plan.kinds,
            vec![Disaster::Meteors, Disaster::Sinkholes, Disaster::Quake]
        );
        assert_eq!(plan.coming(), "a meteor shower, sinkholes and a moonquake");
    }

    #[test]
    fn earth_without_water_or_woods_skips_those() {
        let plan = Plan::with_maps(CelestialBody::Earth, false, None, None, -64);
        assert!(!plan.kinds.contains(&Disaster::Flood));
        assert!(!plan.kinds.contains(&Disaster::Wildfire));
        assert!(plan.kinds.contains(&Disaster::Kaiju));
        assert!(plan.kinds.contains(&Disaster::Snowstorm));
        let files = plan.files("Köln \"Mitte\"");
        // Without biomes to swap, it snows only where it would anyway.
        let swap = &files
            .iter()
            .find(|(p, _)| p.ends_with("snow/swap.mcfunction"))
            .unwrap()
            .1;
        assert!(String::from_utf8_lossy(swap).starts_with('#'));
        let tops = files
            .iter()
            .find(|(p, _)| p.ends_with("flood/tops.mcfunction"));
        assert!(tops.is_some());
        let intro = &files
            .iter()
            .find(|(p, _)| p.ends_with("text/intro.mcfunction"))
            .unwrap()
            .1;
        assert!(
            String::from_utf8_lossy(intro).contains(r#"Survive eight disasters in Köln \"Mitte\""#)
        );
    }
}
