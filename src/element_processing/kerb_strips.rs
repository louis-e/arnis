//! The strip between a sidewalk and the road it runs beside. Mapped as two ways, each drawn as
//! wide as its tags make it, the two leave a block or two of ground between them here and meet
//! there, which reads as a ragged edge. Where they run close and side by side the sidewalk is
//! paved out to the road, unless something stands in between: a building, a tree, a hedge or a
//! fence, a mapped lawn or paved area, water. A verge the land cover keeps green stays where it
//! runs the length of the stretch; where the two meet along much of it, it is the gaps that are
//! the odd ones out, and they are paved too.

use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::XZBBox;
use crate::element_processing::bridges::is_bridge_way;
use crate::element_processing::highways::{
    highway_block_range, in_side_stamp, is_carriageway, stamps_square, surface_highway_type,
    tunnel_renders_at_surface, Sweep,
};
use crate::element_processing::road_width::kerb_sides;
use crate::floodfill_cache::CoordinateBitmap;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;
use fnv::FnvHashMap;
use rayon::prelude::*;

/// The widest strip paved, and the shortest stretch a sidewalk must run beside the road
/// for it, in metres: a path that only meets the road, or crosses it, runs beside it for less.
const MAX_GAP_M: f64 = 3.0;
const MIN_RUN_M: f64 = 6.0;

/// A cell of a strip, with the sidewalk's own cell it reaches out from, whose level it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StripCell {
    pub cell: (i32, i32),
    pub from: (i32, i32),
}

/// The strips paved between sidewalks and the roads they run beside, by sidewalk way.
#[derive(Default)]
pub struct KerbStrips(FnvHashMap<u64, Vec<StripCell>>);

impl KerbStrips {
    /// The strip the sidewalk `way` is paved out over.
    pub fn of(&self, way: u64) -> &[StripCell] {
        self.0.get(&way).map_or(&[], Vec::as_slice)
    }

    /// Every cell of every strip.
    pub fn cells(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.0.values().flatten().map(|c| c.cell)
    }
}

/// A way that may run beside a road as its sidewalk: a footway other than a crossing, a
/// pedestrian street drawn as a line, or a path tagged as the sidewalk; on the ground, not on
/// a bridge, in a tunnel or on another level. A cycle path keeps its own width.
pub(crate) fn runs_as_sidewalk(way: &ProcessedWay, highway_type: &str) -> bool {
    let tag = |k: &str| way.tags.get(k).map(String::as_str);
    let kind = match highway_type {
        "footway" => tag("footway") != Some("crossing"),
        "pedestrian" => tag("area") != Some("yes"),
        "path" => tag("footway") == Some("sidewalk") && tag("bicycle") != Some("designated"),
        _ => false,
    };
    kind && !is_bridge_way(way)
        && tag("tunnel").is_none_or(|t| t == "no")
        && tag("indoor").is_none_or(|t| t == "no")
        && ["layer", "level"]
            .iter()
            .all(|k| tag(k).is_none_or(|v| v.trim().parse::<f64>().is_ok_and(|l| l == 0.0)))
}

/// The cells along a way's centreline.
fn centreline(way: &ProcessedWay) -> Vec<(i32, i32)> {
    let mut points: Vec<(i32, i32)> = Vec::new();
    for pair in way.nodes.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        for (x, _, z) in bresenham_line(a.x, 0, a.z, b.x, 0, b.z) {
            if points.last() != Some(&(x, z)) {
                points.push((x, z));
            }
        }
    }
    points
}

/// Stamps a way's surface into `bits`, as wide as the renderer draws it, each side of a road
/// out to its own kerb.
fn stamp(bits: &mut CoordinateBitmap, way: &ProcessedWay, highway_type: &str, scale: f64) {
    let half = highway_block_range(highway_type, &way.tags, scale);
    let sides = kerb_sides(highway_type, &way.tags, scale);
    let square = stamps_square(way);
    // The stamp's cells, swept along the way: once a way, or once a segment where the two
    // kerbs lie at different reaches and the way's direction says which side is which.
    let cells = |u: Option<(f32, f32)>| {
        Sweep::new(
            half,
            (-half..=half)
                .flat_map(|dx| (-half..=half).map(move |dz| (dx, dz)))
                .filter(|&(dx, dz)| square || in_side_stamp(dx, dz, u, sides))
                .map(|(dx, dz)| (dx, dz, false))
                .collect(),
        )
    };
    let lopsided = !square && sides.0 != sides.1;
    let mut sweep = (!lopsided).then(|| cells(None));
    for pair in way.nodes.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if lopsided {
            let (sx, sz) = ((b.x - a.x) as f32, (b.z - a.z) as f32);
            let len = sx.hypot(sz);
            sweep = Some(cells((len > 0.0).then(|| (sx / len, sz / len))));
        }
        let Some(sweep) = sweep.as_mut() else {
            continue;
        };
        for (x, _, z) in bresenham_line(a.x, 0, a.z, b.x, 0, b.z) {
            for &(dx, dz, _) in sweep.step_to(x, z) {
                bits.set(x + dx, z + dz);
            }
        }
    }
}

