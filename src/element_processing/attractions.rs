//! Funfair and theme park rides from `attraction=*`: carousels, swing rides, star flyers,
//! Ferris wheels, drop towers, pirate ships, bumper cars, teacups and spinning rides.
//!
//! Rides mapped as buildings would come out as plain boxes, so the ride kinds drawn here
//! claim them before the building pass. Halls (dark rides, mazes, covered bumper cars)
//! stay buildings.

use crate::block_definitions::*;
use crate::element_processing::connected_blocks::place_connected;
use crate::element_processing::structure_kit::{
    axis_of, beam, concrete_for_colour, mix, place, stair_backed, with_prop, Frame,
};
use crate::floodfill_cache::{CoordinateBitmap, FloodFillCache, RoadMaskBitmap};
use crate::mapillary::geometry::parse_length_m;
use crate::osm_parser::{ProcessedElement, ProcessedNode};
use crate::world_editor::WorldEditor;
use std::collections::{HashMap, HashSet};
use std::f64::consts::{PI, TAU};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ride {
    Carousel,
    ChainSwings,
    StarFlyer,
    BigWheel,
    DropTower,
    PirateShip,
    BumperCars,
    Teacups,
    Spinner,
}

fn tag<'a>(tags: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    tags.get(key).map(String::as_str)
}

fn height_m(tags: &HashMap<String, String>) -> Option<f64> {
    tag(tags, "height")
        .and_then(parse_length_m)
        .filter(|h| *h > 1.0 && *h < 400.0)
}

/// What a name says about a ride when its tag is generic.
fn hinted(name: &str) -> Option<Ride> {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&[
        "riesenrad",
        "ferris",
        "big wheel",
        "eye",
        "grande roue",
        "ruota",
        "noria",
        "reuzenrad",
        "pariserhjul",
        "diabelski",
        "wheel",
    ]) {
        Some(Ride::BigWheel)
    } else if has(&[
        "kettenkarussell",
        "wellenflieger",
        "swing",
        "chairoplane",
        "star flyer",
        "starflyer",
        "kettenflieger",
    ]) {
        Some(Ride::ChainSwings)
    } else if has(&[
        "freifall",
        "free fall",
        "freefall",
        "drop",
        "fallturm",
        "power tower",
    ]) {
        Some(Ride::DropTower)
    } else if has(&[
        "schiffschaukel",
        "pirat",
        "viking",
        "galeone",
        "swinging ship",
    ]) {
        Some(Ride::PirateShip)
    } else if has(&[
        "autodrom",
        "autoscooter",
        "bumper",
        "dodgem",
        "radiobil",
        "skooter",
    ]) {
        Some(Ride::BumperCars)
    } else if has(&["teacup", "tea cup", "tasse", "kopjes", "kaffeetass"]) {
        Some(Ride::Teacups)
    } else if has(&[
        "karussell",
        "carousel",
        "merry",
        "carrousel",
        "manège",
        "galopper",
    ]) {
        Some(Ride::Carousel)
    } else if has(&["tower", "turm", "tårn", "torre", "tour "]) {
        Some(Ride::DropTower)
    } else {
        None
    }
}

fn ride_kind(tags: &HashMap<String, String>) -> Option<Ride> {
    let attraction = tag(tags, "attraction")?;
    let building = tags.contains_key("building") || tags.contains_key("building:part");
    let tower = tag(tags, "man_made") == Some("tower");
    let name = tag(tags, "name").unwrap_or("");
    let tall = height_m(tags).is_some_and(|h| h >= 30.0);
    let ride = match attraction {
        "carousel" => hinted(name)
            .filter(|r| *r == Ride::ChainSwings || *r == Ride::Teacups)
            .unwrap_or(Ride::Carousel),
        "swing_carousel" | "swing_ride" => Ride::ChainSwings,
        "big_wheel" => Ride::BigWheel,
        "drop_tower" => Ride::DropTower,
        "pirate_ship" | "swinging_ship" | "dragon_swing" | "swinging_boat" => Ride::PirateShip,
        "teacup" => Ride::Teacups,
        "bumper_car" if !building => Ride::BumperCars,
        "amusement_ride" | "kiddie_ride" => match hinted(name) {
            Some(r) => r,
            None if tower => Ride::DropTower,
            None if building => return None,
            None => Ride::Spinner,
        },
        _ => return None,
    };
    // A ride under a hall roof stays a building.
    if building && matches!(ride, Ride::BumperCars | Ride::Spinner | Ride::Teacups) {
        return None;
    }
    // A generic ride drawn as some other area keeps that look.
    if ride == Ride::Spinner
        && [
            "leisure", "landuse", "natural", "amenity", "highway", "waterway",
        ]
        .iter()
        .any(|k| tags.contains_key(*k))
    {
        return None;
    }
    Some(match ride {
        Ride::ChainSwings if tall || tower => Ride::StarFlyer,
        r => r,
    })
}

/// True for rides drawn here, including those mapped as buildings.
pub fn claims(tags: &HashMap<String, String>) -> bool {
    ride_kind(tags).is_some()
}

/// Where a ride stands and how big its mapped outline is.
#[derive(Clone, Debug)]
struct Site {
    cx: f64,
    cz: f64,
    /// Unit vector of the outline's long axis.
    axis: (f64, f64),
    /// Full extent along and across the long axis, 0 for a node.
    long: f64,
    short: f64,
    /// Cells of the mapped outline, empty for a node.
    cells: Vec<(i32, i32)>,
}

impl Site {
    fn radius(&self, default: f64) -> f64 {
        if self.long > 0.0 {
            (self.long + self.short) / 4.0
        } else {
            default
        }
    }
}

