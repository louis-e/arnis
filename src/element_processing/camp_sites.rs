//! Camp and caravan sites: pitches between gravel lanes with tents, caravans and motorhomes.

use crate::block_definitions::*;
use crate::element_processing::area_layout::{
    bounds, cell_hash, inside, interior_point, point_in_rings, AreaMask, Frame,
};
use crate::element_processing::landuse::ground_is_protected;
use crate::floodfill_cache::{
    in_hole, BuildingFootprintBitmap, FloodFillCache, Holes, RoadMaskBitmap,
};
use crate::land_cover::coord_hash;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use crate::structures::car::place_vehicle;
use crate::world_editor::WorldEditor;
use std::collections::HashMap;
use std::time::Duration;

const PITCHES_TAG: &str = "arnis:pitches";

pub fn claims(tags: &HashMap<String, String>) -> bool {
    matches!(
        tags.get("tourism").map(String::as_str),
        Some("camp_site" | "caravan_site")
    )
}

pub fn is_pitch(tags: &HashMap<String, String>) -> bool {
    tags.get("tourism").map(String::as_str) == Some("camp_pitch")
}

/// Tags sites whose pitches are mapped, so their own are not laid out again.
pub fn prepare(elements: &mut [ProcessedElement]) {
    let pitches: Vec<(f64, f64)> = elements
        .iter()
        .filter_map(|e| match e {
            ProcessedElement::Node(n) if is_pitch(&n.tags) => Some((n.x as f64, n.z as f64)),
            ProcessedElement::Way(w) if is_pitch(&w.tags) => interior_point(&w.nodes),
            _ => None,
        })
        .collect();
    if pitches.is_empty() {
        return;
    }
    for e in elements.iter_mut() {
        let (tags, rings): (_, Vec<&[ProcessedNode]>) = match e {
            ProcessedElement::Way(w) => (&mut w.tags, vec![w.nodes.as_slice()]),
            ProcessedElement::Relation(r) => (
                &mut r.tags,
                r.members
                    .iter()
                    .filter(|m| m.role == ProcessedMemberRole::Outer)
                    .map(|m| m.way.nodes.as_slice())
                    .collect(),
            ),
            ProcessedElement::Node(_) => continue,
        };
        if !claims(tags) {
            continue;
        }
        let boxes: Vec<_> = rings.iter().filter_map(|r| bounds(r)).collect();
        if pitches
            .iter()
            .any(|&p| boxes.iter().any(|&b| inside(b, p)) && point_in_rings(&rings, p))
        {
            tags.insert(PITCHES_TAG.to_string(), "yes".to_string());
        }
    }
}

/// What stands on a pitch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stay {
    Dome,
    Ridge,
    Caravan,
    Motorhome,
}

impl Stay {
    fn pick(h: u64, tags: &HashMap<String, String>) -> Option<Self> {
        let no = |k: &str| tags.get(k).map(String::as_str) == Some("no");
        let caravan_site = tags.get("tourism").map(String::as_str) == Some("caravan_site");
        let stay = match h % 100 {
            _ if caravan_site => match h % 100 {
                0..=44 => Stay::Motorhome,
                45..=79 => Stay::Caravan,
                _ => return None,
            },
            0..=21 => Stay::Dome,
            22..=35 => Stay::Ridge,
            36..=62 => Stay::Caravan,
            63..=79 => Stay::Motorhome,
            _ => return None,
        };
        let opted_out = match stay {
            Stay::Dome | Stay::Ridge => no("tents"),
            Stay::Caravan => no("caravans"),
            Stay::Motorhome => no("motorhome"),
        };
        (!opted_out).then_some(stay)
    }

    /// Footprint as (across the pitch, back from the lane) offsets from its centre, inclusive.
    fn extent(self) -> (i32, i32, i32, i32) {
        match self {
            Stay::Dome => (-1, 1, -1, 1),
            Stay::Ridge => (-1, 1, -1, 2),
            // The caravan and the car parked beside it.
            Stay::Caravan => (-1, 5, -3, 3),
            Stay::Motorhome => (-2, 2, -4, 4),
        }
    }
}