/// What a sidewalk finds to one side of a point on it, with its direction there and the
/// way out to that side.
type Found = (Reach, (f32, f32), (f32, f32));

/// What a sidewalk finds out to one side of a point on it: how far out its own edge and the
/// road are, how many blocks lie between, and the last of its own cells on the way out.
struct Reach {
    edge: f32,
    road: f32,
    gap: usize,
    from: (i32, i32),
}

/// Looks out from `p` along `n` for a road within `max_gap` blocks of the sidewalk's edge,
/// over open ground only. `None` where none is, or something stands in between.
fn reach(
    p: (i32, i32),
    n: (f32, f32),
    limit: f32,
    max_gap: usize,
    road: &impl Fn(i32, i32) -> bool,
    walkway: &impl Fn(i32, i32) -> bool,
    open: &impl Fn(i32, i32) -> bool,
) -> Option<Reach> {
    let mut own = true;
    let mut from = p;
    let mut edge = 0.0;
    let mut gap: Vec<(i32, i32)> = Vec::new();
    let mut last = p;
    let mut t = 0.0f32;
    while t < limit {
        t += 0.25;
        let c = (
            (p.0 as f32 + n.0 * t).round() as i32,
            (p.1 as f32 + n.1 * t).round() as i32,
        );
        if c == last {
            continue;
        }
        last = c;
        if own && walkway(c.0, c.1) {
            from = c;
            continue;
        }
        if own {
            own = false;
            edge = t;
        }
        if road(c.0, c.1) {
            return Some(Reach {
                edge,
                road: t,
                gap: gap.len(),
                from,
            });
        }
        if walkway(c.0, c.1) || !open(c.0, c.1) {
            return None;
        }
        if !gap.contains(&c) {
            gap.push(c);
            if gap.len() > max_gap {
                return None;
            }
        }
    }
    None
}

/// The ways a strip lies between, stamped from their tags alone, before the road masks are
/// laid: the roads on the ground and every path and footway, as wide as the renderer draws
/// them, and the sidewalks that may run beside a road. `KerbWays::strips` reads them with
/// the masks.
pub struct KerbWays<'a> {
    vehicles: CoordinateBitmap,
    walkways: CoordinateBitmap,
    sidewalks: Vec<(&'a ProcessedWay, i32)>,
}

impl<'a> KerbWays<'a> {
    pub fn stamp(
        elements: &'a [ProcessedElement],
        editor: &WorldEditor,
        xzbbox: &XZBBox,
        scale: f64,
    ) -> Self {
        let mut vehicles = CoordinateBitmap::new(xzbbox);
        let mut walkways = CoordinateBitmap::new(xzbbox);
        let mut sidewalks: Vec<(&ProcessedWay, i32)> = Vec::new();
        for element in elements {
            let ProcessedElement::Way(way) = element else {
                continue;
            };
            let Some(highway_type) = surface_highway_type(way) else {
                continue;
            };
            if is_carriageway(highway_type) {
                let at_grade = !is_bridge_way(way)
                    && way.tags.get("tunnel").is_none_or(|t| t == "no")
                    && way.tags.get("layer").is_none_or(|l| l.trim() == "0");
                if at_grade && tunnel_renders_at_surface(way, Some(editor), scale) {
                    stamp(&mut vehicles, way, highway_type, scale);
                }
                continue;
            }
            stamp(&mut walkways, way, highway_type, scale);
            if runs_as_sidewalk(way, highway_type) {
                sidewalks.push((way, highway_block_range(highway_type, &way.tags, scale)));
            }
        }
        Self {
            vehicles,
            walkways,
            sidewalks,
        }
    }