fn site_of(
    element: &ProcessedElement,
    flood_fill_cache: &FloodFillCache,
    timeout: Option<&Duration>,
) -> Option<Site> {
    let nodes: Vec<&ProcessedNode> = element.nodes().collect();
    let first = nodes.first()?;
    if nodes.len() == 1 {
        return Some(Site {
            cx: first.x as f64 + 0.5,
            cz: first.z as f64 + 0.5,
            axis: (1.0, 0.0),
            long: 0.0,
            short: 0.0,
            cells: Vec::new(),
        });
    }
    let pts: Vec<(f64, f64)> = nodes
        .iter()
        .map(|n| (n.x as f64 + 0.5, n.z as f64 + 0.5))
        .collect();
    let n = pts.len() as f64;
    let (mx, mz) = (
        pts.iter().map(|p| p.0).sum::<f64>() / n,
        pts.iter().map(|p| p.1).sum::<f64>() / n,
    );
    // Long axis from the vertex spread.
    let (mut sxx, mut szz, mut sxz) = (0.0, 0.0, 0.0);
    for &(x, z) in &pts {
        sxx += (x - mx) * (x - mx);
        szz += (z - mz) * (z - mz);
        sxz += (x - mx) * (z - mz);
    }
    let angle = 0.5 * (2.0 * sxz).atan2(sxx - szz);
    let axis = (angle.cos(), angle.sin());
    let along = |p: &(f64, f64)| (p.0 - mx) * axis.0 + (p.1 - mz) * axis.1;
    let across = |p: &(f64, f64)| -(p.0 - mx) * axis.1 + (p.1 - mz) * axis.0;
    let (a0, a1) = pts
        .iter()
        .map(along)
        .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    let (b0, b1) = pts
        .iter()
        .map(across)
        .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    let cells = match element {
        ProcessedElement::Way(way)
            if way.nodes.len() > 3
                && way.nodes.first().map(|n| n.id) == way.nodes.last().map(|n| n.id) =>
        {
            flood_fill_cache
                .get_or_compute(way, timeout)
                .iter()
                .copied()
                .collect()
        }
        _ => Vec::new(),
    };
    // Centre of the outline's box, not the vertex mean, which leans to the busy side.
    let (ca, cb) = ((a0 + a1) / 2.0, (b0 + b1) / 2.0);
    Some(Site {
        cx: mx + ca * axis.0 - cb * axis.1,
        cz: mz + ca * axis.1 + cb * axis.0,
        axis,
        long: a1 - a0,
        short: b1 - b0,
        cells,
    })
}

/// Bright ride colours; `colour` tags win.
fn ride_colours(tags: &HashMap<String, String>, seed: u64) -> (Block, Block) {
    const MAIN: [Block; 9] = [
        RED_CONCRETE,
        YELLOW_CONCRETE,
        BLUE_CONCRETE,
        LIGHT_BLUE_CONCRETE,
        PINK_CONCRETE,
        ORANGE_CONCRETE,
        LIME_CONCRETE,
        PURPLE_CONCRETE,
        MAGENTA_CONCRETE,
    ];
    let main = tag(tags, "colour")
        .or_else(|| tag(tags, "building:colour"))
        .and_then(concrete_for_colour)
        .unwrap_or(MAIN[(seed % MAIN.len() as u64) as usize]);
    let accent = match main {
        YELLOW_CONCRETE | WHITE_CONCRETE | ORANGE_CONCRETE => RED_CONCRETE,
        _ => WHITE_CONCRETE,
    };
    (main, accent)
}

const RAINBOW: [Block; 8] = [
    RED_CONCRETE,
    ORANGE_CONCRETE,
    YELLOW_CONCRETE,
    LIME_CONCRETE,
    LIGHT_BLUE_CONCRETE,
    BLUE_CONCRETE,
    PURPLE_CONCRETE,
    PINK_CONCRETE,
];

/// Ground under the ride, from the terrain so every tile agrees.
fn ground_at(editor: &WorldEditor, x: i32, z: i32) -> i32 {
    editor
        .terrain_level(x, z)
        .unwrap_or_else(|| editor.get_ground_level(x, z))
}

/// Paves and levels the ground under a ride; returns the floor level.
fn level_ground(
    editor: &mut WorldEditor,
    cells: &[(i32, i32)],
    own: &HashSet<(i32, i32)>,
    paving: Block,
    footprints: &CoordinateBitmap,
    road_mask: &RoadMaskBitmap,
) -> i32 {
    let mut heights: Vec<i32> = cells
        .iter()
        .map(|&(x, z)| ground_at(editor, x, z))
        .collect();
    heights.sort_unstable();
    let floor = heights.get(heights.len() / 2).copied().unwrap_or(0);
    for &(x, z) in cells {
        if road_mask.contains(x, z) || (footprints.contains(x, z) && !own.contains(&(x, z))) {
            continue;
        }
        editor.register_road_surface_y(x, z, floor);
        editor.set_block_absolute(paving, x, floor, z, None, None);
    }
    floor
}

fn disc(cx: f64, cz: f64, r: f64) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    let ri = r.ceil() as i32 + 1;
    let (bx, bz) = (cx.floor() as i32, cz.floor() as i32);
    for x in bx - ri..=bx + ri {
        for z in bz - ri..=bz + ri {
            let d = ((x as f64 + 0.5 - cx).powi(2) + (z as f64 + 0.5 - cz).powi(2)).sqrt();
            if d <= r {
                out.push((x, z));
            }
        }
    }
    out
}

fn dist_to(cx: f64, cz: f64, x: i32, z: i32) -> f64 {
    ((x as f64 + 0.5 - cx).powi(2) + (z as f64 + 0.5 - cz).powi(2)).sqrt()
}

/// Angle of a cell round a centre, 0..TAU.
fn angle_of(cx: f64, cz: f64, x: i32, z: i32) -> f64 {
    (z as f64 + 0.5 - cz)
        .atan2(x as f64 + 0.5 - cx)
        .rem_euclid(TAU)
}