pub fn generate_camp_site(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    flood_fill_cache: &FloodFillCache,
    footprints: &BuildingFootprintBitmap,
    road_mask: &RoadMaskBitmap,
    timeout: Option<&Duration>,
) {
    camp_area(
        editor,
        way,
        flood_fill_cache,
        footprints,
        road_mask,
        timeout,
        None,
    );
}

/// One site, or one outer ring of a site's multipolygon less its holes.
pub fn camp_area(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    flood_fill_cache: &FloodFillCache,
    footprints: &BuildingFootprintBitmap,
    road_mask: &RoadMaskBitmap,
    timeout: Option<&Duration>,
    holes: Option<&Holes>,
) {
    let fill = flood_fill_cache.get_or_compute(way, timeout);
    let Some(frame) = Frame::of(&way.nodes) else {
        return;
    };
    let (mut paths, mut built) = (false, 0usize);
    let mask = AreaMask::new(&fill, |x, z| {
        paths |= road_mask.contains(x, z);
        built += footprints.contains(x, z) as usize;
        !in_hole(holes, x, z)
    });
    // A site mapped with its pitches or mobile homes needs none of its own.
    let laid_out = !way.tags.contains_key(PITCHES_TAG) && built * 12 < fill.len();
    let s = editor.scale().clamp(0.25, 4.0);
    let site = Site {
        id: way.id,
        frame,
        mask,
        width: (9.0 * s).max(5.0),
        depth: (10.0 * s).max(5.0),
        lane: (4.0 * s).max(2.0),
        lanes: laid_out && !paths,
    };

    for &(x, z) in fill.iter() {
        if in_hole(holes, x, z) || !editor.in_bounds(x, z) || ground_is_protected(editor, x, z) {
            continue;
        }
        let lane = site.lanes && site.on_lane(x, z);
        editor.set_block(if lane { GRAVEL } else { GRASS_BLOCK }, x, 0, z, None, None);
    }
    if laid_out {
        site.place(editor, way, road_mask, footprints);
    }
}

struct Site {
    id: u64,
    frame: Frame,
    mask: AreaMask,
    width: f64,
    depth: f64,
    lane: f64,
    lanes: bool,
}

impl Site {
    fn period(&self) -> f64 {
        self.lane + 2.0 * self.depth
    }

    /// The pitch a column lies on as (band, index, side), or None on a lane.
    fn pitch_of(&self, x: i32, z: i32) -> Option<(i64, i64, i64)> {
        let (a, c) = self.frame.local(x, z);
        let p = self.period();
        let band = (c / p).floor();
        let cc = c - band * p;
        if cc < self.lane {
            return None;
        }
        let side = if cc < self.lane + self.depth { 0 } else { 1 };
        Some((band as i64, (a / self.width).floor() as i64, side))
    }

    fn on_lane(&self, x: i32, z: i32) -> bool {
        let (_, c) = self.frame.local(x, z);
        c.rem_euclid(self.period()) < self.lane
    }

    fn free(
        &self,
        editor: &WorldEditor,
        road_mask: &RoadMaskBitmap,
        footprints: &BuildingFootprintBitmap,
        x: i32,
        z: i32,
    ) -> bool {
        self.mask.contains(x, z)
            && !road_mask.contains(x, z)
            && !footprints.contains(x, z)
            && !editor.surface_is_sealed(x, z)
            && !editor.is_lc_water(x, z)
    }