    /// The strips between the sidewalks and the roads they run beside. `roads` is every road
    /// and path surface as the masks laid it, `blocked` what the strip must not pave over
    /// besides the buildings in `footprints`: mapped lawns and paved areas, hedges, fences
    /// and walls.
    pub fn strips(
        &self,
        editor: &WorldEditor,
        scale: f64,
        roads: &CoordinateBitmap,
        footprints: &CoordinateBitmap,
        blocked: &CoordinateBitmap,
    ) -> KerbStrips {
        strips_beside(self, editor, scale, roads, footprints, blocked)
    }
}

fn strips_beside(
    ways: &KerbWays,
    editor: &WorldEditor,
    scale: f64,
    roads: &CoordinateBitmap,
    footprints: &CoordinateBitmap,
    blocked: &CoordinateBitmap,
) -> KerbStrips {
    let max_gap = ((MAX_GAP_M * scale).round() as usize).max(1);
    let min_run = ((MIN_RUN_M * scale).round() as usize).max(4);
    let KerbWays {
        vehicles,
        walkways,
        sidewalks,
    } = ways;
    if sidewalks.is_empty() {
        return KerbStrips::default();
    }
    let road = |x: i32, z: i32| vehicles.contains(x, z) && roads.contains(x, z);
    let walkway = |x: i32, z: i32| walkways.contains(x, z);
    let open = |x: i32, z: i32| {
        !roads.contains(x, z)
            && !footprints.contains(x, z)
            && !blocked.contains(x, z)
            && !editor.water_at(x, z)
            && !editor.trunk_near(x, z)
    };
    let strips: Vec<(u64, Vec<StripCell>)> = sidewalks
        .par_iter()
        .filter_map(|&(way, half)| {
            let points = centreline(way);
            let limit = half as f32 + max_gap as f32 + 2.0;
            let mut cells: Vec<StripCell> = Vec::new();
            for side in [1.0f32, -1.0] {
                // Its direction over a few blocks either way, and what it finds to this side.
                let found: Vec<Option<Found>> = (0..points.len())
                    .map(|i| {
                        let (a, b) = (
                            points[i.saturating_sub(3)],
                            points[(i + 3).min(points.len() - 1)],
                        );
                        let (dx, dz) = ((b.0 - a.0) as f32, (b.1 - a.1) as f32);
                        let len = dx.hypot(dz);
                        if len < 1e-3 {
                            return None;
                        }
                        let d = (dx / len, dz / len);
                        let n = (-d.1 * side, d.0 * side);
                        reach(points[i], n, limit, max_gap, &road, &walkway, &open)
                            .map(|r| (r, d, n))
                    })
                    .collect();
                // Stretches that run beside the road long enough, paved from the sidewalk's
                // edge to the road: the open cells within half a block either way of each
                // point's line out to it.
                let mut i = 0;
                while i < found.len() {
                    let start = i;
                    while i < found.len() && found[i].is_some() {
                        i += 1;
                    }
                    let run = &found[start..i];
                    // Meeting the road along a quarter of the stretch or more, the gaps are
                    // ragged, and paved green or not; else a green verge stays.
                    let meets = run.iter().flatten().filter(|(r, _, _)| r.gap == 0).count();
                    let ragged = meets * 4 >= run.len();
                    if run.len() >= min_run {
                        for (k, f) in run.iter().enumerate() {
                            let Some((r, d, n)) = f else { continue };
                            let p = points[start + k];
                            let reach_out = r.road.ceil() as i32 + 1;
                            for dx in -reach_out..=reach_out {
                                for dz in -reach_out..=reach_out {
                                    let (cx, cz) = (p.0 + dx, p.1 + dz);
                                    let across = dx as f32 * n.0 + dz as f32 * n.1;
                                    let along = dx as f32 * d.0 + dz as f32 * d.1;
                                    if along.abs() <= 0.75
                                        && across > 0.0
                                        && across < r.road - 0.25
                                        && across >= r.edge - 0.75
                                        && !walkway(cx, cz)
                                        && !road(cx, cz)
                                        && open(cx, cz)
                                        && (ragged || !editor.green_at(cx, cz))
                                    {
                                        cells.push(StripCell {
                                            cell: (cx, cz),
                                            from: r.from,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    i = i.max(start + 1);
                }
            }
            cells.sort_unstable_by_key(|c| c.cell);
            cells.dedup_by_key(|c| c.cell);
            (!cells.is_empty()).then_some((way.id, cells))
        })
        .collect();
    KerbStrips(strips.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::ProcessedNode;
    use std::collections::HashMap;

    fn way(id: u64, a: (i32, i32), b: (i32, i32), tags: &[(&str, &str)]) -> ProcessedElement {
        let node = |i: u64, (x, z): (i32, i32)| ProcessedNode {
            id: i,
            tags: HashMap::new(),
            x,
            z,
        };
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes: vec![node(id * 10, a), node(id * 10 + 1, b)],
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    fn strips(
        elements: &[ProcessedElement],
        xzbbox: &XZBBox,
        blocked: &CoordinateBitmap,
    ) -> KerbStrips {
        let llbbox =
            crate::coordinate_system::geographic::LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let editor = WorldEditor::new(std::path::PathBuf::from("/dev/null/unused"), xzbbox, llbbox);
        let mut roads = CoordinateBitmap::new(xzbbox);
        for e in elements {
            if let ProcessedElement::Way(w) = e {
                if let Some(t) = surface_highway_type(w) {
                    stamp(&mut roads, w, t, 1.0);
                }
            }
        }
        KerbWays::stamp(elements, &editor, xzbbox, 1.0).strips(
            &editor,
            1.0,
            &roads,
            &CoordinateBitmap::new_empty(),
            blocked,
        )
    }

    #[test]
    fn a_sidewalk_a_block_or_two_off_the_road_is_paved_out_to_it() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        // A street along z = 50 and its sidewalk four blocks north, then turning away.
        let elements = vec![
            way(1, (0, 50), (119, 50), &[("highway", "residential")]),
            way(
                2,
                (10, 44),
                (80, 44),
                &[("highway", "footway"), ("footway", "sidewalk")],
            ),
            way(
                3,
                (80, 44),
                (80, 5),
                &[("highway", "footway"), ("footway", "sidewalk")],
            ),
        ];
        let found = strips(&elements, &xzbbox, &CoordinateBitmap::new_empty());
        let strip = found.of(2);
        assert!(!strip.is_empty());
        // Every cell between the two along the way is paved, none past either.
        let mut roads = CoordinateBitmap::new(&xzbbox);
        for e in &elements {
            if let ProcessedElement::Way(w) = e {
                stamp(&mut roads, w, surface_highway_type(w).unwrap(), 1.0);
            }
        }
        for x in 20..70 {
            for z in 44..50 {
                if !roads.contains(x, z) {
                    assert!(strip.iter().any(|c| c.cell == (x, z)), "({x}, {z})");
                }
            }
        }
        assert!(strip.iter().all(|c| !roads.contains(c.cell.0, c.cell.1)));
        // The part that turns away to the north runs beside no road.
        assert!(found.of(3).is_empty());
    }

    #[test]
    fn a_strip_stops_at_a_mapped_lawn_and_a_wider_verge_stays() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let elements = vec![
            way(1, (0, 50), (119, 50), &[("highway", "residential")]),
            way(
                2,
                (10, 44),
                (100, 44),
                &[("highway", "footway"), ("footway", "sidewalk")],
            ),
            // Eight blocks off: a verge, not a gap.
            way(
                3,
                (10, 60),
                (100, 60),
                &[("highway", "footway"), ("footway", "sidewalk")],
            ),
        ];
        let mut lawn = CoordinateBitmap::new(&xzbbox);
        for x in 40..60 {
            for z in 40..50 {
                lawn.set(x, z);
            }
        }
        let found = strips(&elements, &xzbbox, &lawn);
        let strip = found.of(2);
        assert!(strip.iter().any(|c| c.cell.0 < 40) && strip.iter().any(|c| c.cell.0 >= 60));
        assert!(strip.iter().all(|c| !lawn.contains(c.cell.0, c.cell.1)));
        assert!(found.of(3).is_empty());
    }

    #[test]
    fn a_crossing_and_a_path_to_a_door_are_no_sidewalks() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let elements = vec![
            way(1, (0, 50), (119, 50), &[("highway", "residential")]),
            way(2, (40, 47), (40, 10), &[("highway", "footway")]),
            way(
                3,
                (60, 30),
                (60, 70),
                &[("highway", "footway"), ("footway", "crossing")],
            ),
        ];
        let found = strips(&elements, &xzbbox, &CoordinateBitmap::new_empty());
        assert_eq!(found.cells().count(), 0);
    }
}
