//! Allotment gardens: hedged plots in pairs between paths, with beds, a fruit tree and a shed.

use crate::block_definitions::*;
use crate::element_processing::area_layout::{
    bounds, cell_hash, centroid, inside, point_in_rings, AreaMask, Frame,
};
use crate::element_processing::connected_blocks::connected_fence;
use crate::element_processing::landuse::ground_is_protected;
use crate::element_processing::plantations::Fruit;
use crate::floodfill_cache::{in_hole, BuildingFootprintBitmap, Holes, RoadMaskBitmap};
use crate::land_cover::coord_hash;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::HashMap;

const CROPS: [Block; 8] = [
    WHEAT,
    CARROTS,
    POTATOES,
    BEETROOTS,
    WHEAT_YOUNG,
    CARROTS_YOUNG,
    POTATOES_YOUNG,
    BEETROOTS_YOUNG,
];

/// The colony's plot grid in the frame of its outline.
struct Colony {
    id: u64,
    frame: Frame,
    mask: AreaMask,
    /// Plot width along the paths, plot depth from the path, path width.
    width: f64,
    depth: f64,
    path: f64,
    /// Paths and sheds are only invented where the map has none.
    paths: bool,
    sheds: bool,
}

/// What one column of a plot holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    Path,
    Hedge,
    Fence,
    Gate,
    Walk,
    Lawn,
    Flowers,
    Bed(Block),
    Shed { wall: bool },
    Compost,
    Barrel,
}

/// One plot: its key and its column in plot coordinates, `la` along the path and `b` from it.
struct PlotAt {
    key: (i64, i64, i64),
    la: f64,
    b: f64,
}

impl Colony {
    fn period(&self) -> f64 {
        self.path + 2.0 * self.depth
    }

    /// The plot a column lies in, or None on the path.
    fn plot_at(&self, x: i32, z: i32) -> Option<PlotAt> {
        let (a, c) = self.frame.local(x, z);
        let p = self.period();
        let band = (c / p).floor();
        let cc = c - band * p;
        if cc < self.path {
            return None;
        }
        // Two plots back to back between paths; the far one faces the next path.
        let (side, b) = if cc < self.path + self.depth {
            (0, cc - self.path)
        } else {
            (1, p - cc)
        };
        let i = (a / self.width).floor();
        Some(PlotAt {
            key: (band as i64, i as i64, side),
            la: a - i * self.width,
            b,
        })
    }

    fn hash(&self, key: (i64, i64, i64)) -> u64 {
        cell_hash(self.id ^ key.2 as u64, key.0, key.1)
    }

    /// Shed rectangle in plot coordinates: back corner, away from the gate.
    fn shed(&self, h: u64) -> (f64, f64, f64, f64) {
        let (w, d) = (4.0, 3.0 + (h >> 40) as f64 % 2.0);
        let left = (h >> 36).is_multiple_of(2);
        let la0 = if left { 1.5 } else { self.width - 1.5 - w };
        (la0, la0 + w, self.depth - 1.5 - d, self.depth - 1.5)
    }

    fn in_shed(&self, x: i32, z: i32) -> bool {
        self.plot_at(x, z).is_some_and(|p| {
            let h = self.hash(p.key);
            let (l0, l1, b0, b1) = self.shed(h);
            self.sheds && p.b >= 1.0 && (l0..l1).contains(&p.la) && (b0..b1).contains(&p.b)
        })
    }