    fn place(
        &self,
        editor: &mut WorldEditor,
        way: &ProcessedWay,
        road_mask: &RoadMaskBitmap,
        footprints: &BuildingFootprintBitmap,
    ) {
        let [along, across] = self.frame.local_span(editor.bounds());
        let (a0, a1) = (along.0.max(0.0), along.1.min(self.frame.length()));
        let (c0, c1) = (across.0.max(0.0), across.1.min(self.frame.width()));
        if a0 > a1 || c0 > c1 {
            return;
        }
        let p = self.period();
        let back = self.frame.across_axis();
        for band in (c0 / p).floor() as i64 - 1..=(c1 / p).ceil() as i64 {
            for i in (a0 / self.width).floor() as i64 - 1..=(a1 / self.width).ceil() as i64 {
                for side in 0..2i64 {
                    let h = cell_hash(self.id ^ side as u64, band, i);
                    // The near pitch backs away from its lane, the far one from the next.
                    let (c_mid, dir) = if side == 0 {
                        (band as f64 * p + self.lane + self.depth / 2.0, back)
                    } else {
                        (band as f64 * p + p - self.depth / 2.0, (-back.0, -back.1))
                    };
                    let a_mid = (i as f64 + 0.5) * self.width;
                    let centre = self.frame.column(a_mid, c_mid);
                    let post = self.frame.column(
                        i as f64 * self.width + 0.5,
                        if side == 0 {
                            band as f64 * p + self.lane + 0.5
                        } else {
                            band as f64 * p + p - 0.5
                        },
                    );
                    if editor.owns(post.0, post.1)
                        && self.free(editor, road_mask, footprints, post.0, post.1)
                    {
                        editor.set_block(OAK_FENCE, post.0, 1, post.1, None, None);
                        if h.is_multiple_of(3) {
                            editor.set_block(LANTERN, post.0, 2, post.1, None, None);
                        }
                    }
                    let Some(stay) = Stay::pick(h >> 8, &way.tags) else {
                        continue;
                    };
                    let side_dir = (-dir.1, dir.0);
                    // The caravan and its car side by side, the pair centred on the pitch.
                    let centre = if stay == Stay::Caravan {
                        (centre.0 - 2 * side_dir.0, centre.1 - 2 * side_dir.1)
                    } else {
                        centre
                    };
                    if !editor.owns(centre.0, centre.1) {
                        continue;
                    }
                    let at = |u: i32, t: i32| {
                        (
                            centre.0 + u * side_dir.0 + t * dir.0,
                            centre.1 + u * side_dir.1 + t * dir.1,
                        )
                    };
                    // The whole stay on its own pitch, and a step around it on open ground.
                    let (u0, u1, t0, t1) = stay.extent();
                    let own = Some((band, i, side));
                    let fits = (u0 - 1..=u1 + 1).all(|u| {
                        (t0 - 1..=t1 + 1).all(|t| {
                            let (x, z) = at(u, t);
                            let inner = (u0..=u1).contains(&u) && (t0..=t1).contains(&t);
                            self.free(editor, road_mask, footprints, x, z)
                                && (!inner || self.pitch_of(x, z) == own)
                        })
                    });
                    if fits {
                        draw(editor, stay, h >> 16, &at, dir);
                    }
                }
            }
        }
    }
}

const TENTS: [Block; 7] = [
    GREEN_WOOL,
    BLUE_WOOL,
    ORANGE_WOOL,
    RED_WOOL,
    YELLOW_WOOL,
    CYAN_WOOL,
    GRAY_WOOL,
];

fn draw(
    editor: &mut WorldEditor,
    stay: Stay,
    h: u64,
    at: &dyn Fn(i32, i32) -> (i32, i32),
    dir: (i32, i32),
) {
    let mut put = |u: i32, t: i32, y: i32, block: Block| {
        let (x, z) = at(u, t);
        editor.set_block(block, x, y, z, None, None);
    };
    let cloth = TENTS[(h % TENTS.len() as u64) as usize];
    // Vehicles turn their length to run back from the lane.
    let rot = if dir.0 != 0 { 1 } else { 0 };
    match stay {
        Stay::Dome => {
            for u in -1..=1 {
                for t in -1..=1 {
                    // The door faces the lane.
                    if (u, t) != (0, -1) {
                        put(u, t, 1, cloth);
                    }
                    if u == 0 || t == 0 {
                        put(u, t, 2, cloth);
                    }
                }
            }
        }
        Stay::Ridge => {
            for t in -1..=2 {
                put(-1, t, 1, cloth);
                put(1, t, 1, cloth);
                put(0, t, 2, cloth);
            }
            put(0, 2, 1, cloth);
        }
        Stay::Caravan => {
            for u in -1..=1 {
                for t in -2i32..=2 {
                    let side = u != 0 || t.abs() == 2;
                    put(u, t, 1, WHITE_CONCRETE);
                    let window = side && (t.abs() == 2 || (u != 0 && t == 0));
                    put(u, t, 2, if window { GLASS } else { WHITE_CONCRETE });
                    put(u, t, 3, SMOOTH_QUARTZ_SLAB);
                }
            }
            // The drawbar towards the lane, an awning-side step, and the car alongside.
            put(0, -3, 1, IRON_BARS);
            let (cx, cz) = at(4, 0);
            place_vehicle(editor, cx, cz, rot, false, h >> 4);
        }
        Stay::Motorhome => {
            let (cx, cz) = at(0, 0);
            place_vehicle(editor, cx, cz, rot, true, h >> 4);
        }
    }
}

