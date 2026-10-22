//! Petrol stations: a branded canopy over pump islands.
//!
//! Most stations are a fuel node or area with the canopy mapped separately as
//! `building=roof`, so an index built before the tiles links each canopy to its station.
//! A station without a mapped canopy gets one where there is room.

use crate::block_definitions::*;
use crate::element_processing::get_nearest_road_block;
use crate::element_processing::structure_kit::{
    axis_of, facing_name, mix, place, with_prop, with_props, Frame,
};
use crate::floodfill_cache::{CoordinateBitmap, FloodFillCache, RoadMaskBitmap};
use crate::mapillary::geometry::parse_length_m;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

fn tag<'a>(tags: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    tags.get(key).map(String::as_str)
}

fn is_fuel(tags: &HashMap<String, String>) -> bool {
    tag(tags, "amenity") == Some("fuel")
}

fn is_roof(tags: &HashMap<String, String>) -> bool {
    tag(tags, "building") == Some("roof") || tag(tags, "building:part") == Some("roof")
}

/// Colours a station is dressed in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Livery {
    fascia: Block,
    stripe: Block,
    /// A second, lower stripe.
    stripe2: Option<Block>,
    column: Block,
}

const fn livery(fascia: Block, stripe: Block, stripe2: Option<Block>, column: Block) -> Livery {
    Livery {
        fascia,
        stripe,
        stripe2,
        column,
    }
}