pub fn generate_ride(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    flood_fill_cache: &FloodFillCache,
    footprints: &CoordinateBitmap,
    road_mask: &RoadMaskBitmap,
    scale: f64,
    timeout: Option<&Duration>,
) {
    let tags = element.tags();
    let Some(ride) = ride_kind(tags) else {
        return;
    };
    let Some(site) = site_of(element, flood_fill_cache, timeout) else {
        return;
    };
    // Only a compact outline reads as one spinning ride; a big or long one is a track.
    if ride == Ride::Spinner
        && site.long > 0.0
        && (site.long > 30.0 * scale || site.short < site.long * 0.55)
    {
        return;
    }
    let seed = mix(element.id(), 0x52_49_44_45);
    let colours = ride_colours(tags, seed);
    let height = height_m(tags).map(|h| h * scale);
    let own: HashSet<(i32, i32)> = site.cells.iter().copied().collect();
    let place = Placement {
        site: &site,
        own: &own,
        colours,
        tagged: tag(tags, "colour")
            .or_else(|| tag(tags, "building:colour"))
            .and_then(concrete_for_colour),
        seed,
        height,
        footprints,
        road_mask,
    };
    match ride {
        Ride::Carousel => carousel(editor, &place),
        Ride::ChainSwings => swing_ride(editor, &place),
        Ride::StarFlyer => star_flyer(editor, &place),
        Ride::BigWheel => big_wheel(editor, &place),
        Ride::DropTower => drop_tower(editor, &place),
        Ride::PirateShip => pirate_ship(editor, &place),
        Ride::BumperCars => bumper_cars(editor, &place),
        Ride::Teacups => teacups(editor, &place),
        Ride::Spinner => spinner(editor, &place),
    }
}

struct Placement<'a> {
    site: &'a Site,
    own: &'a HashSet<(i32, i32)>,
    colours: (Block, Block),
    /// The mapped colour, if any.
    tagged: Option<Block>,
    seed: u64,
    height: Option<f64>,
    footprints: &'a CoordinateBitmap,
    road_mask: &'a RoadMaskBitmap,
}

impl Placement<'_> {
    fn level(&self, editor: &mut WorldEditor, cells: &[(i32, i32)], paving: Block) -> i32 {
        level_ground(
            editor,
            cells,
            self.own,
            paving,
            self.footprints,
            self.road_mask,
        )
    }

    /// A fence ring round a round ride, with a gap for the entrance.
    fn fence_ring(&self, editor: &mut WorldEditor, r: f64, floor: i32) {
        let (cx, cz) = (self.site.cx, self.site.cz);
        let gate = (self.seed % 360) as f64 * PI / 180.0;
        for (x, z) in disc(cx, cz, r + 0.5) {
            let d = dist_to(cx, cz, x, z);
            if d < r - 0.5 || self.road_mask.contains(x, z) {
                continue;
            }
            let a = angle_of(cx, cz, x, z);
            let off = (a - gate).rem_euclid(TAU);
            if off < 1.6 / r || off > TAU - 1.6 / r {
                continue;
            }
            if self.footprints.contains(x, z) && !self.own.contains(&(x, z)) {
                continue;
            }
            place_connected(editor, IRON_BARS, x, floor + 1, z);
        }
    }
}

/// Merry-go-round: a round deck with horses on brass poles under a striped tent roof.
fn carousel(editor: &mut WorldEditor, p: &Placement) {
    let r = p.site.radius(7.0).clamp(4.0, 14.0);
    let (cx, cz) = (p.site.cx, p.site.cz);
    let (main, accent) = p.colours;
    let floor = p.level(editor, &disc(cx, cz, r + 1.5), POLISHED_ANDESITE);
    let deck = floor + 1;
    let eave = deck + 5;
    let slope = 0.55;
    let apex = eave + ((r * slope).round() as i32).max(2);
    let sectors = ((r * 1.6).round() as usize).clamp(8, 20) & !1;
    let sector_of = |x: i32, z: i32| (angle_of(cx, cz, x, z) / TAU * sectors as f64) as usize;
    for (x, z) in disc(cx, cz, r) {
        let d = dist_to(cx, cz, x, z);
        // Deck: wooden boards with a stepped rim.
        let board = if d > r - 1.0 {
            GOLD_BLOCK
        } else if sector_of(x, z) % 2 == 0 {
            SPRUCE_PLANKS
        } else {
            BIRCH_PLANKS
        };
        editor.set_block_absolute(board, x, deck, z, None, None);
        // Tent roof in stripes.
        let y = eave + ((r - d) * slope).round() as i32;
        let stripe = if sector_of(x, z) % 2 == 0 {
            main
        } else {
            accent
        };
        editor.set_block_absolute(stripe, x, y.min(apex), z, None, None);
        // Valance with bulbs under the eave.
        if d > r - 1.0 {
            let bulb = (x + z).rem_euclid(3) == 0;
            editor.set_block_absolute(
                if bulb { GLOWSTONE } else { GOLD_BLOCK },
                x,
                eave - 1,
                z,
                None,
                None,
            );
        }
        // Centre drum with painted panels.
        if d <= 1.6 {
            for y in deck + 1..y.min(apex) {
                let panel = if y == deck + 1 || y == eave - 1 {
                    GOLD_BLOCK
                } else if sector_of(x, z) % 2 == 0 {
                    WHITE_CONCRETE
                } else {
                    main
                };
                editor.set_block_absolute(panel, x, y, z, None, None);
            }
        }
    }
    let (ax, az) = (cx.floor() as i32, cz.floor() as i32);
    editor.set_block_absolute(GOLD_BLOCK, ax, apex + 1, az, None, None);
    editor.set_block_absolute(LIGHTNING_ROD, ax, apex + 2, az, None, None);
    // Horses on rings, nose to tail round the drum.
    let mut rings = vec![r - 2.0];
    if r >= 9.0 {
        rings.push(r - 4.5);
    }
    const COATS: [Block; 5] = [
        WHITE_WOOL,
        BROWN_TERRACOTTA,
        WHITE_WOOL,
        BLACK_WOOL,
        GRAY_WOOL,
    ];
    let mut placed: HashSet<(i32, i32)> = HashSet::new();
    let mut k = 0usize;
    for (ri, ring) in rings.iter().enumerate() {
        let count = ((TAU * ring) / 3.2).floor().max(4.0) as usize;
        for h in 0..count {
            let ang = TAU * (h as f64 + 0.5 * ri as f64) / count as f64;
            let (hx, hz) = (cx + ring * ang.cos(), cz + ring * ang.sin());
            let (x, z) = (hx.floor() as i32, hz.floor() as i32);
            // Heads point the way the carousel turns.
            let tangent = axis_of(-ang.sin(), ang.cos());
            let (nx, nz) = (x + tangent.0, z + tangent.1);
            if !placed.insert((x, z)) || !placed.insert((nx, nz)) {
                continue;
            }
            let rise = ((h + ri) % 2) as i32;
            for y in deck + 1..eave - 1 {
                if y != deck + 2 + rise {
                    editor.set_block_absolute(LIGHTNING_ROD, x, y, z, None, None);
                }
            }
            let coat = COATS[(k + (p.seed as usize)) % COATS.len()];
            editor.set_block_absolute(coat, x, deck + 2 + rise, z, None, None);
            place(
                editor,
                stair_backed(QUARTZ_STAIRS, tangent),
                nx,
                deck + 2 + rise,
                nz,
            );
            editor.set_block_absolute(coat, nx, deck + 3 + rise, nz, None, None);
            k += 1;
        }
    }
    if p.site.cells.is_empty() {
        p.fence_ring(editor, r + 1.5, floor);
    }
}