/// A pitch mapped on its own, with a tent or a camper on it.
pub fn generate_pitch_node(editor: &mut WorldEditor, node: &ProcessedNode) {
    if !editor.owns(node.x, node.z) {
        return;
    }
    let h = coord_hash(node.x ^ 0x0CA3, node.z);
    let dir = if h.is_multiple_of(2) { (0, 1) } else { (1, 0) };
    let side = (-dir.1, dir.0);
    let at = |u: i32, t: i32| {
        (
            node.x + u * side.0 + t * dir.0,
            node.z + u * side.1 + t * dir.1,
        )
    };
    let no = |k: &str| node.tags.get(k).map(String::as_str) == Some("no");
    let mut stays = Vec::new();
    if !no("tents") {
        stays.extend([Stay::Dome, Stay::Ridge]);
    }
    if !no("motorhome") {
        stays.push(Stay::Motorhome);
    }
    if let Some(&stay) = stays.get(((h >> 8) % 3) as usize % stays.len().max(1)) {
        draw(editor, stay, h >> 12, &at, dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_of_stay_has_its_own_opt_out() {
        let tags: HashMap<String, String> = [
            ("tourism".to_string(), "camp_site".to_string()),
            ("caravans".to_string(), "no".to_string()),
        ]
        .into();
        let stays: Vec<Stay> = (0..400).filter_map(|h| Stay::pick(h, &tags)).collect();
        assert!(!stays.contains(&Stay::Caravan));
        assert!(stays.contains(&Stay::Motorhome) && stays.contains(&Stay::Dome));
    }

    #[test]
    fn a_caravan_site_takes_no_tents() {
        let tags: HashMap<String, String> =
            [("tourism".to_string(), "caravan_site".to_string())].into();
        for h in 0..200 {
            assert!(!matches!(
                Stay::pick(h, &tags),
                Some(Stay::Dome | Stay::Ridge)
            ));
        }
        let no_tents: HashMap<String, String> = [
            ("tourism".to_string(), "camp_site".to_string()),
            ("tents".to_string(), "no".to_string()),
        ]
        .into();
        assert!(
            (0..200).all(|h| !matches!(Stay::pick(h, &no_tents), Some(Stay::Dome | Stay::Ridge)))
        );
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::*;

    #[test]
    fn a_mapped_pitch_marks_its_site() {
        let pitch = ProcessedNode {
            id: 9,
            tags: tag_map(&[("tourism", "camp_pitch")]),
            x: 20,
            z: 20,
        };
        let mut elements = vec![
            ProcessedElement::Way(rect_way(1, 0, 0, 50, 50, &[("tourism", "camp_site")])),
            ProcessedElement::Way(rect_way(2, 100, 0, 150, 50, &[("tourism", "camp_site")])),
            ProcessedElement::Node(pitch),
        ];
        prepare(&mut elements);
        let marked = |i: usize| match &elements[i] {
            ProcessedElement::Way(w) => w.tags.contains_key(PITCHES_TAG),
            _ => false,
        };
        assert!(marked(0) && !marked(1));
    }

    #[test]
    fn an_empty_site_is_laid_out_with_lanes_and_stays() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 90.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let way = rect_way(41, 2, 2, 117, 87, &[("tourism", "camp_site")]);
        generate_camp_site(
            &mut editor,
            &way,
            &FloodFillCache::new(),
            &BuildingFootprintBitmap::new_empty(),
            &RoadMaskBitmap::new_empty(),
            None,
        );
        let lanes = count_blocks(&editor, 3..117, 3..87, 0, &[GRAVEL]);
        let stays = (1..3)
            .map(|y| {
                count_blocks(&editor, 3..117, 3..87, y, &TENTS)
                    + count_blocks(&editor, 3..117, 3..87, y, &[WHITE_CONCRETE])
            })
            .sum::<usize>();
        assert!(lanes > 300, "{lanes} lane columns");
        assert!(stays > 40, "{stays} tent and caravan blocks");
    }
}
