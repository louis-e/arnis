//! Interiors furnished by use, per storey and per unit of a shared storey.

mod canvas;
mod civic;
mod commerce;
mod halls;
pub mod index;
mod residential;
pub mod uses;

pub use canvas::Entry;
pub use index::{Claim, InteriorUseIndex};
pub use uses::{plan_interior, InteriorPlan, PlanInputs};

use crate::block_definitions::*;
use crate::element_processing::buildings::{cached_prop_block, BUILDING_PASSAGE_HEIGHT};
use crate::element_processing::subprocessor::buildings_loot::{themed_chest_loot, LootTheme};
use crate::floodfill_cache::CoordinateBitmap;
use crate::world_editor::WorldEditor;
use canvas::{bed, facing, mix, pick, seat, top_slab, wood_for, Canvas, Frame, Wood, DIRS};
use std::collections::HashSet;
use uses::Use;

/// Smallest home that gets a bed and a kitchen corner.
const MIN_COTTAGE_CELLS: usize = 45;

/// Everything the interior needs to know about one building.
pub struct InteriorRequest<'a> {
    pub footprint: &'a [(i32, i32)],
    /// Floor slab rows, measured like `start_y_offset`.
    pub floor_levels: &'a [i32],
    pub start_y_offset: i32,
    pub building_height: i32,
    pub abs_terrain_offset: i32,
    /// Interior walls are built of the outer wall's block, as they show through windows.
    pub wall_block: Block,
    pub plan: &'a InteriorPlan,
    pub abandoned: bool,
    pub passages: &'a CoordinateBitmap,
    /// Doors in the outer wall at ground level.
    pub entrances: &'a [Entry],
    /// Corners of the outline's bounding box.
    pub bounds: ((i32, i32), (i32, i32)),
    /// Smaller buildings standing on this one's floor, which furnish it themselves.
    pub claims: &'a [Claim],
    /// Blocks per metre, to compare storeys with claim heights.
    pub scale: f64,
    /// The slab between storeys, which a light may replace.
    pub floor_block: Block,
    pub seed: u64,
}

/// Per-storey settings shared by the furnishers.
struct FloorCtx {
    floor: usize,
    floors: usize,
    seed: u64,
    wood: Wood,
    /// Loot salt, per building.
    salt: u32,
    /// Corners of the area the house plans tile over.
    origin: (i32, i32),
    far: (i32, i32),
    /// Big enough for the tiled house plan rather than a cottage layout.
    homes_fit: bool,
}

pub fn generate_building_interior(editor: &mut WorldEditor, req: &InteriorRequest) {
    if req.footprint.is_empty() || req.floor_levels.is_empty() || req.plan.floors.is_empty() {
        return;
    }
    let abs = req.abs_terrain_offset;
    let ((min_x, min_z), (max_x, max_z)) = req.bounds;
    let floors = req.floor_levels.len();
    // Floor shared with a smaller building belongs to it up to that building's top.
    let claimed_to: Vec<f64> = req
        .footprint
        .iter()
        .map(|&(x, z)| {
            req.claims
                .iter()
                .filter(|claim| index::covers(&claim.ring, x, z))
                .map(|claim| claim.top_m)
                .fold(0.0, f64::max)
        })
        .collect();
    let own_at = |floor_rel: i32| -> Vec<(i32, i32)> {
        let floor_m = (floor_rel - req.start_y_offset) as f64 / req.scale.max(0.01);
        req.footprint
            .iter()
            .zip(&claimed_to)
            .filter(|(_, &top)| top <= floor_m + 1.0)
            .map(|(&cell, _)| cell)
            .collect()
    };
    let own = own_at(req.start_y_offset);
    if own.is_empty() {
        return;
    }
    let footprint: HashSet<(i32, i32)> = own.iter().copied().collect();
    let shaft = if floors >= 2 {
        pick_shaft(&own, &footprint, req.entrances, req.passages)
    } else {
        None
    };
    let passage_top = req.start_y_offset + BUILDING_PASSAGE_HEIGHT.min(req.building_height);
    let homes_fit = max_x - min_x + 1 >= 8 && max_z - min_z + 1 >= 8 && own.len() > 100;

    for (i, &floor_rel) in req.floor_levels.iter().enumerate() {
        let floor_y = floor_rel + abs;
        let top = match req.floor_levels.get(i + 1) {
            Some(next) => next - 1 + abs,
            None => req.start_y_offset + req.building_height + abs,
        };
        if top - floor_y < 2 {
            continue;
        }
        let ctx = FloorCtx {
            floor: i,
            floors,
            seed: req.seed ^ (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15),
            wood: wood_for(req.seed),
            salt: (min_x as u32).wrapping_mul(0x9E37_79B1) ^ (min_z as u32),
            origin: (min_x + 2, min_z + 2),
            far: (max_x - 2, max_z - 2),
            homes_fit,
        };
        // The ladder only climbs where this storey has room for it, never through the shell.
        let mut shaft_open = false;
        {
            let in_passage = floor_rel < passage_top;
            let skip = |x: i32, z: i32| in_passage && req.passages.contains(x, z);
            let storey = own_at(floor_rel);
            let mut c = Canvas::new(editor, &storey, floor_y, top, req.wall_block, &skip);
            if let Some((cell, n)) = shaft {
                shaft_open = c.walkable(cell.0, cell.1);
                c.keep(cell.0, cell.1);
                c.keep(cell.0 + n.0, cell.1 + n.1);
                // Upper storeys are reached by the ladder, the ground floor by its doors.
                if i > 0 || req.entrances.is_empty() {
                    c.add_start(cell);
                }
            }
            let units = req
                .plan
                .floors
                .get(i)
                .or(req.plan.floors.last())
                .cloned()
                .unwrap_or_default();
            let anchors: Vec<(i32, i32)> = if req.abandoned {
                vec![(min_x, min_z)]
            } else {
                units.iter().map(|u| u.anchor).collect()
            };
            if i == 0 {
                c.reserve_entrances(req.entrances);
            }
            let zones = c.split_nearest(&anchors);
            if i == 0 {
                c.add_entrances(req.entrances);
            }
            c.connect();
            c.clear_doorways();
            if req.abandoned {
                if homes_fit {
                    c.focus(zones[0]);
                    residential::tile_floor(&mut c, zones[0], &ctx, true);
                }
            } else {
                for (unit, zone) in units.iter().zip(zones) {
                    furnish(&mut c, zone, unit.use_, &ctx);
                }
                c.focus(0);
                c.light_up(req.floor_block, req.floor_levels.get(i + 1).is_some());
            }
        }
        // A ladder up through the ceiling to the next storey.
        if let (Some((cell, n)), Some(&next), true) =
            (shaft, req.floor_levels.get(i + 1), shaft_open)
        {
            let ladder =
                cached_prop_block(LADDER, &[("facing", facing(n)), ("waterlogged", "false")]);
            for y in floor_y + 1..=next + abs {
                editor.set_block_with_properties_absolute(
                    ladder.clone(),
                    cell.0,
                    y,
                    cell.1,
                    None,
                    Some(&[]),
                );
            }
        }
    }
}