    fn part(&self, x: i32, z: i32) -> Part {
        let Some(p) = self.plot_at(x, z) else {
            return if self.paths { Part::Path } else { Part::Lawn };
        };
        let h = self.hash(p.key);
        let hedged = !h.is_multiple_of(3);
        let edge = if hedged { Part::Hedge } else { Part::Fence };
        let mid = self.width / 2.0;
        // Gate onto the path; the back hedge is drawn by the near plot only.
        if p.b < 1.0 {
            return if (p.la - mid).abs() < 0.75 {
                Part::Gate
            } else {
                edge
            };
        }
        if p.la < 1.0 || (p.key.2 == 0 && p.b >= self.depth - 1.0) {
            return edge;
        }
        if self.sheds {
            let (l0, l1, b0, b1) = self.shed(h);
            if (l0..l1).contains(&p.la) && (b0..b1).contains(&p.b) {
                // A wall where a neighbouring column is outside the shed.
                let wall = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .any(|(dx, dz)| !self.in_shed(x + dx, z + dz));
                return Part::Shed { wall };
            }
        }
        if (p.la - mid).abs() < 0.75 && p.b < self.depth * 0.6 {
            return Part::Walk;
        }
        let corner = (p.la - (self.width - 2.0)).abs() < 0.5 && (p.b - 2.0).abs() < 0.5;
        if corner && (h >> 44).is_multiple_of(2) {
            return Part::Compost;
        }
        if corner && (h >> 44) % 4 == 1 {
            return Part::Barrel;
        }
        match (h >> 8) % 4 {
            // Vegetable beds three wide across the front half, lawn behind.
            0 | 1 if p.b >= 2.0 && p.b < self.depth * 0.62 => {
                let k = ((p.la - 1.0) / 4.0).floor();
                if (p.la - 1.0) - k * 4.0 < 3.0 {
                    let bed = cell_hash(h, k as i64, 7);
                    Part::Bed(CROPS[(bed % CROPS.len() as u64) as usize])
                } else {
                    Part::Walk
                }
            }
            // A flower border along the hedge, lawn in the middle.
            2 if p.la < 2.0 || p.la >= self.width - 1.0 => Part::Flowers,
            _ => Part::Lawn,
        }
    }

    /// The plot's fruit tree, on the lawn behind the beds.
    fn tree(&self, key: (i64, i64, i64)) -> Option<(i32, i32)> {
        let h = self.hash(key);
        if (h >> 48).is_multiple_of(5) {
            return None;
        }
        let la = if (h >> 36).is_multiple_of(2) {
            self.width - 3.5
        } else {
            3.5
        };
        let b = self.depth * 0.72;
        let p = self.period();
        let (band, i, side) = key;
        let a = i as f64 * self.width + la;
        let c = band as f64 * p + if side == 0 { self.path + b } else { p - b };
        Some(self.frame.column(a, c))
    }
}

/// Garden houses: small buildings in a colony get wood walls, a pitched roof and one storey
/// wherever the map leaves those open, instead of a town building's palette.
pub fn prepare(elements: &mut [ProcessedElement], scale: f64) {
    const CELL: i32 = 128;
    let mut colonies: Vec<(Bounds, Vec<ProcessedNode>)> = Vec::new();
    for e in elements.iter() {
        let tags = e.tags();
        if tags.get("landuse").map(String::as_str) != Some("allotments") {
            continue;
        }
        let rings: Vec<&[ProcessedNode]> = match e {
            ProcessedElement::Way(w) => vec![w.nodes.as_slice()],
            ProcessedElement::Relation(r) => r
                .members
                .iter()
                .filter(|m| m.role == ProcessedMemberRole::Outer)
                .map(|m| m.way.nodes.as_slice())
                .collect(),
            ProcessedElement::Node(_) => continue,
        };
        for ring in rings {
            if let Some(b) = bounds(ring) {
                colonies.push((b, ring.to_vec()));
            }
        }
    }
    if colonies.is_empty() {
        return;
    }
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, &(b, _)) in colonies.iter().enumerate() {
        let (x0, z0) = ((b.0 as i32).div_euclid(CELL), (b.1 as i32).div_euclid(CELL));
        let (x1, z1) = ((b.2 as i32).div_euclid(CELL), (b.3 as i32).div_euclid(CELL));
        for gx in x0..=x1 {
            for gz in z0..=z1 {
                grid.entry((gx, gz)).or_default().push(i);
            }
        }
    }
    let max_area = 100.0 * scale * scale;
    for e in elements.iter_mut() {
        let ProcessedElement::Way(w) = e else {
            continue;
        };
        let small_house = matches!(
            w.tags.get("building").map(String::as_str),
            Some(
                "yes"
                    | "allotment_house"
                    | "garden_house"
                    | "shed"
                    | "hut"
                    | "cabin"
                    | "summer_house"
            )
        );
        if !small_house || w.nodes.len() < 4 || ring_area(&w.nodes) > max_area {
            continue;
        }
        let c = centroid(&w.nodes);
        let key = ((c.0 as i32).div_euclid(CELL), (c.1 as i32).div_euclid(CELL));
        let in_colony = grid.get(&key).is_some_and(|ids| {
            ids.iter().any(|&i| {
                let (b, ring) = &colonies[i];
                inside(*b, c) && point_in_rings(&[ring.as_slice()], c)
            })
        });
        if !in_colony {
            continue;
        }
        let h = cell_hash(w.id, 7, 11);
        let roof = ["gabled", "gabled", "gabled", "hipped", "skillion"][(h % 5) as usize];
        let cover = if (h >> 8).is_multiple_of(3) {
            "roof_tiles"
        } else {
            "roofing_felt"
        };
        for (k, v) in [
            ("building:material", "wood"),
            ("roof:shape", roof),
            ("roof:material", cover),
        ] {
            w.tags.entry(k.to_string()).or_insert_with(|| v.to_string());
        }
        if !w.tags.contains_key("height") {
            w.tags
                .entry("building:levels".to_string())
                .or_insert_with(|| "1".to_string());
        }
    }
}