/// Classic chain swing ride, parked with its seats hanging straight down.
fn swing_ride(editor: &mut WorldEditor, p: &Placement) {
    let r = p.site.radius(8.0).clamp(5.0, 14.0);
    let (cx, cz) = (p.site.cx, p.site.cz);
    let (main, accent) = p.colours;
    let floor = p.level(editor, &disc(cx, cz, r + 1.5), POLISHED_ANDESITE);
    let crown_r = (r * 0.8).max(4.0);
    let top = floor
        + p.height
            .map(|h| h.round() as i32)
            .unwrap_or(9 + (r * 0.4) as i32)
            .clamp(8, 25);
    let rim = top - 2;
    let sectors = 16usize;
    for (x, z) in disc(cx, cz, crown_r + 0.5) {
        let d = dist_to(cx, cz, x, z);
        let a = angle_of(cx, cz, x, z);
        let stripe = if ((a / TAU * sectors as f64) as usize).is_multiple_of(2) {
            main
        } else {
            accent
        };
        // Umbrella crown, decorated rim with bulbs.
        let y = rim + ((crown_r - d) * 0.45).round() as i32;
        editor.set_block_absolute(stripe, x, y.min(top + 1), z, None, None);
        if d > crown_r - 1.0 {
            let bulb = (x + z).rem_euclid(3) == 0;
            editor.set_block_absolute(
                if bulb { GLOWSTONE } else { main },
                x,
                rim - 1,
                z,
                None,
                None,
            );
        }
        if d <= 1.0 {
            for y in floor + 1..rim {
                let band = (y - floor) % 4 == 0;
                editor.set_block_absolute(
                    if band { main } else { WHITE_CONCRETE },
                    x,
                    y,
                    z,
                    None,
                    None,
                );
            }
        }
    }
    // Swings on chains round the rim.
    let swing_r = crown_r - 1.2;
    let count = ((TAU * swing_r) / 2.5).floor().max(6.0) as usize;
    let mut used: HashSet<(i32, i32)> = HashSet::new();
    for s in 0..count {
        let ang = TAU * s as f64 / count as f64;
        let (x, z) = (
            (cx + swing_r * ang.cos()).floor() as i32,
            (cz + swing_r * ang.sin()).floor() as i32,
        );
        if !used.insert((x, z)) {
            continue;
        }
        for y in floor + 3..rim - 1 {
            editor.set_block_absolute(CHAIN_Y, x, y, z, None, None);
        }
        let out = axis_of(ang.cos(), ang.sin());
        place(
            editor,
            stair_backed(DARK_OAK_STAIRS, (-out.0, -out.1)),
            x,
            floor + 2,
            z,
        );
    }
    p.fence_ring(editor, r + 1.0, floor);
}

/// Tall swing tower: a mast with the swing carousel lifted near its top.
fn star_flyer(editor: &mut WorldEditor, p: &Placement) {
    let (cx, cz) = (p.site.cx, p.site.cz);
    let (main, accent) = p.colours;
    let h = p.height.unwrap_or(60.0).clamp(30.0, 130.0).round() as i32;
    let floor = p.level(editor, &disc(cx, cz, 9.5), POLISHED_ANDESITE);
    let top = floor + h;
    let (ax, az) = (cx.floor() as i32, cz.floor() as i32);
    // Mast, plus-shaped and banded.
    for y in floor + 1..=top {
        let band = (y - floor) % 12 < 2;
        let b = if band { main } else { WHITE_CONCRETE };
        editor.set_block_absolute(b, ax, y, az, None, None);
        if y < top - 3 {
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                editor.set_block_absolute(b, ax + dx, y, az + dz, None, None);
            }
        }
    }
    for (x, z) in disc(cx, cz, 2.6) {
        for y in top - 2..=top {
            editor.set_block_absolute(accent, x, y, z, None, None);
        }
        editor.set_block_absolute(GLOWSTONE, x, top + 1, z, None, None);
    }
    editor.set_block_absolute(LIGHTNING_ROD, ax, top + 2, az, None, None);
    editor.set_block_absolute(REDSTONE_LAMP, ax, top + 3, az, None, None);
    // The carousel ring, high up with its swings flung out.
    let ring_y = top - (h as f64 * 0.18).round() as i32 - 4;
    let ring_r = 7.5;
    for (x, z) in disc(cx, cz, ring_r + 0.5) {
        let d = dist_to(cx, cz, x, z);
        if d <= 1.6 {
            continue;
        }
        let a = angle_of(cx, cz, x, z);
        let stripe = if ((a / TAU * 16.0) as usize).is_multiple_of(2) {
            main
        } else {
            accent
        };
        let y = ring_y + if d > ring_r - 1.0 { 0 } else { 1 };
        editor.set_block_absolute(stripe, x, y, z, None, None);
        if d > ring_r - 1.0 && (x + z).rem_euclid(3) == 0 {
            editor.set_block_absolute(GLOWSTONE, x, ring_y - 1, z, None, None);
        }
    }
    let count = 24usize;
    for s in 0..count {
        let ang = TAU * s as f64 / count as f64;
        let (ux, uz) = (ang.cos(), ang.sin());
        let from = [
            cx + ux * (ring_r - 0.5),
            ring_y as f64,
            cz + uz * (ring_r - 0.5),
        ];
        // Seats swing out about 35 degrees.
        let len = 9.0;
        let to = [
            from[0] + ux * len * 0.57,
            from[1] - len * 0.82,
            from[2] + uz * len * 0.57,
        ];
        let seed = (p.seed as i64).wrapping_add(s as i64);
        beam(
            editor,
            from,
            to,
            0.06,
            "minecraft:gray_concrete",
            seed,
            CHAIN_X,
            CHAIN_Y,
        );
        let (sx, sy, sz) = (
            to[0].floor() as i32,
            to[1].floor() as i32,
            to[2].floor() as i32,
        );
        let out = axis_of(ux, uz);
        place(
            editor,
            stair_backed(DARK_OAK_STAIRS, (-out.0, -out.1)),
            sx,
            sy,
            sz,
        );
    }
    p.fence_ring(editor, 9.0, floor);
}