fn furnish(c: &mut Canvas, zone: u16, use_: Use, ctx: &FloorCtx) {
    if c.area(zone) < 4 {
        return;
    }
    c.focus(zone);
    match use_ {
        Use::Home if ctx.homes_fit => residential::tile_floor(c, zone, ctx, false),
        // Smaller than this is a shed more often than a home.
        Use::Home if c.area(zone) >= MIN_COTTAGE_CELLS => cottage(c, zone, ctx),
        Use::Home => {}
        Use::Shop(goods) => commerce::shop(c, zone, goods, ctx),
        Use::Supermarket => commerce::supermarket(c, zone, ctx),
        Use::Food(kind) => commerce::eatery(c, zone, kind, ctx),
        Use::Office => commerce::office(c, zone, ctx),
        Use::Bank => commerce::bank(c, zone, ctx),
        Use::Workshop => commerce::workshop(c, zone, ctx),
        Use::School => civic::school(c, zone, ctx),
        Use::Kindergarten => civic::kindergarten(c, zone, ctx),
        Use::Library => civic::library(c, zone, ctx),
        Use::Clinic => civic::clinic(c, zone, ctx),
        Use::Hospital => civic::hospital(c, zone, ctx),
        Use::Hotel => civic::hotel(c, zone, ctx),
        Use::Museum => civic::museum(c, zone, ctx),
        Use::Station => civic::station(c, zone, ctx),
        Use::Worship(faith) => halls::worship(c, zone, faith, ctx),
        Use::SportsHall => halls::sports_hall(c, zone, ctx),
        Use::Gym => halls::gym(c, zone, ctx),
        Use::Auditorium => halls::auditorium(c, zone, ctx),
        Use::Warehouse => halls::warehouse(c, zone, ctx),
        Use::Factory => halls::factory(c, zone, ctx),
        Use::Barn => halls::barn(c, zone, ctx),
    }
}

/// A home too small for the house plan: bed, kitchen corner, table and chest.
fn cottage(c: &mut Canvas, zone: u16, ctx: &FloorCtx) {
    let Some(f) = c.frame(zone) else {
        return;
    };
    let mut walls = c.wall_cells(zone);
    // Beds at the back, the kitchen by the door.
    walls.sort_by_key(|&((x, z), _)| std::cmp::Reverse(f.local(x, z).1));
    let beds = if ctx.floor == 0 && ctx.floors > 1 {
        0
    } else {
        1 + (c.area(zone) / 60).min(2)
    };
    let mut slept = 0;
    for &((x, z), n) in &walls {
        if slept >= beds {
            break;
        }
        if bed(c, (x + n.0, z + n.1), (-n.0, -n.1), RED_BED_NORTH_HEAD) {
            slept += 1;
        }
    }
    if ctx.floor == 0 {
        let kitchen = [FURNACE, CRAFTING_TABLE, WATER_CAULDRON, BARREL];
        let mut k = 0;
        for &((x, z), n) in walls.iter().rev() {
            if k >= kitchen.len() {
                break;
            }
            let block = kitchen[k];
            let placed = if block == FURNACE {
                c.put_with(x, 1, z, canvas::facing_block(FURNACE, n))
            } else {
                c.put(x, 1, z, block)
            };
            if placed {
                k += 1;
            }
        }
        // Table in the middle of the room.
        let depth = c.depth(zone);
        if let Some((x, z)) = c
            .cells(zone)
            .into_iter()
            .filter(|&(x, z)| depth.get(x, z) >= 2 && c.is_free(x, z))
            .min_by_key(|&(x, z)| {
                let (u, v) = f.local(x, z);
                (u - f.width / 2).abs() + (v - f.depth / 2).abs()
            })
        {
            let (u, v) = f.local(x, z);
            table_set(c, &f, u, v, ctx.wood, &[(-1, 0), (1, 0)]);
        }
    }
    // A chest and a bookshelf on what wall is left.
    let mut extras = [true, true];
    for ((x, z), n) in c.wall_cells(zone) {
        if extras[0] && mix(x, z, ctx.seed).is_multiple_of(3) {
            extras[0] = !chest(c, x, z, LootTheme::Mixed, ctx.salt);
        } else if extras[1] && mix(x, z, ctx.seed) % 3 == 1 {
            extras[1] = !c.put_with(x, 1, z, book_shelf(n, mix(x, z, 9)));
        }
    }
}

/// Ladder spot: a corner against the outer wall, far from the entrance. Footprint
/// only, so tiles agree. Returns the cell and the direction into the room.
fn pick_shaft(
    footprint: &[(i32, i32)],
    set: &HashSet<(i32, i32)>,
    entrances: &[Entry],
    passages: &CoordinateBitmap,
) -> Option<((i32, i32), (i32, i32))> {
    type Shaft = ((i32, i32), (i32, i32));
    let mut best: Option<((bool, i32, u64), Shaft)> = None;
    for &(x, z) in footprint {
        if passages.contains(x, z) {
            continue;
        }
        let walls: Vec<(i32, i32)> = DIRS
            .iter()
            .copied()
            .filter(|d| !set.contains(&(x + d.0, z + d.1)))
            .collect();
        let Some(&d) = walls.first() else {
            continue;
        };
        let n = (-d.0, -d.1);
        if walls.len() > 2 || !set.contains(&(x + n.0, z + n.1)) {
            continue;
        }
        let far = entrances
            .iter()
            .map(|e| (e.cell.0 - x).abs() + (e.cell.1 - z).abs())
            .min()
            .unwrap_or(30)
            .min(30);
        if far < 4 {
            continue;
        }
        let key = (walls.len() == 2, far, mix(x, z, 0x01AD_DE25));
        if best.as_ref().is_none_or(|(k, _)| key > *k) {
            best = Some((key, ((x, z), n)));
        }
    }
    best.map(|(_, shaft)| shaft)
}