/// Brand colours by a lower-case fragment of `brand`, `name` or `operator`.
const BRANDS: &[(&str, Livery)] = &[
    (
        "aral",
        livery(
            LIGHT_BLUE_CONCRETE,
            WHITE_CONCRETE,
            None,
            LIGHT_BLUE_CONCRETE,
        ),
    ),
    (
        "shell",
        livery(YELLOW_CONCRETE, RED_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "esso",
        livery(
            WHITE_CONCRETE,
            RED_CONCRETE,
            Some(BLUE_CONCRETE),
            WHITE_CONCRETE,
        ),
    ),
    (
        "exxon",
        livery(
            WHITE_CONCRETE,
            RED_CONCRETE,
            Some(BLUE_CONCRETE),
            WHITE_CONCRETE,
        ),
    ),
    (
        "mobil",
        livery(
            WHITE_CONCRETE,
            BLUE_CONCRETE,
            Some(RED_CONCRETE),
            WHITE_CONCRETE,
        ),
    ),
    (
        "bp",
        livery(GREEN_CONCRETE, LIME_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "total",
        livery(
            WHITE_CONCRETE,
            RED_CONCRETE,
            Some(ORANGE_CONCRETE),
            WHITE_CONCRETE,
        ),
    ),
    (
        "jet",
        livery(YELLOW_CONCRETE, BLUE_CONCRETE, None, YELLOW_CONCRETE),
    ),
    (
        "avia",
        livery(WHITE_CONCRETE, RED_CONCRETE, None, RED_CONCRETE),
    ),
    (
        "agip",
        livery(YELLOW_CONCRETE, BLACK_CONCRETE, None, YELLOW_CONCRETE),
    ),
    (
        "eni",
        livery(YELLOW_CONCRETE, BLACK_CONCRETE, None, YELLOW_CONCRETE),
    ),
    (
        "omv",
        livery(
            WHITE_CONCRETE,
            BLUE_CONCRETE,
            Some(LIME_CONCRETE),
            WHITE_CONCRETE,
        ),
    ),
    (
        "q8",
        livery(BLUE_CONCRETE, ORANGE_CONCRETE, None, BLUE_CONCRETE),
    ),
    (
        "texaco",
        livery(BLACK_CONCRETE, RED_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "chevron",
        livery(
            WHITE_CONCRETE,
            BLUE_CONCRETE,
            Some(RED_CONCRETE),
            WHITE_CONCRETE,
        ),
    ),
    (
        "circle k",
        livery(RED_CONCRETE, ORANGE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "repsol",
        livery(ORANGE_CONCRETE, RED_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "lukoil",
        livery(RED_CONCRETE, WHITE_CONCRETE, None, RED_CONCRETE),
    ),
    (
        "gazprom",
        livery(BLUE_CONCRETE, WHITE_CONCRETE, None, BLUE_CONCRETE),
    ),
    (
        "rosneft",
        livery(YELLOW_CONCRETE, BLACK_CONCRETE, None, YELLOW_CONCRETE),
    ),
    (
        "petrobras",
        livery(GREEN_CONCRETE, YELLOW_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "pemex",
        livery(GREEN_CONCRETE, RED_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "indian oil",
        livery(ORANGE_CONCRETE, BLUE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "sinopec",
        livery(WHITE_CONCRETE, RED_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "petrochina",
        livery(RED_CONCRETE, YELLOW_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "eneos",
        livery(WHITE_CONCRETE, RED_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "caltex",
        livery(RED_CONCRETE, BLUE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "ampol",
        livery(
            WHITE_CONCRETE,
            RED_CONCRETE,
            Some(BLUE_CONCRETE),
            WHITE_CONCRETE,
        ),
    ),
    (
        "speedway",
        livery(RED_CONCRETE, WHITE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "marathon",
        livery(
            WHITE_CONCRETE,
            BLUE_CONCRETE,
            Some(RED_CONCRETE),
            WHITE_CONCRETE,
        ),
    ),
    (
        "valero",
        livery(BLUE_CONCRETE, YELLOW_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "sunoco",
        livery(
            BLUE_CONCRETE,
            YELLOW_CONCRETE,
            Some(RED_CONCRETE),
            WHITE_CONCRETE,
        ),
    ),
    (
        "citgo",
        livery(
            WHITE_CONCRETE,
            RED_CONCRETE,
            Some(BLUE_CONCRETE),
            WHITE_CONCRETE,
        ),
    ),
    (
        "phillips 66",
        livery(BLACK_CONCRETE, RED_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "conoco",
        livery(RED_CONCRETE, WHITE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "sinclair",
        livery(GREEN_CONCRETE, WHITE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "costco",
        livery(RED_CONCRETE, BLUE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "arco",
        livery(
            BLUE_CONCRETE,
            WHITE_CONCRETE,
            Some(RED_CONCRETE),
            WHITE_CONCRETE,
        ),
    ),
    (
        "76",
        livery(ORANGE_CONCRETE, BLUE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "wawa",
        livery(RED_CONCRETE, YELLOW_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "sheetz",
        livery(RED_CONCRETE, WHITE_CONCRETE, None, RED_CONCRETE),
    ),
    (
        "casey",
        livery(RED_CONCRETE, YELLOW_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "quiktrip",
        livery(RED_CONCRETE, WHITE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "love's",
        livery(YELLOW_CONCRETE, RED_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "pilot",
        livery(RED_CONCRETE, YELLOW_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "tesco",
        livery(BLUE_CONCRETE, RED_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "sainsbury",
        livery(ORANGE_CONCRETE, WHITE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "morrisons",
        livery(YELLOW_CONCRETE, GREEN_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "asda",
        livery(GREEN_CONCRETE, WHITE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "gulf",
        livery(ORANGE_CONCRETE, BLUE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "tamoil",
        livery(ORANGE_CONCRETE, RED_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "orlen",
        livery(RED_CONCRETE, WHITE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "star",
        livery(RED_CONCRETE, YELLOW_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "bft",
        livery(BLUE_CONCRETE, RED_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "hem",
        livery(RED_CONCRETE, WHITE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "westfalen",
        livery(GREEN_CONCRETE, YELLOW_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "raiffeisen",
        livery(GREEN_CONCRETE, YELLOW_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "neste",
        livery(GREEN_CONCRETE, BLUE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "okq8",
        livery(BLUE_CONCRETE, ORANGE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "st1",
        livery(ORANGE_CONCRETE, BLUE_CONCRETE, None, WHITE_CONCRETE),
    ),
    (
        "tinq",
        livery(BLUE_CONCRETE, ORANGE_CONCRETE, None, WHITE_CONCRETE),
    ),
];

/// A station's livery from its tags, or a plain white one with a seeded stripe.
pub fn livery_for(tags: &HashMap<String, String>, seed: u64) -> Livery {
    for key in ["brand", "name", "operator"] {
        let Some(value) = tag(tags, key) else {
            continue;
        };
        let v = value.to_lowercase();
        // Whole-word match, so "Stará" is not Star and "Esso" is not in "Pessoa".
        let words: Vec<&str> = v
            .split(|c: char| !c.is_alphanumeric() && c != '\'')
            .filter(|w| !w.is_empty())
            .collect();
        for (brand, look) in BRANDS {
            let parts: Vec<&str> = brand.split(' ').collect();
            let hit = words
                .windows(parts.len())
                .any(|w| w.iter().zip(&parts).all(|(a, b)| a == b));
            if hit {
                return *look;
            }
        }
    }
    const PLAIN: [Block; 4] = [RED_CONCRETE, BLUE_CONCRETE, GREEN_CONCRETE, ORANGE_CONCRETE];
    livery(
        WHITE_CONCRETE,
        PLAIN[(seed % PLAIN.len() as u64) as usize],
        None,
        WHITE_CONCRETE,
    )
}

#[derive(Clone, Debug)]
struct Site {
    id: u64,
    x: f64,
    z: f64,
    /// Outline of an area station.
    ring: Vec<(f64, f64)>,
    livery: Livery,
    has_canopy: bool,
    has_shop: bool,
}

/// Stations and the canopies that belong to them, shared by every tile.
#[derive(Default, Debug)]
pub struct FuelIndex {
    sites: Vec<Site>,
    canopies: HashMap<u64, usize>,
}

/// How far a canopy or shop may stand from a station node.
const NODE_REACH: f64 = 40.0;

fn ring_of(way: &ProcessedWay) -> Vec<(f64, f64)> {
    way.nodes
        .iter()
        .map(|n| (n.x as f64 + 0.5, n.z as f64 + 0.5))
        .collect()
}

fn centre(ring: &[(f64, f64)]) -> (f64, f64) {
    let n = ring.len().max(1) as f64;
    (
        ring.iter().map(|p| p.0).sum::<f64>() / n,
        ring.iter().map(|p| p.1).sum::<f64>() / n,
    )
}

fn inside(ring: &[(f64, f64)], x: f64, z: f64) -> bool {
    let mut hit = false;
    let n = ring.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, zi) = ring[i];
        let (xj, zj) = ring[j];
        if (zi > z) != (zj > z) && x < (xj - xi) * (z - zi) / (zj - zi) + xi {
            hit = !hit;
        }
        j = i;
    }
    hit
}

impl FuelIndex {
    pub fn build(elements: &[ProcessedElement], scale: f64) -> FuelIndex {
        let mut sites: Vec<Site> = Vec::new();
        for e in elements {
            let tags = e.tags();
            if !is_fuel(tags) {
                continue;
            }
            let (x, z, ring) = match e {
                ProcessedElement::Node(n) => (n.x as f64 + 0.5, n.z as f64 + 0.5, Vec::new()),
                ProcessedElement::Way(w) if w.nodes.len() >= 4 => {
                    let ring = ring_of(w);
                    let (x, z) = centre(&ring);
                    (x, z, ring)
                }
                _ => continue,
            };
            sites.push(Site {
                id: e.id(),
                x,
                z,
                ring,
                livery: livery_for(tags, mix(e.id(), 0xF0E1)),
                has_canopy: is_roof(tags),
                has_shop: tags.contains_key("building") && !is_roof(tags),
            });
        }
        // A node inside or beside a mapped station area is the same station.
        let areas: Vec<Site> = sites
            .iter()
            .filter(|s| !s.ring.is_empty())
            .cloned()
            .collect();
        let merge = 30.0 * scale;
        sites.retain(|s| {
            !s.ring.is_empty()
                || !areas.iter().any(|a| {
                    inside(&a.ring, s.x, s.z)
                        || ((a.x - s.x).powi(2) + (a.z - s.z).powi(2)).sqrt() < merge
                })
        });
        let mut index = FuelIndex {
            sites,
            canopies: HashMap::new(),
        };
        if index.sites.is_empty() {
            return index;
        }
        let reach = NODE_REACH * scale;
        for e in elements {
            let ProcessedElement::Way(w) = e else {
                continue;
            };
            if !w.tags.contains_key("building") && !w.tags.contains_key("building:part") {
                continue;
            }
            let ring = ring_of(w);
            let (cx, cz) = centre(&ring);
            let roof = is_roof(&w.tags);
            let best = index
                .sites
                .iter()
                .enumerate()
                .filter(|(_, s)| s.id != w.id)
                .filter_map(|(i, s)| {
                    let d = ((s.x - cx).powi(2) + (s.z - cz).powi(2)).sqrt();
                    let near = if s.ring.is_empty() {
                        d <= reach
                    } else {
                        inside(&s.ring, cx, cz)
                    };
                    near.then_some((i, d))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((i, _)) = best {
                if roof {
                    index.canopies.insert(w.id, i);
                    index.sites[i].has_canopy = true;
                } else {
                    index.sites[i].has_shop = true;
                }
            }
        }
        // A canopy that is itself the station is its own site.
        for (i, s) in index.sites.iter().enumerate() {
            if s.has_canopy && !s.ring.is_empty() {
                index.canopies.entry(s.id).or_insert(i);
            }
        }
        index
    }

    fn site_for_canopy(&self, way_id: u64) -> Option<&Site> {
        self.canopies.get(&way_id).map(|&i| &self.sites[i])
    }

    fn site(&self, id: u64) -> Option<&Site> {
        self.sites.iter().find(|s| s.id == id)
    }
}

/// True for a mapped canopy that belongs to a station.
pub fn is_station_canopy(editor: &WorldEditor, way: &ProcessedWay) -> bool {
    is_roof(&way.tags)
        && editor
            .fuel_index()
            .is_some_and(|i| i.canopies.contains_key(&way.id))
}

/// Ground under a structure, from the terrain so every tile agrees.
fn ground_at(editor: &WorldEditor, x: i32, z: i32) -> i32 {
    editor
        .terrain_level(x, z)
        .unwrap_or_else(|| editor.get_ground_level(x, z))
}

/// The long axis of a set of cells, as a unit vector.
fn long_axis(cells: &[(i32, i32)]) -> (f64, f64, f64, f64) {
    let n = cells.len().max(1) as f64;
    let mx = cells.iter().map(|c| c.0 as f64 + 0.5).sum::<f64>() / n;
    let mz = cells.iter().map(|c| c.1 as f64 + 0.5).sum::<f64>() / n;
    let (mut sxx, mut szz, mut sxz) = (0.0, 0.0, 0.0);
    for &(x, z) in cells {
        let (dx, dz) = (x as f64 + 0.5 - mx, z as f64 + 0.5 - mz);
        sxx += dx * dx;
        szz += dz * dz;
        sxz += dx * dz;
    }
    let angle = 0.5 * (2.0 * sxz).atan2(sxx - szz);
    (mx, mz, angle.cos(), angle.sin())
}

/// Draws a mapped canopy in its station's livery, with the pumps under it.
#[allow(clippy::too_many_arguments)]
pub fn generate_canopy(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    flood_fill_cache: &FloodFillCache,
    road_mask: &RoadMaskBitmap,
    scale: f64,
    timeout: Option<&Duration>,
) {
    let Some(index) = editor.fuel_index() else {
        return;
    };
    let Some(site) = index.site_for_canopy(way.id).cloned() else {
        return;
    };
    let cells: Vec<(i32, i32)> = flood_fill_cache
        .get_or_compute(way, timeout)
        .iter()
        .copied()
        .collect();
    if cells.len() < 12 {
        return;
    }
    let metres = |key: &str| {
        tag(&way.tags, key)
            .and_then(parse_length_m)
            .filter(|m| *m > 0.0 && *m < 30.0)
    };
    let soffit = metres("min_height")
        .map(|m| (m * scale).round() as i32)
        .unwrap_or(5)
        .clamp(4, 9);
    let top = metres("height")
        .map(|m| (m * scale).round() as i32)
        .unwrap_or(soffit + 2)
        .clamp(soffit + 1, soffit + 3);
    draw_canopy(editor, &cells, &site, soffit, top, road_mask, way.id);
}

fn draw_canopy(
    editor: &mut WorldEditor,
    cells: &[(i32, i32)],
    site: &Site,
    soffit: i32,
    top: i32,
    road_mask: &RoadMaskBitmap,
    seed: u64,
) {
    let set: HashSet<(i32, i32)> = cells.iter().copied().collect();
    let mut heights: Vec<i32> = cells
        .iter()
        .map(|&(x, z)| ground_at(editor, x, z))
        .collect();
    heights.sort_unstable();
    let base = heights[heights.len() / 2];
    let look = site.livery;
    let edge = |x: i32, z: i32| {
        [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .iter()
            .any(|(dx, dz)| !set.contains(&(x + dx, z + dz)))
    };
    // Forecourt, soffit with lights, and the branded fascia round the edge.
    for &(x, z) in cells {
        if !road_mask.contains(x, z) {
            editor.register_road_surface_y(x, z, base);
            editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, base, z, None, None);
        }
        if edge(x, z) {
            for y in base + soffit..=base + top {
                let from_top = base + top - y;
                let b = if from_top == 0 {
                    look.fascia
                } else if from_top == 1 {
                    look.stripe
                } else if from_top == 2 {
                    look.stripe2.unwrap_or(look.fascia)
                } else {
                    look.fascia
                };
                editor.set_block_absolute(b, x, y, z, None, None);
            }
        } else {
            let lamp = x.rem_euclid(3) == 0 && z.rem_euclid(3) == 0;
            editor.set_block_absolute(
                if lamp { SEA_LANTERN } else { WHITE_CONCRETE },
                x,
                base + soffit,
                z,
                None,
                None,
            );
            for y in base + soffit + 1..=base + top {
                editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, y, z, None, None);
            }
        }
    }
    // Pump islands along the long axis, a column on each between two pumps.
    let (mx, mz, ux, uz) = long_axis(cells);
    let f = Frame::new(mx, mz, ux, uz);
    let (mut a0, mut a1, mut b0, mut b1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for &(x, z) in cells {
        let (a, b) = f.local(x as f64 + 0.5, z as f64 + 0.5);
        a0 = a0.min(a);
        a1 = a1.max(a);
        b0 = b0.min(b);
        b1 = b1.max(b);
    }
    let (len, width) = (a1 - a0, b1 - b0);
    if len < 6.0 || width < 4.0 {
        return;
    }
    let rows = ((width - 1.0) / 7.0).round().max(1.0) as usize;
    let bays = ((len - 2.0) / 8.0).round().max(1.0) as usize;
    let along = axis_of(ux, uz);
    let across = (-along.1, along.0);
    let axis_aligned = ux.abs() > 0.92 || uz.abs() > 0.92;
    let mut k = 0u64;
    for r in 0..rows {
        let b = b0 + width * (r as f64 + 0.5) / rows as f64;
        for bay in 0..bays {
            let a = a0 + len * (bay as f64 + 0.5) / bays as f64;
            let at = |da: f64| f.cell(a + da, b);
            // Lanes mapped through the station run between the islands, not round them.
            let inside_canopy = [-3.0, 3.0].iter().all(|&d| set.contains(&at(d)));
            if !inside_canopy {
                continue;
            }
            for da in [-3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0] {
                let (x, z) = at(da);
                place(
                    editor,
                    with_prop(SMOOTH_STONE_SLAB, "type", "bottom"),
                    x,
                    base + 1,
                    z,
                );
            }
            let (cx, cz) = at(0.0);
            for y in base + 1..base + soffit {
                editor.set_block_absolute(look.column, cx, y, cz, None, None);
            }
            for da in [-2.0, 2.0] {
                let (x, z) = at(da);
                editor.set_block_absolute(
                    WHITE_CONCRETE,
                    x,
                    base + 1,
                    z,
                    Some(&[SMOOTH_STONE_SLAB]),
                    None,
                );
                editor.set_block_absolute(pump_head(look), x, base + 2, z, None, None);
                place(
                    editor,
                    with_prop(SMOOTH_STONE_SLAB, "type", "bottom"),
                    x,
                    base + 3,
                    z,
                );
                // A nozzle holstered on each side.
                for side in [across, (-across.0, -across.1)] {
                    place(
                        editor,
                        with_props(
                            TRIPWIRE_HOOK,
                            &[
                                ("facing", facing_name(side)),
                                ("attached", "false"),
                                ("powered", "false"),
                            ],
                        ),
                        x + side.0,
                        base + 2,
                        z + side.1,
                    );
                }
            }
            for da in [-3.0, 3.0] {
                let (x, z) = at(da);
                editor.set_block_absolute(
                    ANDESITE_WALL,
                    x,
                    base + 1,
                    z,
                    Some(&[SMOOTH_STONE_SLAB]),
                    None,
                );
                editor.set_block_absolute(YELLOW_CONCRETE, x, base + 2, z, None, None);
            }
            let (bx, bz) = at(-1.0);
            editor.set_block_absolute(CAULDRON, bx, base + 1, bz, Some(&[SMOOTH_STONE_SLAB]), None);
            // A car filling up on either side now and then.
            if axis_aligned {
                for side in [-3.0, 3.0] {
                    k += 1;
                    if !mix(seed, k).is_multiple_of(3) {
                        continue;
                    }
                    let (x, z) = f.cell(a - 0.5, b + side);
                    if set.contains(&(x, z)) {
                        let rot = if along.0 != 0 { 1 } else { 0 };
                        crate::structures::car::maybe_place_car_on_deck(
                            editor,
                            x,
                            z,
                            base + 1,
                            rot,
                            soffit - 1,
                        );
                    }
                }
            }
        }
    }
}

/// The coloured head of a pump: the stripe colour unless that is plain white.
fn pump_head(look: Livery) -> Block {
    if look.stripe == WHITE_CONCRETE {
        look.fascia
    } else {
        look.stripe
    }
}

/// A canopy (and kiosk) for a station that has none mapped.
#[allow(clippy::too_many_arguments)]
pub fn generate_station(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    flood_fill_cache: &FloodFillCache,
    footprints: &CoordinateBitmap,
    road_mask: &RoadMaskBitmap,
    scale: f64,
    timeout: Option<&Duration>,
) {
    let Some(index) = editor.fuel_index() else {
        return;
    };
    let Some(site) = index.site(element.id()).cloned() else {
        return;
    };
    let road = get_nearest_road_block(site.x as i32, site.z as i32, 40, road_mask);
    if site.has_canopy {
        return;
    }
    // Area stations keep their canopy inside the area.
    let area: HashSet<(i32, i32)> = match element {
        ProcessedElement::Way(w) if !site.ring.is_empty() => flood_fill_cache
            .get_or_compute(w, timeout)
            .iter()
            .copied()
            .collect(),
        _ => HashSet::new(),
    };
    // The canopy runs along the road it serves, set back from it.
    let (rx, rz) = road
        .map(|(x, z)| (x as f64 + 0.5 - site.x, z as f64 + 0.5 - site.z))
        .unwrap_or((0.0, 1.0));
    let away = {
        let d = (rx * rx + rz * rz).sqrt().max(1e-6);
        (-rx / d, -rz / d)
    };
    let (half_l, half_w) = ((7.0 * scale).max(5.0), (4.5 * scale).max(3.5));
    // Clear ground first; then over lanes mapped through the forecourt, never over buildings.
    for road_share in [0.0, 0.5] {
        for shift in [0.0, 3.0, 6.0, 9.0] {
            let (cx, cz) = (site.x + away.0 * shift, site.z + away.1 * shift);
            let f = Frame::new(cx, cz, -away.1, away.0);
            let cells: Vec<(i32, i32)> = f
                .cells(-half_l, half_l, -half_w, half_w)
                .into_iter()
                .map(|(x, z, _, _)| (x, z))
                .collect();
            let blocked = cells.iter().any(|&(x, z)| {
                footprints.contains(x, z) || (!area.is_empty() && !area.contains(&(x, z)))
            });
            let on_road = cells
                .iter()
                .filter(|&&(x, z)| road_mask.contains(x, z))
                .count();
            if blocked || on_road as f64 > road_share * cells.len() as f64 {
                continue;
            }
            draw_canopy(editor, &cells, &site, 5, 7, road_mask, site.id);
            if !site.has_shop {
                kiosk(editor, &site, &f, half_w, footprints, road_mask, &area);
            }
            return;
        }
    }
}

/// A small shop behind a generated canopy.
fn kiosk(
    editor: &mut WorldEditor,
    site: &Site,
    canopy: &Frame,
    canopy_half_w: f64,
    footprints: &CoordinateBitmap,
    road_mask: &RoadMaskBitmap,
    area: &HashSet<(i32, i32)>,
) {
    let (b0, b1) = (canopy_half_w + 3.0, canopy_half_w + 9.0);
    let cells = canopy.cells(-4.5, 4.5, b0, b1);
    if cells.iter().any(|&(x, z, _, _)| {
        footprints.contains(x, z)
            || road_mask.contains(x, z)
            || (!area.is_empty() && !area.contains(&(x, z)))
    }) {
        return;
    }
    let mut heights: Vec<i32> = cells
        .iter()
        .map(|&(x, z, _, _)| ground_at(editor, x, z))
        .collect();
    heights.sort_unstable();
    let base = heights[heights.len() / 2];
    let front = axis_of(-canopy.v().0, -canopy.v().1);
    for &(x, z, a, b) in &cells {
        editor.register_road_surface_y(x, z, base);
        editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, base, z, None, None);
        let wall = a.abs() > 4.0 || (b - b0) < 1.0 || (b1 - b) < 1.0;
        if wall {
            let glazed = (b - b0) < 1.0 && a.abs() < 3.5;
            for y in base + 1..=base + 3 {
                let block = if glazed && y < base + 3 {
                    GLASS
                } else {
                    WHITE_CONCRETE
                };
                editor.set_block_absolute(block, x, y, z, None, None);
            }
            editor.set_block_absolute(site.livery.fascia, x, base + 4, z, None, None);
        } else {
            editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, base + 4, z, None, None);
        }
    }
    // Door in the glazed front.
    let (dx, dz) = canopy.cell(0.0, b0 + 0.5);
    editor.set_block_absolute(AIR, dx, base + 1, dz, Some(&[GLASS]), None);
    editor.set_block_absolute(AIR, dx, base + 2, dz, Some(&[GLASS]), None);
    place(
        editor,
        with_props(
            IRON_DOOR,
            &[("facing", facing_name(front)), ("half", "lower")],
        ),
        dx,
        base + 1,
        dz,
    );
    place(
        editor,
        with_props(
            IRON_DOOR,
            &[("facing", facing_name(front)), ("half", "upper")],
        ),
        dx,
        base + 2,
        dz,
    );
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
    fn brands_match_whole_words() {
        assert_eq!(
            livery_for(&tags(&[("brand", "Shell")]), 0).fascia,
            YELLOW_CONCRETE
        );
        assert_eq!(
            livery_for(&tags(&[("name", "Aral Tankstelle")]), 0).fascia,
            LIGHT_BLUE_CONCRETE
        );
        assert_eq!(
            livery_for(&tags(&[("brand", "Circle K")]), 0).fascia,
            RED_CONCRETE
        );
        // Neither "Stará" nor "Pessoa" is a brand.
        assert_eq!(
            livery_for(&tags(&[("name", "Stará Pessoa")]), 0).fascia,
            WHITE_CONCRETE
        );
    }

    #[test]
    fn canopies_find_their_station() {
        use crate::osm_parser::{ProcessedNode, ProcessedWay};
        let node = |id, x, z, t: &[(&str, &str)]| ProcessedNode {
            id,
            tags: tags(t),
            x,
            z,
        };
        let rect = |id, x0: i32, z0: i32, x1: i32, z1: i32, t: &[(&str, &str)]| {
            ProcessedElement::Way(ProcessedWay {
                id,
                nodes: vec![
                    node(id * 10, x0, z0, &[]),
                    node(id * 10 + 1, x1, z0, &[]),
                    node(id * 10 + 2, x1, z1, &[]),
                    node(id * 10 + 3, x0, z1, &[]),
                    node(id * 10, x0, z0, &[]),
                ],
                tags: tags(t),
            })
        };
        let elements = vec![
            ProcessedElement::Node(node(1, 10, 10, &[("amenity", "fuel"), ("brand", "Esso")])),
            rect(2, 0, 0, 16, 10, &[("building", "roof")]),
            rect(3, 20, 0, 30, 8, &[("building", "retail")]),
            rect(4, 300, 300, 316, 310, &[("building", "roof")]),
        ];
        let index = FuelIndex::build(&elements, 1.0);
        assert!(index.canopies.contains_key(&2));
        assert!(!index.canopies.contains_key(&4));
        let site = index.site(1).unwrap();
        assert!(site.has_canopy && site.has_shop);

        // The same station mapped as a node and as its building is one site.
        let elements = vec![
            ProcessedElement::Node(node(5, 40, 40, &[("amenity", "fuel")])),
            rect(
                6,
                35,
                35,
                50,
                48,
                &[("amenity", "fuel"), ("building", "yes")],
            ),
        ];
        let index = FuelIndex::build(&elements, 1.0);
        assert!(index.site(5).is_none() && index.site(6).is_some());
    }
}