fn ring_area(nodes: &[ProcessedNode]) -> f64 {
    let twice: f64 = nodes
        .windows(2)
        .map(|w| w[0].x as f64 * w[1].z as f64 - w[1].x as f64 * w[0].z as f64)
        .sum();
    twice.abs() / 2.0
}

type Bounds = (f64, f64, f64, f64);

pub fn allotments_area(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    fill: &[(i32, i32)],
    road_mask: &RoadMaskBitmap,
    footprints: &BuildingFootprintBitmap,
    holes: Option<&Holes>,
) {
    let Some(frame) = Frame::of(&way.nodes) else {
        return;
    };
    let (mut paths, mut built) = (false, false);
    let mask = AreaMask::new(fill, |x, z| {
        paths |= road_mask.contains(x, z);
        built |= footprints.contains(x, z);
        !in_hole(holes, x, z)
    });
    let s = editor.scale().clamp(0.25, 4.0);
    let h = cell_hash(way.id, 3, 5);
    let colony = Colony {
        id: way.id,
        frame,
        mask,
        width: ((10 + h % 5) as f64 * s).max(6.0),
        depth: ((15 + (h >> 8) % 6) as f64 * s).max(8.0),
        path: (2.0 * s).max(1.0),
        paths: !paths,
        sheds: !built,
    };
    let tree = Fruit {
        trunk: 2,
        radius: 1.8,
        log: OAK_LOG,
        leaves: OAK_LEAVES,
        accent: Some(FLOWERING_AZALEA_LEAVES),
    };

    for &(x, z) in fill {
        if in_hole(holes, x, z) || !editor.in_bounds(x, z) || ground_is_protected(editor, x, z) {
            continue;
        }
        let part = colony.part(x, z);
        let ground = match part {
            Part::Path => GRAVEL,
            Part::Walk | Part::Gate => DIRT_PATH,
            Part::Bed(_) => FARMLAND,
            _ => GRASS_BLOCK,
        };
        editor.set_block(ground, x, 0, z, None, None);
        if !editor.owns(x, z) || editor.is_lc_water(x, z) || footprints.contains(x, z) {
            continue;
        }
        // The plot's fruit tree first, so no undergrowth takes its trunk's place.
        if matches!(part, Part::Lawn | Part::Flowers | Part::Walk) {
            if let Some(p) = colony.plot_at(x, z) {
                // Trees stand far enough inside their plot that only its own reaches here.
                if let Some(t) = colony.tree(p.key) {
                    if colony.mask.contains(t.0, t.1) && !footprints.contains(t.0, t.1) {
                        tree.grow(editor, t, x, z);
                    }
                }
            }
        }
        let ch = coord_hash(x ^ 0x0A11, z);
        match part {
            Part::Hedge => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
            Part::Fence => {
                let ground_y = editor.get_ground_level(x, z);
                let joins = |dx: i32, dz: i32| {
                    let (nx, nz) = (x + dx, z + dz);
                    colony.part(nx, nz) == Part::Fence
                        && colony.mask.contains(nx, nz)
                        && !footprints.contains(nx, nz)
                        && !road_mask.contains(nx, nz)
                        && editor.get_ground_level(nx, nz) == ground_y
                };
                let fence = connected_fence(
                    SPRUCE_FENCE,
                    joins(0, -1),
                    joins(0, 1),
                    joins(1, 0),
                    joins(-1, 0),
                );
                let y = editor.get_absolute_y(x, 1, z);
                if !editor.block_exists_absolute(x, y, z) {
                    editor.set_block_with_properties_absolute(fence, x, y, z, None, None);
                }
            }
            Part::Bed(crop) => editor.set_block(crop, x, 1, z, None, None),
            Part::Flowers if ch.is_multiple_of(2) => {
                editor.set_block(super::cemetery::flower(ch >> 1), x, 1, z, None, None);
            }
            Part::Compost => editor.set_block(COMPOSTER, x, 1, z, None, None),
            Part::Barrel => editor.set_block(BARREL, x, 1, z, None, None),
            Part::Shed { wall } => {
                let key = colony
                    .plot_at(x, z)
                    .map(|p| colony.hash(p.key))
                    .unwrap_or(0);
                const WALLS: [Block; 4] =
                    [SPRUCE_PLANKS, OAK_PLANKS, BIRCH_PLANKS, DARK_OAK_PLANKS];
                let planks = WALLS[(key >> 52) as usize % WALLS.len()];
                if wall {
                    editor.set_block(planks, x, 1, z, None, None);
                    let window = ch.is_multiple_of(5);
                    editor.set_block(if window { GLASS } else { planks }, x, 2, z, None, None);
                }
                editor.set_block(DARK_OAK_SLAB, x, 3, z, None, None);
            }
            Part::Lawn if ch.is_multiple_of(7) => editor.set_block(GRASS, x, 1, z, None, None),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::*;

    fn colony(footprints: &BuildingFootprintBitmap) -> WorldEditor<'static> {
        let xzbbox = Box::leak(Box::new(XZBBox::rect_from_xz_lengths(130.0, 80.0).unwrap()));
        let mut editor = test_editor(xzbbox);
        let way = rect_way(21, 2, 2, 127, 77, &[("landuse", "allotments")]);
        let fill = crate::floodfill::flood_fill_area(
            &way.nodes.iter().map(|n| (n.x, n.z)).collect::<Vec<_>>(),
            None,
        );
        allotments_area(
            &mut editor,
            &way,
            &fill,
            &RoadMaskBitmap::new_empty(),
            footprints,
            None,
        );
        editor
    }

    #[test]
    fn a_colony_is_cut_into_hedged_plots_along_paths() {
        let editor = colony(&BuildingFootprintBitmap::new_empty());
        let paths = count_blocks(&editor, 3..127, 3..77, 0, &[GRAVEL]);
        let edges = count_blocks(&editor, 3..127, 3..77, 1, &[OAK_LEAVES, SPRUCE_FENCE]);
        let beds = count_blocks(&editor, 3..127, 3..77, 0, &[FARMLAND]);
        let sheds = count_blocks(&editor, 3..127, 3..77, 3, &[DARK_OAK_SLAB]);
        assert!(paths > 200, "{paths} path columns");
        assert!(edges > 400, "{edges} hedge and fence columns");
        assert!(beds > 100, "{beds} bed columns");
        assert!(sheds > 30, "{sheds} shed roof columns");
    }

    #[test]
    fn small_houses_in_a_colony_become_wooden_garden_houses() {
        let mut elements = vec![
            ProcessedElement::Way(rect_way(1, 0, 0, 100, 100, &[("landuse", "allotments")])),
            ProcessedElement::Way(rect_way(
                2,
                10,
                10,
                15,
                14,
                &[("building", "allotment_house")],
            )),
            ProcessedElement::Way(rect_way(3, 30, 30, 50, 50, &[("building", "yes")])),
            ProcessedElement::Way(rect_way(4, 150, 10, 155, 14, &[("building", "yes")])),
            ProcessedElement::Way(rect_way(
                5,
                60,
                60,
                65,
                64,
                &[("building", "shed"), ("roof:shape", "flat")],
            )),
        ];
        prepare(&mut elements, 1.0);
        let tag = |i: usize, k: &str| elements[i].tags().get(k).cloned();
        assert_eq!(tag(1, "building:material").as_deref(), Some("wood"));
        assert_eq!(tag(1, "building:levels").as_deref(), Some("1"));
        assert!(
            tag(2, "building:material").is_none(),
            "a clubhouse is too big"
        );
        assert!(tag(3, "building:material").is_none(), "outside the colony");
        assert_eq!(
            tag(4, "roof:shape").as_deref(),
            Some("flat"),
            "mapped tags stay"
        );
    }

    #[test]
    fn mapped_sheds_leave_no_room_for_invented_ones() {
        let xzbbox = XZBBox::rect_from_xz_lengths(130.0, 80.0).unwrap();
        let shed = bitmap_with_rect(&xzbbox, 60, 30, 63, 33);
        let editor = colony(&shed);
        assert_eq!(count_blocks(&editor, 3..127, 3..77, 3, &[DARK_OAK_SLAB]), 0);
    }
}