/// Where the entry meets the frame across the unit, or its middle without one.
fn entry_u(c: &Canvas, zone: u16, f: &Frame) -> i32 {
    c.entry(zone)
        .map(|e| f.local(e.cell.0, e.cell.1).0)
        .unwrap_or(f.width / 2)
}

/// A table at (`u`, `v`) with chairs at the offsets, placed only if a chair fits.
fn table_set(c: &mut Canvas, f: &Frame, u: i32, v: i32, wood: Wood, chairs: &[(i32, i32)]) -> bool {
    let (x, z) = f.world(u, v);
    if !c.is_free(x, z) {
        return false;
    }
    let seats: Vec<((i32, i32), (i32, i32))> = chairs
        .iter()
        .map(|&(du, dv)| (f.world(u + du, v + dv), f.dir(-du, -dv)))
        .filter(|&((sx, sz), _)| c.is_free(sx, sz))
        .collect();
    if seats.is_empty() {
        return false;
    }
    c.put_with(x, 1, z, top_slab(wood.slab));
    for ((sx, sz), look) in seats {
        c.put_with(sx, 1, sz, seat(wood.stairs, look));
    }
    true
}

/// A chest of loot standing on a free cell.
fn chest(c: &mut Canvas, x: i32, z: i32, theme: LootTheme, salt: u32) -> bool {
    if !c.is_free(x, z) {
        return false;
    }
    let y = c.floor_y + 1;
    c.editor
        .set_chest_with_items_absolute(x, y, z, themed_chest_loot(x, z, salt, theme));
    c.put(x, 1, z, CHEST)
}

/// A potted plant or a bush at (`u`, `v`) of the frame.
fn plant(c: &mut Canvas, f: &Frame, u: i32, v: i32, seed: u64) -> bool {
    let (x, z) = f.world(u, v);
    let h = mix(x, z, seed);
    c.put(
        x,
        1,
        z,
        pick(
            &[
                AZALEA,
                FLOWERING_AZALEA,
                POTTED_RED_TULIP,
                POTTED_BLUE_ORCHID,
            ],
            h,
        ),
    )
}