/// Ferris wheel in the plane of its outline's long axis.
fn big_wheel(editor: &mut WorldEditor, p: &Placement) {
    let site = p.site;
    let diameter = match (p.height, site.long > 0.0) {
        (Some(h), _) => (h - 4.0).max(12.0),
        (None, true) => site.long.max(12.0),
        (None, false) => 30.0,
    }
    .clamp(12.0, 150.0);
    let r = diameter / 2.0;
    let half_gap = if diameter >= 50.0 { 3.0 } else { 2.0 };
    let cabin_half = (half_gap - 1.0) as i32;
    let f = Frame::new(site.cx, site.cz, site.axis.0, site.axis.1);
    let pad: Vec<(i32, i32)> = f
        .cells(
            -r * 0.62 - 2.0,
            r * 0.62 + 2.0,
            -half_gap - 3.0,
            half_gap + 3.0,
        )
        .into_iter()
        .map(|(x, z, _, _)| (x, z))
        .collect();
    let floor = p.level(editor, &pad, POLISHED_ANDESITE);
    let cabin_h = if diameter >= 50.0 { 4 } else { 3 };
    let hub_y = floor as f64 + 3.0 + cabin_h as f64 + r;
    let rim = p.tagged.unwrap_or(WHITE_CONCRETE);
    let at = |a: f64, b: f64, y: f64| -> (i32, i32, i32) {
        let (x, z) = f.world(a, b);
        (x.floor() as i32, y.floor() as i32, z.floor() as i32)
    };
    // Two rims with lamps.
    let steps = (TAU * r * 2.5).ceil() as usize;
    for side in [-half_gap, half_gap] {
        for k in 0..steps {
            let t = TAU * k as f64 / steps as f64;
            let (x, y, z) = at(r * t.cos(), side, hub_y + r * t.sin());
            let lamp = k % 9 == 0;
            editor.set_block_absolute(if lamp { SEA_LANTERN } else { rim }, x, y, z, None, None);
        }
    }
    // Spokes, gondolas and the ties that carry them.
    let count = ((TAU * r) / 6.0).round().clamp(8.0, 48.0) as usize;
    let mut seed = (p.seed as i64).wrapping_mul(31);
    for k in 0..count {
        let t = TAU * (k as f64 + 0.25) / count as f64;
        let (ca, cy) = (r * t.cos(), hub_y + r * t.sin());
        for side in [-half_gap, half_gap] {
            let (x0, z0) = f.world(0.0, side);
            let (x1, z1) = f.world(ca, side);
            seed += 1;
            beam(
                editor,
                [x0, hub_y + 0.5, z0],
                [x1, cy + 0.5, z1],
                0.18,
                "minecraft:white_concrete",
                seed,
                IRON_BARS,
                IRON_BARS,
            );
        }
        let b0 = -half_gap as i32;
        for b in b0..=-b0 {
            let (x, y, z) = at(ca, b as f64, cy);
            editor.set_block_absolute(rim, x, y, z, None, None);
        }
        let colour = RAINBOW[k % RAINBOW.len()];
        let roof = cy.floor() as i32 - 1;
        let half = cabin_half as f64 + 0.5;
        let (hx, _, hz) = at(ca, 0.0, cy);
        editor.set_block_absolute(CHAIN_Y, hx, roof, hz, None, None);
        let floor_y = roof - cabin_h;
        for (x, z, a, b) in f.cells(ca - half, ca + half, -half, half) {
            let (ea, eb) = ((a - ca).abs() > half - 1.0, b.abs() > half - 1.0);
            // Roof and floor in the cabin colour, corner posts, a band of windows.
            editor.set_block_absolute(colour, x, roof - 1, z, None, None);
            editor.set_block_absolute(WHITE_CONCRETE, x, floor_y, z, None, None);
            if !(ea || eb) {
                continue;
            }
            for y in floor_y + 1..roof - 1 {
                let post = ea && eb;
                let sill = y == floor_y + 1 && cabin_h >= 4;
                let b = if post || sill {
                    colour
                } else {
                    GRAY_STAINED_GLASS
                };
                editor.set_block_absolute(b, x, y, z, None, None);
            }
        }
    }
    // Hub and axle between the rims.
    for (x, z, _, _) in f.cells(-1.6, 1.6, -half_gap - 1.5, half_gap + 1.5) {
        for y in (hub_y - 1.5).floor() as i32..=(hub_y + 1.5).floor() as i32 {
            editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, y, z, None, None);
        }
    }
    // A-frame legs either side.
    let leg_foot = r * 0.62;
    for side in [-(half_gap + 1.0), half_gap + 1.0] {
        for dir in [-1.0, 1.0] {
            let n = (r * 2.5).ceil() as usize;
            for k in 0..=n {
                let t = k as f64 / n as f64;
                let a = dir * leg_foot * t;
                let y = hub_y - (hub_y - floor as f64) * t;
                let (x, yy, z) = at(a, side, y);
                editor.set_block_absolute(WHITE_CONCRETE, x, yy, z, None, None);
                if diameter >= 60.0 {
                    let (x2, _, z2) = at(a, side + side.signum(), y);
                    editor.set_block_absolute(WHITE_CONCRETE, x2, yy, z2, None, None);
                }
            }
        }
    }
    // Boarding deck under the lowest gondola.
    for (x, z, _, _) in f.cells(-2.5, 2.5, -half_gap - 1.5, half_gap + 1.5) {
        editor.set_block_absolute(SMOOTH_STONE_SLAB, x, floor + 1, z, None, None);
    }
}

/// Drop tower: a tall banded column with the seat ring raised for the drop.
fn drop_tower(editor: &mut WorldEditor, p: &Placement) {
    let (cx, cz) = (p.site.cx, p.site.cz);
    let (main, accent) = p.colours;
    // Unknown height: a kiddie tower on a small plot, a big drop on a big one.
    let guess = if p.site.long > 0.0 {
        (p.site.long * 3.0).clamp(15.0, 60.0)
    } else {
        40.0
    };
    let h = p.height.unwrap_or(guess).clamp(12.0, 140.0).round() as i32;
    let floor = p.level(editor, &disc(cx, cz, 7.5), POLISHED_ANDESITE);
    let top = floor + h;
    let (ax, az) = (cx.floor() as i32, cz.floor() as i32);
    let half: i32 = if h >= 70 { 2 } else { 1 };
    for y in floor + 1..=top {
        let band = (y - floor) % 10 < 2;
        for dx in -half..=half {
            for dz in -half..=half {
                let corner = dx.abs() == half && dz.abs() == half;
                let b = if band {
                    main
                } else if corner {
                    WHITE_CONCRETE
                } else {
                    LIGHT_GRAY_CONCRETE
                };
                editor.set_block_absolute(b, ax + dx, y, az + dz, None, None);
            }
        }
    }
    // Head with lights and an aviation lamp.
    let cap = half + 2;
    for dx in -cap..=cap {
        for dz in -cap..=cap {
            for y in top + 1..=top + 3 {
                let edge = dx.abs() == cap || dz.abs() == cap;
                let b = if y == top + 2 && edge && (dx + dz).rem_euclid(2) == 0 {
                    GLOWSTONE
                } else {
                    accent
                };
                editor.set_block_absolute(b, ax + dx, y, az + dz, None, None);
            }
        }
    }
    editor.set_block_absolute(LIGHTNING_ROD, ax, top + 4, az, None, None);
    editor.set_block_absolute(REDSTONE_LAMP, ax, top + 5, az, None, None);
    // Seat ring near the top, riders facing out.
    let ring_y = top - (h / 6).max(4);
    let ring = half + 1;
    for dx in -ring..=ring {
        for dz in -ring..=ring {
            if dx.abs() != ring && dz.abs() != ring {
                continue;
            }
            let (x, z) = (ax + dx, az + dz);
            editor.set_block_absolute(main, x, ring_y - 1, z, None, None);
            editor.set_block_absolute(main, x, ring_y + 2, z, None, None);
            if dx.abs() == ring && dz.abs() == ring {
                editor.set_block_absolute(main, x, ring_y, z, None, None);
                editor.set_block_absolute(main, x, ring_y + 1, z, None, None);
                continue;
            }
            let out = if dx.abs() == ring {
                (dx.signum(), 0)
            } else {
                (0, dz.signum())
            };
            place(
                editor,
                stair_backed(POLISHED_DEEPSLATE_STAIRS, (-out.0, -out.1)),
                x,
                ring_y,
                z,
            );
            editor.set_block_absolute(YELLOW_CONCRETE, x, ring_y + 1, z, None, None);
        }
    }
    p.fence_ring(editor, 7.0, floor);
}

/// Swinging pirate ship hanging between two A-frames.
fn pirate_ship(editor: &mut WorldEditor, p: &Placement) {
    let site = p.site;
    let (main, accent) = p.colours;
    let len = if site.long > 0.0 { site.long } else { 16.0 }.clamp(10.0, 26.0);
    let half_l = len / 2.0;
    let half_w = if len >= 16.0 { 3.0 } else { 2.0 };
    let f = Frame::new(site.cx, site.cz, site.axis.0, site.axis.1);
    let pad: Vec<(i32, i32)> = f
        .cells(-half_l - 2.0, half_l + 2.0, -half_w - 4.0, half_w + 4.0)
        .into_iter()
        .map(|(x, z, _, _)| (x, z))
        .collect();
    let floor = p.level(editor, &pad, POLISHED_ANDESITE);
    let keel = floor + 2;
    let pivot = keel as f64 + len * 0.6 + 2.0;
    // Hull: a banana shape rising to bow and stern.
    for (x, z, a, b) in f.cells(-half_l, half_l, -half_w - 0.5, half_w + 0.5) {
        let u = (a.abs() / half_l).min(1.0);
        let lift = (u.powi(3) * 4.0).round() as i32;
        let bottom = keel + lift + if b.abs() > half_w - 1.0 { 1 } else { 0 };
        let gunwale = keel + 3 + lift;
        let side = b.abs() > half_w - 0.5 || a.abs() > half_l - 1.0;
        for y in bottom..=gunwale {
            let block = if y == gunwale && side {
                accent
            } else if side || y == bottom {
                if y == gunwale - 1 && side {
                    main
                } else {
                    DARK_OAK_PLANKS
                }
            } else {
                continue;
            };
            editor.set_block_absolute(block, x, y, z, None, None);
        }
        // Benches across the hull, riders facing the middle.
        if !side && (a.round() as i32).rem_euclid(2) == 0 && a.abs() > 1.0 {
            let back = axis_of(site.axis.0 * a.signum(), site.axis.1 * a.signum());
            place(editor, stair_backed(SPRUCE_STAIRS, back), x, bottom + 1, z);
        }
    }
    let (bx, bz) = f.cell(half_l + 0.5, 0.0);
    editor.set_block_absolute(GOLD_BLOCK, bx, keel + 6, bz, None, None);
    // Mast with a black flag.
    let (mx, mz) = f.cell(0.0, 0.0);
    for y in keel + 1..keel + 9 {
        editor.set_block_absolute(SPRUCE_LOG, mx, y, mz, None, None);
    }
    for (x, z, _, _) in f.cells(0.5, 3.5, -0.5, 0.5) {
        for y in keel + 6..keel + 9 {
            editor.set_block_absolute(BLACK_WOOL, x, y, z, None, None);
        }
    }
    // A-frames and the arms down to the hull.
    let frame_b = half_w + 2.0;
    let foot = len * 0.45;
    for side in [-frame_b, frame_b] {
        for dir in [-1.0, 1.0] {
            let n = (pivot * 2.0) as usize;
            for k in 0..=n {
                let t = k as f64 / n as f64;
                let (x, z) = f.cell(dir * foot * t, side);
                let y = (pivot - (pivot - floor as f64) * t).floor() as i32;
                editor.set_block_absolute(WHITE_CONCRETE, x, y, z, None, None);
            }
            let arm_b = side - side.signum();
            let n = (pivot * 2.0) as usize;
            for k in 0..=n {
                let t = k as f64 / n as f64;
                let (x, z) = f.cell(dir * half_l * 0.55 * t, arm_b);
                let y = (pivot - (pivot - (keel + 3) as f64) * t).floor() as i32;
                editor.set_block_absolute(main, x, y, z, None, None);
            }
        }
    }
    for (x, z, _, _) in f.cells(-0.5, 0.5, -frame_b - 0.5, frame_b + 0.5) {
        editor.set_block_absolute(GRAY_CONCRETE, x, pivot.floor() as i32, z, None, None);
    }
    // Boarding steps along one side.
    for (x, z, _, _) in f.cells(-3.0, 3.0, half_w + 0.5, half_w + 1.5) {
        editor.set_block_absolute(SMOOTH_STONE_SLAB, x, floor + 1, z, None, None);
    }
}