/// A chiseled bookshelf turned into the room, some of its slots filled.
fn book_shelf(front: (i32, i32), h: u64) -> crate::block_definitions::BlockWithProperties {
    const FILLS: [[&str; 6]; 4] = [
        ["true", "true", "false", "true", "true", "true"],
        ["true", "false", "true", "true", "true", "false"],
        ["false", "true", "true", "true", "false", "true"],
        ["true", "true", "true", "false", "true", "true"],
    ];
    let fill = FILLS[(h % FILLS.len() as u64) as usize];
    cached_prop_block(
        CHISELLED_BOOKSHELF,
        &[
            ("facing", facing(front)),
            ("slot_0_occupied", fill[0]),
            ("slot_1_occupied", fill[1]),
            ("slot_2_occupied", fill[2]),
            ("slot_3_occupied", fill[3]),
            ("slot_4_occupied", fill[4]),
            ("slot_5_occupied", fill[5]),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Args;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_facade::BuildingContext;
    use crate::element_processing::building_test_support::{rect_way, tag_map, test_editor};
    use crate::element_processing::buildings::generate_buildings;
    use crate::floodfill_cache::FloodFillCache;
    use crate::osm_parser::{ProcessedElement, ProcessedNode, ProcessedWay};
    use clap::Parser;
    use fnv::FnvHashMap;

    fn interior_args() -> Args {
        Args::parse_from([
            "arnis",
            "--bbox",
            "1,2,3,4",
            "--mode",
            "geo-only",
            "--ground-level",
            "0",
            "--interior",
        ])
    }

    fn poi(id: u64, x: i32, z: i32, tags: &[(&str, &str)]) -> ProcessedElement {
        ProcessedElement::Node(ProcessedNode {
            id,
            tags: tag_map(tags),
            x,
            z,
        })
    }

    /// Builds every building among `elements` with interiors on.
    fn build<'a>(xz: &'a XZBBox, elements: &[ProcessedElement]) -> WorldEditor<'a> {
        let mut editor = test_editor(xz);
        let cache = FloodFillCache::new();
        let passages = CoordinateBitmap::new_empty();
        let road = CoordinateBitmap::new(xz);
        let footprints = CoordinateBitmap::new(xz);
        let groups: FnvHashMap<u64, Vec<u64>> = FnvHashMap::default();
        let index = InteriorUseIndex::build(elements, xz);
        let ctx = BuildingContext {
            flood_fill_cache: &cache,
            building_passages: &passages,
            road_mask: &road,
            building_footprints: &footprints,
            group_members: &groups,
            interior_uses: &index,
        };
        let args = interior_args();
        for e in elements {
            if let ProcessedElement::Way(way) = e {
                if way.tags.contains_key("building") {
                    generate_buildings(&mut editor, way, &args, None, None, &ctx, way.id);
                }
            }
        }
        editor
    }

    fn bbox(way: &ProcessedWay) -> (i32, i32, i32, i32) {
        let xs = way.nodes.iter().map(|n| n.x);
        let zs = way.nodes.iter().map(|n| n.z);
        (
            xs.clone().min().unwrap(),
            xs.max().unwrap(),
            zs.clone().min().unwrap(),
            zs.max().unwrap(),
        )
    }

    /// Every block one row up inside the outline.
    fn blocks_at(editor: &WorldEditor, way: &ProcessedWay, y: i32) -> Vec<Block> {
        let (x0, x1, z0, z1) = bbox(way);
        let mut out = Vec::new();
        for z in z0 + 1..z1 {
            for x in x0 + 1..x1 {
                if let Some(b) = editor.get_block_absolute(x, y, z) {
                    out.push(b);
                }
            }
        }
        out
    }

    fn has(blocks: &[Block], wanted: Block) -> bool {
        blocks.iter().any(|b| b.id() == wanted.id())
    }

    fn count_named(blocks: &[Block], suffix: &str) -> usize {
        blocks.iter().filter(|b| b.name().ends_with(suffix)).count()
    }

    /// Rows holding a floor slab across most of the footprint.
    fn floor_rows(editor: &WorldEditor, way: &ProcessedWay, max_y: i32) -> Vec<i32> {
        let (x0, x1, z0, z1) = bbox(way);
        let ring: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
        let cells: Vec<(i32, i32)> = (z0 + 1..z1)
            .flat_map(|z| (x0 + 1..x1).map(move |x| (x, z)))
            .filter(|&(x, z)| index::covers(&ring, x, z))
            .collect();
        (0..max_y)
            .filter(|&y| {
                let solid = cells
                    .iter()
                    .filter(|&&(x, z)| editor.get_block_absolute(x, y, z).is_some())
                    .count();
                solid * 10 >= cells.len() * 8
            })
            .collect()
    }

    /// Top-down picture of one row, for eyeballing a layout.
    fn picture(editor: &WorldEditor, way: &ProcessedWay, y: i32) -> String {
        let (x0, x1, z0, z1) = bbox(way);
        let wall = editor
            .get_block_absolute(way.nodes[0].x, 3, way.nodes[0].z)
            .map(|b| b.id());
        let mut s = String::new();
        for z in z0..=z1 {
            for x in x0..=x1 {
                let ch = match editor.get_block_absolute(x, y, z) {
                    None => ' ',
                    Some(b) => {
                        let name = b.name();
                        if Some(b.id()) == wall {
                            '#'
                        } else if name.ends_with("_stairs") {
                            'h'
                        } else if name.ends_with("_slab") {
                            '_'
                        } else if name.ends_with("_carpet") {
                            '.'
                        } else if name.ends_with("_door") {
                            'D'
                        } else if name.ends_with("_bed") {
                            'B'
                        } else if name.contains("glass") {
                            'g'
                        } else if name == "ladder" {
                            '^'
                        } else {
                            name.chars().next().unwrap().to_ascii_uppercase()
                        }
                    }
                };
                s.push(ch);
            }
            s.push('\n');
        }
        s
    }

    fn sized(id: u64, w: i32, d: i32, tags: &[(&str, &str)]) -> ProcessedWay {
        rect_way(id, 10, 10, 10 + w, 10 + d, tags)
    }

    /// Closed outline through the given corners.
    fn poly(id: u64, corners: &[(i32, i32)], tags: &[(&str, &str)]) -> ProcessedWay {
        let mut nodes: Vec<ProcessedNode> = corners
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: id * 100 + i as u64,
                tags: Default::default(),
                x,
                z,
            })
            .collect();
        nodes.push(nodes[0].clone());
        ProcessedWay {
            id,
            nodes,
            tags: tag_map(tags),
        }
    }

    /// Outlines that are not rectangles.
    fn odd_shapes() -> Vec<(&'static str, Vec<(i32, i32)>)> {
        vec![
            (
                "L",
                vec![(10, 10), (40, 10), (40, 22), (24, 22), (24, 40), (10, 40)],
            ),
            (
                "U",
                vec![
                    (10, 10),
                    (46, 10),
                    (46, 36),
                    (36, 36),
                    (36, 20),
                    (20, 20),
                    (20, 36),
                    (10, 36),
                ],
            ),
            ("diamond", vec![(35, 12), (58, 35), (35, 58), (12, 35)]),
            ("triangle", vec![(10, 10), (50, 10), (10, 44)]),
            (
                "hexagon",
                vec![(20, 10), (40, 10), (50, 27), (40, 44), (20, 44), (10, 27)],
            ),
        ]
    }

    /// Uses worth trying on every shape.
    fn shape_uses() -> Vec<Vec<(&'static str, &'static str)>> {
        vec![
            vec![
                ("building", "retail"),
                ("shop", "clothes"),
                ("building:levels", "1"),
            ],
            vec![("building", "school"), ("building:levels", "2")],
            vec![("building", "office"), ("building:levels", "2")],
            vec![("building", "apartments"), ("building:levels", "3")],
            vec![("building", "church")],
            vec![
                ("building", "yes"),
                ("amenity", "restaurant"),
                ("building:levels", "1"),
            ],
            vec![("building", "hospital"), ("building:levels", "2")],
            vec![("building", "warehouse")],
        ]
    }

    fn print_case(name: &str, way: ProcessedWay, mut extra: Vec<ProcessedElement>) {
        let xz = XZBBox::rect_from_xz_lengths(80.0, 80.0).unwrap();
        extra.insert(0, ProcessedElement::Way(way.clone()));
        let editor = build(&xz, &extra);
        let rows = floor_rows(&editor, &way, 40);
        println!("=== {name} (floor rows {rows:?})");
        let (x0, x1, z0, z1) = bbox(&way);
        for &y in &rows {
            let open = (z0 + 1..z1)
                .flat_map(|z| (x0 + 1..x1).map(move |x| (x, z)))
                .any(|(x, z)| editor.get_block_absolute(x, y + 1, z).is_none());
            if open {
                println!("--- y {}", y + 1);
                print!("{}", picture(&editor, &way, y + 1));
            }
        }
    }

    #[test]
    #[ignore = "prints layouts for review: cargo test print_layouts -- --ignored --nocapture"]
    fn print_layouts() {
        let only = std::env::var("LAYOUT").ok();
        let cases: Vec<(&str, ProcessedWay, Vec<ProcessedElement>)> = vec![
            (
                "bakery in flats",
                sized(
                    1,
                    16,
                    12,
                    &[("building", "apartments"), ("building:levels", "3")],
                ),
                vec![poi(900, 14, 14, &[("shop", "bakery")])],
            ),
            (
                "supermarket",
                sized(
                    2,
                    30,
                    24,
                    &[("building", "retail"), ("shop", "supermarket")],
                ),
                vec![],
            ),
            (
                "restaurant",
                sized(
                    3,
                    18,
                    16,
                    &[
                        ("building", "yes"),
                        ("amenity", "restaurant"),
                        ("building:levels", "1"),
                    ],
                ),
                vec![],
            ),
            (
                "cafe",
                sized(
                    4,
                    10,
                    9,
                    &[
                        ("building", "yes"),
                        ("amenity", "cafe"),
                        ("building:levels", "1"),
                    ],
                ),
                vec![],
            ),
            (
                "office",
                sized(
                    5,
                    24,
                    18,
                    &[("building", "office"), ("building:levels", "2")],
                ),
                vec![],
            ),
            (
                "school",
                sized(
                    6,
                    34,
                    16,
                    &[("building", "school"), ("building:levels", "2")],
                ),
                vec![],
            ),
            (
                "church",
                sized(7, 30, 14, &[("building", "church")]),
                vec![],
            ),
            (
                "mosque",
                sized(8, 20, 20, &[("building", "mosque"), ("religion", "muslim")]),
                vec![],
            ),
            (
                "sports hall",
                sized(9, 36, 22, &[("building", "sports_hall")]),
                vec![],
            ),
            (
                "warehouse",
                sized(10, 30, 20, &[("building", "warehouse")]),
                vec![],
            ),
            (
                "factory",
                sized(11, 30, 18, &[("building", "industrial")]),
                vec![],
            ),
            ("barn", sized(12, 22, 12, &[("building", "barn")]), vec![]),
            (
                "hotel",
                sized(
                    13,
                    26,
                    16,
                    &[("building", "hotel"), ("building:levels", "3")],
                ),
                vec![],
            ),
            (
                "hospital",
                sized(
                    14,
                    30,
                    18,
                    &[("building", "hospital"), ("building:levels", "2")],
                ),
                vec![],
            ),
            (
                "library",
                sized(
                    15,
                    20,
                    16,
                    &[
                        ("building", "yes"),
                        ("amenity", "library"),
                        ("building:levels", "1"),
                    ],
                ),
                vec![],
            ),
            (
                "cinema",
                sized(16, 26, 16, &[("building", "yes"), ("amenity", "cinema")]),
                vec![],
            ),
            (
                "bank",
                sized(
                    17,
                    14,
                    12,
                    &[
                        ("building", "yes"),
                        ("amenity", "bank"),
                        ("building:levels", "1"),
                    ],
                ),
                vec![],
            ),
            (
                "kindergarten",
                sized(
                    18,
                    16,
                    14,
                    &[("building", "kindergarten"), ("building:levels", "1")],
                ),
                vec![],
            ),
            (
                "three shops",
                sized(19, 36, 12, &[("building", "yes"), ("building:levels", "2")]),
                vec![
                    poi(901, 14, 15, &[("shop", "clothes")]),
                    poi(902, 28, 15, &[("amenity", "cafe")]),
                    poi(903, 40, 15, &[("shop", "chemist")]),
                ],
            ),
            (
                "cottage",
                sized(20, 9, 8, &[("building", "house"), ("building:levels", "2")]),
                vec![],
            ),
            (
                "museum",
                sized(
                    21,
                    22,
                    16,
                    &[("building", "museum"), ("building:levels", "1")],
                ),
                vec![],
            ),
            (
                "station",
                sized(22, 30, 14, &[("building", "train_station")]),
                vec![],
            ),
            (
                "salon",
                sized(
                    23,
                    10,
                    8,
                    &[
                        ("building", "yes"),
                        ("shop", "hairdresser"),
                        ("building:levels", "1"),
                    ],
                ),
                vec![],
            ),
            (
                "clinic",
                sized(
                    24,
                    16,
                    14,
                    &[
                        ("building", "yes"),
                        ("amenity", "doctors"),
                        ("building:levels", "1"),
                    ],
                ),
                vec![],
            ),
        ];
        for (name, way, extra) in cases {
            if only.as_deref().is_some_and(|o| !name.contains(o)) {
                continue;
            }
            print_case(name, way, extra);
        }
        for (shape, corners) in odd_shapes() {
            for (i, tags) in shape_uses().into_iter().enumerate() {
                let kind = tags.iter().rev().find(|t| t.0 != "building:levels");
                let name = format!("{shape} {}", kind.map_or("", |t| t.1));
                if only.as_deref().is_some_and(|o| !name.contains(o)) {
                    continue;
                }
                print_case(&name, poly(40 + i as u64, &corners, &tags), vec![]);
            }
        }
    }

    fn args_without_interior() -> Args {
        Args::parse_from([
            "arnis",
            "--bbox",
            "1,2,3,4",
            "--mode",
            "geo-only",
            "--ground-level",
            "0",
        ])
    }

    fn one(way: ProcessedWay, pois: Vec<ProcessedElement>) -> (XZBBox, Vec<ProcessedElement>) {
        let mut elements = vec![ProcessedElement::Way(way)];
        elements.extend(pois);
        (XZBBox::rect_from_xz_lengths(90.0, 90.0).unwrap(), elements)
    }

    fn way_of(elements: &[ProcessedElement]) -> &ProcessedWay {
        match &elements[0] {
            ProcessedElement::Way(w) => w,
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_bakery_takes_the_ground_floor_and_homes_stay_above() {
        let (xz, elements) = one(
            sized(
                1,
                16,
                12,
                &[("building", "apartments"), ("building:levels", "3")],
            ),
            vec![poi(900, 14, 14, &[("shop", "bakery")])],
        );
        let editor = build(&xz, &elements);
        let way = way_of(&elements);
        let rows = floor_rows(&editor, way, 12);
        assert!(rows.len() >= 3, "three storeys: {rows:?}");
        let ground = [blocks_at(&editor, way, 1), blocks_at(&editor, way, 2)].concat();
        assert!(has(&ground, CAKE), "cakes on the bakery counter");
        assert!(has(&ground, HAY_BALE), "bread on the bakery shelves");
        let upper = blocks_at(&editor, way, rows[1] + 1);
        assert!(!has(&upper, CAKE) && !has(&upper, HAY_BALE));
        assert!(
            upper
                .iter()
                .any(|b| [CRAFTING_TABLE, FURNACE, BOOKSHELF, CAULDRON, ANVIL].contains(b)),
            "flats furnish the floor above"
        );
    }

    #[test]
    fn nothing_is_furnished_with_interiors_off() {
        let way = sized(
            2,
            30,
            24,
            &[("building", "retail"), ("shop", "supermarket")],
        );
        let xz = XZBBox::rect_from_xz_lengths(90.0, 90.0).unwrap();
        let mut editor = test_editor(&xz);
        let cache = FloodFillCache::new();
        let passages = CoordinateBitmap::new_empty();
        let road = CoordinateBitmap::new(&xz);
        let footprints = CoordinateBitmap::new(&xz);
        let groups: FnvHashMap<u64, Vec<u64>> = FnvHashMap::default();
        let ctx = BuildingContext {
            flood_fill_cache: &cache,
            building_passages: &passages,
            road_mask: &road,
            building_footprints: &footprints,
            group_members: &groups,
            interior_uses: InteriorUseIndex::empty(),
        };
        generate_buildings(
            &mut editor,
            &way,
            &args_without_interior(),
            None,
            None,
            &ctx,
            2,
        );
        let inside = blocks_at(&editor, &way, 1);
        assert!(
            inside.is_empty(),
            "an empty shell: {:?}",
            &inside[..inside.len().min(5)]
        );
    }

    #[test]
    fn a_supermarket_has_checkouts_fridges_and_aisles() {
        let (xz, elements) = one(
            sized(
                2,
                30,
                24,
                &[("building", "retail"), ("shop", "supermarket")],
            ),
            vec![],
        );
        let editor = build(&xz, &elements);
        let way = way_of(&elements);
        let low = blocks_at(&editor, way, 1);
        let high = blocks_at(&editor, way, 2);
        assert!(has(&high, DAYLIGHT_DETECTOR), "tills on the checkouts");
        assert!(has(&high, LIGHT_BLUE_STAINED_GLASS), "fridge fronts");
        assert!(
            low.iter().filter(|b| **b == BARREL).count() > 30,
            "stocked aisles"
        );
    }

    #[test]
    fn a_church_is_one_room_with_pews_and_an_altar() {
        let (xz, elements) = one(sized(7, 30, 14, &[("building", "church")]), vec![]);
        let editor = build(&xz, &elements);
        let way = way_of(&elements);
        let rows = floor_rows(&editor, way, 30);
        let low = blocks_at(&editor, way, 1);
        assert!(count_named(&low, "_stairs") > 40, "rows of pews");
        assert!(has(&low, CHISELED_QUARTZ_BLOCK), "an altar");
        // No storey slab between the floor and the roof.
        let roof = rows.iter().copied().find(|&y| y > 0).unwrap_or(30);
        assert!(roof >= 9, "the nave is open up to the roof: {rows:?}");
    }

    #[test]
    fn upper_floors_are_reached_by_ladder() {
        let (xz, elements) = one(
            sized(
                5,
                24,
                18,
                &[("building", "office"), ("building:levels", "3")],
            ),
            vec![],
        );
        let editor = build(&xz, &elements);
        let way = way_of(&elements);
        let rows = floor_rows(&editor, way, 20);
        let (x0, x1, z0, z1) = bbox(way);
        let ladder_through_ceiling = (z0..=z1)
            .flat_map(|z| (x0..=x1).map(move |x| (x, z)))
            .any(|(x, z)| editor.get_block_absolute(x, rows[1], z) == Some(LADDER));
        assert!(
            ladder_through_ceiling,
            "a ladder climbs through the first ceiling"
        );
    }

    #[test]
    fn shops_sharing_a_floor_get_walls_and_doors_between_them() {
        let (xz, elements) = one(
            sized(19, 36, 12, &[("building", "yes"), ("building:levels", "2")]),
            vec![
                poi(901, 14, 15, &[("shop", "clothes")]),
                poi(902, 28, 15, &[("amenity", "cafe")]),
                poi(903, 40, 15, &[("shop", "chemist")]),
            ],
        );
        let editor = build(&xz, &elements);
        let way = way_of(&elements);
        let low = blocks_at(&editor, way, 1);
        assert!(
            count_named(&low, "_door") >= 2,
            "a door between each pair of units"
        );
        assert!(
            has(&low, RED_WOOL) || has(&low, BLUE_WOOL),
            "clothes on the shelves"
        );
        assert!(has(&low, QUARTZ_BLOCK), "the chemist's counter");
    }

    #[test]
    fn a_school_has_classrooms_off_a_corridor() {
        let (xz, elements) = one(
            sized(
                6,
                34,
                16,
                &[("building", "school"), ("building:levels", "2")],
            ),
            vec![],
        );
        let editor = build(&xz, &elements);
        let way = way_of(&elements);
        let low = blocks_at(&editor, way, 1);
        let boards = blocks_at(&editor, way, 2);
        // Three rooms off the corridor; the one the front door opens into is the lobby.
        assert!(
            count_named(&low, "_door") >= 2,
            "a door into each classroom"
        );
        assert!(
            has(&boards, GREEN_CONCRETE),
            "boards on the classroom walls"
        );
        assert!(has(&low, LECTERN), "a teacher's lectern");
    }

    #[test]
    fn the_way_in_stays_clear() {
        for (id, tags) in [
            (2, vec![("building", "retail"), ("shop", "supermarket")]),
            (3, vec![("building", "yes"), ("amenity", "restaurant")]),
            (10, vec![("building", "warehouse")]),
            (16, vec![("building", "yes"), ("amenity", "cinema")]),
        ] {
            let (xz, elements) = one(sized(id, 24, 18, &tags), vec![]);
            let editor = build(&xz, &elements);
            let way = way_of(&elements);
            let (x0, x1, z0, z1) = bbox(way);
            let mut doors = 0;
            for (x, z) in (z0..=z1).flat_map(|z| (x0..=x1).map(move |x| (x, z))) {
                let on_rim = x == x0 || x == x1 || z == z0 || z == z1;
                let is_door = editor
                    .get_block_absolute(x, 1, z)
                    .is_some_and(|b| b.name().ends_with("_door"));
                if !on_rim || !is_door {
                    continue;
                }
                doors += 1;
                let inward = if z == z1 {
                    (0, -1)
                } else if z == z0 {
                    (0, 1)
                } else if x == x0 {
                    (1, 0)
                } else {
                    (-1, 0)
                };
                for step in 1..=2 {
                    let (cx, cz) = (x + inward.0 * step, z + inward.1 * step);
                    assert!(
                        editor.get_block_absolute(cx, 1, cz).is_none(),
                        "{tags:?}: furniture blocks the door at ({x}, {z})"
                    );
                }
            }
            assert!(doors > 0, "{tags:?} has a door");
        }
    }

    /// Tiles build the parts of a building they overlap on their own, seeing only what
    /// lies in their own bounds. Each tile must build its part exactly as one editor
    /// building the whole would.
    #[test]
    fn a_tile_furnishes_its_part_as_the_whole_world_would() {
        let cases = [
            (
                sized(
                    2,
                    70,
                    24,
                    &[("building", "retail"), ("shop", "supermarket")],
                ),
                vec![],
            ),
            (
                sized(
                    6,
                    70,
                    16,
                    &[("building", "school"), ("building:levels", "2")],
                ),
                vec![],
            ),
            (
                sized(19, 70, 12, &[("building", "yes"), ("building:levels", "2")]),
                vec![
                    poi(901, 14, 15, &[("shop", "clothes")]),
                    poi(902, 44, 15, &[("amenity", "restaurant")]),
                    poi(903, 70, 15, &[("shop", "hardware")]),
                ],
            ),
            (sized(10, 70, 20, &[("building", "warehouse")]), vec![]),
        ];
        for (way, pois) in cases {
            let mut elements = vec![ProcessedElement::Way(way.clone())];
            elements.extend(pois);
            let world = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
            let whole = build(&world, &elements);
            // A tile ending part way along the building.
            let tile = XZBBox::rect_from_min_max(0, 0, 52, 99).unwrap();
            let part = build(&tile, &elements);
            let (_, _, z0, z1) = bbox(&way);
            for x in 0..=40 {
                for z in z0..=z1 {
                    for y in 0..12 {
                        assert_eq!(
                            whole.get_block_absolute(x, y, z),
                            part.get_block_absolute(x, y, z),
                            "{:?} differs at ({x}, {y}, {z})",
                            way.tags.get("building")
                        );
                    }
                }
            }
        }
    }

    /// Share of open floor cells at standing height with a light above them nearby.
    fn lit_share(editor: &WorldEditor, way: &ProcessedWay, floor: i32, top: i32) -> f64 {
        let (x0, x1, z0, z1) = bbox(way);
        let lights = [GLOWSTONE, LANTERN, SEA_LANTERN];
        let mut open = 0;
        let mut lit = 0;
        for z in z0 + 1..z1 {
            for x in x0 + 1..x1 {
                if editor.get_block_absolute(x, floor + 1, z).is_some()
                    || editor.get_block_absolute(x, floor, z).is_none()
                {
                    continue;
                }
                open += 1;
                let near = (-3..=3).any(|dx| {
                    (-3..=3).any(|dz| {
                        (floor + 1..=top + 1).any(|y| {
                            editor
                                .get_block_absolute(x + dx, y, z + dz)
                                .is_some_and(|b| lights.contains(&b))
                        })
                    })
                });
                if near {
                    lit += 1;
                }
            }
        }
        lit as f64 / open.max(1) as f64
    }

    #[test]
    fn single_storey_rooms_and_top_floors_are_lit() {
        for (id, tags) in [
            (
                2,
                vec![
                    ("building", "retail"),
                    ("shop", "supermarket"),
                    ("building:levels", "1"),
                ],
            ),
            (
                24,
                vec![
                    ("building", "yes"),
                    ("amenity", "doctors"),
                    ("building:levels", "1"),
                ],
            ),
            (5, vec![("building", "office"), ("building:levels", "3")]),
        ] {
            let (xz, elements) = one(sized(id, 24, 18, &tags), vec![]);
            let editor = build(&xz, &elements);
            let way = way_of(&elements);
            let rows = floor_rows(&editor, way, 30);
            // Every storey up to the roof.
            for pair in rows.windows(2) {
                let (floor, ceiling) = (pair[0], pair[1]);
                if ceiling - floor < 3 {
                    continue;
                }
                let share = lit_share(&editor, way, floor, ceiling - 1);
                assert!(
                    share > 0.95,
                    "{tags:?}: storey at {floor} is lit on {:.0}% of its floor",
                    share * 100.0
                );
            }
        }
    }

    #[test]
    fn tall_halls_hang_lamps_low_enough_to_light_the_floor() {
        let (xz, elements) = one(sized(10, 30, 20, &[("building", "warehouse")]), vec![]);
        let editor = build(&xz, &elements);
        let way = way_of(&elements);
        let low = [blocks_at(&editor, way, 3), blocks_at(&editor, way, 4)].concat();
        assert!(has(&low, LANTERN), "lamps hang down into the hall");
    }

    #[test]
    fn shops_sharing_a_building_each_get_a_street_door() {
        let (xz, elements) = one(
            sized(19, 36, 12, &[("building", "yes"), ("building:levels", "2")]),
            vec![
                poi(901, 14, 15, &[("shop", "clothes")]),
                poi(902, 28, 15, &[("amenity", "cafe")]),
                poi(903, 40, 15, &[("shop", "chemist")]),
            ],
        );
        let editor = build(&xz, &elements);
        let way = way_of(&elements);
        let (x0, x1, z0, z1) = bbox(way);
        let outside_doors = (z0..=z1)
            .flat_map(|z| (x0..=x1).map(move |x| (x, z)))
            .filter(|&(x, z)| x == x0 || x == x1 || z == z0 || z == z1)
            .filter(|&(x, z)| {
                editor
                    .get_block_absolute(x, 1, z)
                    .is_some_and(|b| b.name().ends_with("_door"))
            })
            .count();
        assert!(
            outside_doors >= 3,
            "one door per shop, found {outside_doors}"
        );
    }

    #[test]
    fn a_building_standing_inside_another_keeps_its_own_floor() {
        // A cafe pavilion inside a supermarket hall: the hall's shelves stay out of it.
        let hall = sized(
            2,
            40,
            30,
            &[
                ("building", "retail"),
                ("shop", "supermarket"),
                ("building:levels", "1"),
            ],
        );
        let cafe = rect_way(
            3,
            30,
            20,
            40,
            28,
            &[
                ("building", "yes"),
                ("amenity", "cafe"),
                ("building:levels", "1"),
            ],
        );
        let xz = XZBBox::rect_from_xz_lengths(90.0, 90.0).unwrap();
        let elements = vec![ProcessedElement::Way(hall), ProcessedElement::Way(cafe)];
        let editor = build(&xz, &elements);
        let stock = [
            BARREL,
            HAY_BALE,
            MELON,
            PUMPKIN,
            WHITE_CONCRETE,
            DAYLIGHT_DETECTOR,
        ];
        for x in 30..=40 {
            for z in 20..=28 {
                for y in 1..=2 {
                    let b = editor.get_block_absolute(x, y, z);
                    assert!(
                        !b.is_some_and(|b| stock.contains(&b)),
                        "supermarket stock at ({x}, {y}, {z}) inside the cafe"
                    );
                }
            }
        }
    }

    #[test]
    fn odd_shapes_are_furnished_inside_their_outline() {
        for (shape, corners) in odd_shapes() {
            for (i, tags) in shape_uses().into_iter().enumerate() {
                let way = poly(40 + i as u64, &corners, &tags);
                let ring: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
                let (xz, elements) = one(way, vec![]);
                let editor = build(&xz, &elements);
                let way = way_of(&elements);
                let (x0, x1, z0, z1) = bbox(way);
                let mut furnished = 0;
                for z in z0 - 2..=z1 + 2 {
                    for x in x0 - 2..=x1 + 2 {
                        let Some(b) = editor.get_block_absolute(x, 1, z) else {
                            continue;
                        };
                        let inside = index::covers(&ring, x, z);
                        let name = b.name();
                        let furniture = name.ends_with("_stairs")
                            || name.ends_with("_wool")
                            || name.ends_with("_slab")
                            || name.ends_with("_bed")
                            || name.ends_with("_carpet")
                            || [BARREL, CHEST, BOOKSHELF, CHISELLED_BOOKSHELF, LECTERN]
                                .contains(&b);
                        if furniture {
                            furnished += 1;
                            assert!(
                                inside,
                                "{shape} {tags:?}: {name} outside the outline at ({x}, {z})"
                            );
                        }
                    }
                }
                assert!(furnished > 0, "{shape} {tags:?} is furnished");
            }
        }
    }

    #[test]
    fn odd_shapes_build_the_same_in_every_tile() {
        for (shape, corners) in odd_shapes() {
            for (i, tags) in shape_uses().into_iter().enumerate() {
                let way = poly(40 + i as u64, &corners, &tags);
                let elements = vec![ProcessedElement::Way(way)];
                let world = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
                let whole = build(&world, &elements);
                let tile = XZBBox::rect_from_min_max(0, 0, 99, 34).unwrap();
                let part = build(&tile, &elements);
                for x in 0..100 {
                    for z in 0..=26 {
                        for y in 0..14 {
                            assert_eq!(
                                whole.get_block_absolute(x, y, z),
                                part.get_block_absolute(x, y, z),
                                "{shape} {tags:?} differs at ({x}, {y}, {z})"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_hotel_lobby_keeps_a_restaurant_at_the_back() {
        let (xz, elements) = one(
            sized(
                13,
                34,
                24,
                &[("building", "hotel"), ("building:levels", "3")],
            ),
            vec![],
        );
        let editor = build(&xz, &elements);
        let way = way_of(&elements);
        let (x0, x1, z0, z1) = bbox(way);
        let seats = |zs: std::ops::Range<i32>| {
            zs.flat_map(|z| (x0 + 1..x1).map(move |x| (x, z)))
                .filter(|&(x, z)| {
                    editor
                        .get_block_absolute(x, 1, z)
                        .is_some_and(|b| b.name().ends_with("_stairs"))
                })
                .count()
        };
        let mid = (z0 + z1) / 2;
        let (north, south) = (seats(z0 + 1..mid), seats(mid..z1));
        assert!(
            north > 0 && south > 0,
            "seats in lobby and restaurant: {north} / {south}"
        );
    }

    #[test]
    fn utility_buildings_stay_empty() {
        for kind in ["toilets", "bunker", "container"] {
            let (xz, elements) = one(sized(30, 9, 8, &[("building", kind)]), vec![]);
            let editor = build(&xz, &elements);
            let inside = blocks_at(&editor, way_of(&elements), 1);
            assert!(
                !has(&inside, RED_BED_NORTH_HEAD) && !has(&inside, FURNACE),
                "{kind}"
            );
        }
    }

    #[test]
    fn a_pavilion_claims_only_the_storeys_it_reaches() {
        let hall = sized(
            2,
            40,
            30,
            &[("building", "retail"), ("building:levels", "3")],
        );
        let cafe = rect_way(
            3,
            30,
            20,
            40,
            28,
            &[
                ("building", "yes"),
                ("amenity", "cafe"),
                ("building:levels", "1"),
            ],
        );
        let xz = XZBBox::rect_from_xz_lengths(90.0, 90.0).unwrap();
        let elements = vec![
            ProcessedElement::Way(hall.clone()),
            ProcessedElement::Way(cafe),
        ];
        let editor = build(&xz, &elements);
        let rows = floor_rows(&editor, &hall, 20);
        let upper = rows[1] + 1;
        let stock = [BARREL, HAY_BALE, PUMPKIN, COMPOSTER];
        let over_cafe = (31..40)
            .flat_map(|x| (21..28).map(move |z| (x, z)))
            .any(|(x, z)| {
                editor
                    .get_block_absolute(x, upper, z)
                    .is_some_and(|b| stock.contains(&b))
            });
        assert!(
            over_cafe,
            "the hall furnishes its upper floor above the pavilion"
        );
    }

    #[test]
    fn flat_roofs_keep_their_lights_inside() {
        // Heights that leave a low top storey under a roof laid in the floor block.
        let mut id = 40;
        for height in ["7", "8", "12", "13"] {
            for building in ["office", "apartments", "yes"] {
                id += 1;
                let tags = [("building", building), ("height", height)];
                let (xz, elements) = one(sized(id, 24, 18, &tags), vec![]);
                let editor = build(&xz, &elements);
                let way = way_of(&elements);
                let roof = *floor_rows(&editor, way, 20).last().unwrap();
                assert!(
                    !has(&blocks_at(&editor, way, roof), GLOWSTONE),
                    "{tags:?}: glowstone in the roof at {roof}"
                );
            }
        }
    }
}