/// Open bumper car track: steel floor, ceiling grid on posts, cars with their poles.
fn bumper_cars(editor: &mut WorldEditor, p: &Placement) {
    let site = p.site;
    let (main, accent) = p.colours;
    let cells: Vec<(i32, i32)> = if site.cells.len() >= 30 {
        site.cells.clone()
    } else {
        let f = Frame::new(site.cx, site.cz, site.axis.0, site.axis.1);
        f.cells(-8.0, 8.0, -5.0, 5.0)
            .into_iter()
            .map(|(x, z, _, _)| (x, z))
            .collect()
    };
    let set: HashSet<(i32, i32)> = cells.iter().copied().collect();
    let floor = p.level(editor, &cells, SMOOTH_STONE);
    let ceiling = floor + 5;
    let edge = |x: i32, z: i32| {
        [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .iter()
            .any(|(dx, dz)| !set.contains(&(x + dx, z + dz)))
    };
    for &(x, z) in &cells {
        if edge(x, z) {
            editor.set_block_absolute(BLACK_CONCRETE, x, floor + 1, z, None, None);
            let post = (x + 2 * z).rem_euclid(6) == 0;
            if post {
                for y in floor + 2..ceiling {
                    editor.set_block_absolute(GRAY_CONCRETE, x, y, z, None, None);
                }
            }
            let band = if (x + z).rem_euclid(4) < 2 {
                main
            } else {
                accent
            };
            editor.set_block_absolute(band, x, ceiling, z, None, None);
            editor.set_block_absolute(band, x, ceiling + 1, z, None, None);
            if (x + z).rem_euclid(3) == 0 {
                editor.set_block_absolute(GLOWSTONE, x, ceiling + 1, z, None, None);
            }
        } else {
            let light = x.rem_euclid(4) == 0 && z.rem_euclid(4) == 0;
            editor.set_block_absolute(
                if light { SEA_LANTERN } else { GRAY_CONCRETE },
                x,
                ceiling,
                z,
                None,
                None,
            );
        }
    }
    // Cars parked about the floor, each with its pole to the ceiling.
    let inner: Vec<(i32, i32)> = cells
        .iter()
        .copied()
        .filter(|&(x, z)| (-2..=2).all(|dx| (-2..=2).all(|dz| set.contains(&(x + dx, z + dz)))))
        .collect();
    let mut taken: HashSet<(i32, i32)> = HashSet::new();
    let wanted = (inner.len() / 14).clamp(3, 16);
    let mut k = 0usize;
    let mut i = 0usize;
    while k < wanted && i < inner.len() * 2 {
        let pick = inner[(mix(p.seed, i as u64) % inner.len() as u64) as usize];
        i += 1;
        let dir = match mix(p.seed, 1000 + i as u64) % 4 {
            0 => (1, 0),
            1 => (-1, 0),
            2 => (0, 1),
            _ => (0, -1),
        };
        let front = (pick.0 + dir.0, pick.1 + dir.1);
        if (-1..=1).any(|dx| {
            (-1..=1).any(|dz| {
                taken.contains(&(pick.0 + dx, pick.1 + dz))
                    || taken.contains(&(front.0 + dx, front.1 + dz))
            })
        }) {
            continue;
        }
        taken.insert(pick);
        taken.insert(front);
        let colour = RAINBOW[(k + p.seed as usize) % RAINBOW.len()];
        editor.set_block_absolute(colour, front.0, floor + 1, front.1, None, None);
        place(
            editor,
            stair_backed(POLISHED_DEEPSLATE_STAIRS, (-dir.0, -dir.1)),
            pick.0,
            floor + 1,
            pick.1,
        );
        for y in floor + 2..ceiling {
            editor.set_block_absolute(IRON_BARS, pick.0, y, pick.1, None, None);
        }
        k += 1;
    }
}

/// Teacups spinning on a round deck.
fn teacups(editor: &mut WorldEditor, p: &Placement) {
    let r = p.site.radius(8.0).clamp(5.0, 14.0);
    let (cx, cz) = (p.site.cx, p.site.cz);
    let (main, _) = p.colours;
    let floor = p.level(editor, &disc(cx, cz, r + 1.5), POLISHED_ANDESITE);
    for (x, z) in disc(cx, cz, r) {
        let d = dist_to(cx, cz, x, z);
        let b = if d > r - 1.0 { main } else { WHITE_CONCRETE };
        editor.set_block_absolute(b, x, floor + 1, z, None, None);
    }
    let cups = ((TAU * r * 0.55) / 4.5).floor().clamp(3.0, 8.0) as usize;
    for c in 0..cups {
        let ang = TAU * c as f64 / cups as f64;
        let (ux, uz) = (cx + r * 0.55 * ang.cos(), cz + r * 0.55 * ang.sin());
        let colour = RAINBOW[(c * 3 + p.seed as usize) % RAINBOW.len()];
        for (x, z) in disc(ux, uz, 1.6) {
            let d = dist_to(ux, uz, x, z);
            editor.set_block_absolute(colour, x, floor + 2, z, None, None);
            if d > 0.8 {
                editor.set_block_absolute(colour, x, floor + 3, z, None, None);
            }
        }
        let (x, z) = (ux.floor() as i32, uz.floor() as i32);
        editor.set_block_absolute(OAK_FENCE, x, floor + 3, z, None, None);
        place(
            editor,
            with_prop(OAK_SLAB, "type", "bottom"),
            x,
            floor + 4,
            z,
        );
    }
    let (ax, az) = (cx.floor() as i32, cz.floor() as i32);
    for y in floor + 2..=floor + 4 {
        editor.set_block_absolute(main, ax, y, az, None, None);
    }
    editor.set_block_absolute(GLOWSTONE, ax, floor + 5, az, None, None);
    p.fence_ring(editor, r + 1.0, floor);
}

/// Spinning ride: a hub with arms reaching out to cars, on a round deck.
fn spinner(editor: &mut WorldEditor, p: &Placement) {
    let r = p.site.radius(8.0).clamp(4.0, 13.0);
    let (cx, cz) = (p.site.cx, p.site.cz);
    let (main, accent) = p.colours;
    let floor = p.level(editor, &disc(cx, cz, r + 1.5), POLISHED_ANDESITE);
    for (x, z) in disc(cx, cz, r) {
        let d = dist_to(cx, cz, x, z);
        let a = angle_of(cx, cz, x, z);
        let b = if d > r - 1.0 {
            accent
        } else if ((a / TAU * 12.0) as usize).is_multiple_of(2) {
            main
        } else {
            WHITE_CONCRETE
        };
        editor.set_block_absolute(b, x, floor + 1, z, None, None);
    }
    let hub_top = floor + 6;
    for (x, z) in disc(cx, cz, 1.6) {
        for y in floor + 2..=hub_top {
            editor.set_block_absolute(
                if y == hub_top { GLOWSTONE } else { main },
                x,
                y,
                z,
                None,
                None,
            );
        }
    }
    let arms = 6;
    for k in 0..arms {
        let ang = TAU * k as f64 / arms as f64 + (p.seed % 60) as f64 * PI / 180.0;
        let (ux, uz) = (ang.cos(), ang.sin());
        let reach = r - 2.0;
        let n = (reach * 3.0) as usize;
        for s in 0..=n {
            let t = s as f64 / n as f64;
            let d = 1.5 + (reach - 1.5) * t;
            let y = hub_top - 1 - (t * 2.0).round() as i32;
            let (x, z) = ((cx + ux * d).floor() as i32, (cz + uz * d).floor() as i32);
            editor.set_block_absolute(accent, x, y, z, None, None);
        }
        let (ex, ez) = (cx + ux * reach, cz + uz * reach);
        let colour = RAINBOW[(k + p.seed as usize) % RAINBOW.len()];
        let (x0, z0) = (ex.floor() as i32, ez.floor() as i32);
        let out = axis_of(ux, uz);
        for (dx, dz) in [(0, 0), (out.1, -out.0)] {
            editor.set_block_absolute(colour, x0 + dx, floor + 2, z0 + dz, None, None);
            place(
                editor,
                stair_backed(POLISHED_DEEPSLATE_STAIRS, (-out.0, -out.1)),
                x0 + dx - out.0,
                floor + 2,
                z0 + dz - out.1,
            );
        }
        editor.set_block_absolute(CHAIN_Y, x0, floor + 3, z0, None, None);
    }
    p.fence_ring(editor, r + 1.0, floor);
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
    fn ride_buildings_are_claimed_but_halls_are_not() {
        assert!(claims(&tags(&[
            ("attraction", "carousel"),
            ("building", "yes")
        ])));
        assert!(claims(&tags(&[("attraction", "big_wheel")])));
        assert!(!claims(&tags(&[
            ("attraction", "dark_ride"),
            ("building", "yes")
        ])));
        assert!(!claims(&tags(&[
            ("attraction", "bumper_car"),
            ("building", "yes")
        ])));
        assert!(claims(&tags(&[("attraction", "bumper_car")])));
        assert!(!claims(&tags(&[("attraction", "animal")])));
    }

    #[test]
    fn tall_swing_rides_become_star_flyers() {
        let t = tags(&[
            ("attraction", "swing_carousel"),
            ("height", "117"),
            ("man_made", "tower"),
        ]);
        assert_eq!(ride_kind(&t), Some(Ride::StarFlyer));
        let t = tags(&[("attraction", "swing_carousel")]);
        assert_eq!(ride_kind(&t), Some(Ride::ChainSwings));
    }

    #[test]
    fn names_sort_out_generic_rides() {
        let t = tags(&[("attraction", "amusement_ride"), ("name", "Riesenrad Wien")]);
        assert_eq!(ride_kind(&t), Some(Ride::BigWheel));
        let t = tags(&[("attraction", "amusement_ride"), ("name", "Autoscooter")]);
        assert_eq!(ride_kind(&t), Some(Ride::BumperCars));
        let t = tags(&[("attraction", "amusement_ride"), ("man_made", "tower")]);
        assert_eq!(ride_kind(&t), Some(Ride::DropTower));
        let t = tags(&[("attraction", "amusement_ride"), ("name", "Tagada")]);
        assert_eq!(ride_kind(&t), Some(Ride::Spinner));
    }
}
