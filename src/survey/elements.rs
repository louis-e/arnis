//! Sheds, garages and pools from the survey as ordinary elements.
//!
//! They are built by the same generators as mapped ones, so they get real roofs,
//! pool coping and every edition's blocks, and they meet OSM the way Overture
//! buildings do: a survey shed whose outline reaches any mapped or Overture
//! building is dropped, a survey pool that reaches a mapped pool or any building
//! is dropped, and neither stands on a mapped road, square or car park. OSM always
//! wins.

use super::grid::{label, shapes};
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::element_processing::highways::highway_block_range;
use crate::ground::Ground;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use fnv::FnvHashMap;
use std::collections::HashMap;

/// Id range of the survey's elements, clear of OSM's and of Overture's hashes.
const ID_BASE: u64 = 0x7A00_0000_0000_0000;

/// The survey's sheds and pools in the area, not yet checked against anything.
pub fn candidates(ground: &Ground, xzbbox: &XZBBox) -> Vec<ProcessedElement> {
    if !ground.has_survey() {
        return Vec::new();
    }
    let (x0, z0) = (xzbbox.min_x(), xzbbox.min_z());
    let w = (xzbbox.max_x() - x0 + 1).max(0) as usize;
    let h = (xzbbox.max_z() - z0 + 1).max(0) as usize;
    let mut out = Vec::new();
    let mut next_id = ID_BASE;
    for kind in [super::Kind::Shed, super::Kind::Pool] {
        let mut mask = vec![false; w * h];
        let mut height = vec![0.0f32; w * h];
        for z in 0..h {
            for x in 0..w {
                if let Some(c) = ground.survey_cell(XZPoint::new(x as i32, z as i32)) {
                    if c.kind == kind {
                        mask[z * w + x] = true;
                        height[z * w + x] = c.height;
                    }
                }
            }
        }
        let (lab, n) = label(&mask, w, h, false);
        if n == 0 {
            continue;
        }
        let sh = shapes(&lab, n, w);
        let mut cells: Vec<Vec<(i32, i32)>> = vec![Vec::new(); n as usize + 1];
        let mut heights: Vec<Vec<f32>> = vec![Vec::new(); n as usize + 1];
        for (i, &l) in lab.iter().enumerate() {
            if l != 0 {
                cells[l as usize].push(((i % w) as i32, (i / w) as i32));
                heights[l as usize].push(height[i]);
            }
        }
        for l in 1..=n as usize {
            // Sampling the metre grid onto blocks can leave a sliver; the survey's
            // own size limits were set on the metre grid.
            if sh[l].area < 4.0 {
                continue;
            }
            let ring = hull(&cells[l]);
            if ring.len() < 3 {
                continue;
            }
            let mut tags = HashMap::new();
            match kind {
                super::Kind::Shed => {
                    let mut hs = heights[l].clone();
                    hs.sort_by(f32::total_cmp);
                    let tall = hs[hs.len() / 2].clamp(2.0, 5.0);
                    let garage = sh[l].area >= 15.0 && tall >= 2.2;
                    tags.insert(
                        "building".to_string(),
                        if garage { "garage" } else { "shed" }.to_string(),
                    );
                    tags.insert("height".to_string(), format!("{tall:.1}"));
                }
                super::Kind::Pool => {
                    tags.insert("leisure".to_string(), "swimming_pool".to_string());
                    tags.insert("access".to_string(), "private".to_string());
                }
                _ => unreachable!(),
            }
            tags.insert("source".to_string(), "garden_detail".to_string());
            let id = next_id;
            next_id += 1 << 8;
            let mut nodes: Vec<ProcessedNode> = ring
                .iter()
                .enumerate()
                .map(|(k, &(x, z))| ProcessedNode {
                    id: id + 1 + k as u64,
                    tags: HashMap::new(),
                    x: x0 + x,
                    z: z0 + z,
                })
                .collect();
            let mut first = nodes[0].clone();
            first.id = id + 1;
            nodes.push(first);
            out.push(ProcessedElement::Way(ProcessedWay { id, nodes, tags }));
        }
    }
    out
}

/// A centreline segment and how far from it a candidate may not stand, in blocks.
type Line = ((f64, f64), (f64, f64), f64);

/// Railway tracks whose wagons stand on the ground of the world.
const TRACKS: &[&str] = &[
    "rail",
    "light_rail",
    "tram",
    "subway",
    "narrow_gauge",
    "preserved",
    "monorail",
    "funicular",
];

/// Side of the cells the mapped roads are bucketed in.
const PAVED_CELL: i32 = 32;

/// Where the survey's sheds and pools cannot stand: the mapped roads and paths as wide as
/// they are drawn, railway tracks as wide as a wagon, and the squares, car parks, bus
/// stations and railway yards mapped as areas. On these the laser sees parked vans,
/// buses, wagons, market stalls and awnings, and a square's paving can read as a pool.
///
/// A driveway leads up to a garage's door, or past its side, so the garage reaches onto
/// it: a candidate is only left out for standing mostly on one (a car or van parked
/// there). The garage boxes and carports of a car park mapped as such are what the
/// laser sees there.
struct Paved {
    /// Centreline segments of the roads, paths and tracks, each with the reach of its
    /// surface.
    lines: Vec<Line>,
    /// Whether each of `lines` is a driveway's.
    driveway: Vec<bool>,
    /// Outlines of the squares, car parks, bus stations and railway yards, and of
    /// marinas, harbours and piers.
    areas: Vec<Vec<(f64, f64)>>,
    /// Outlines of mapped ground no garden shed or pool stands on: playgrounds,
    /// parks, pitches, woods, meadows, building sites, school grounds.
    not_gardens: Vec<Vec<(f64, f64)>>,
    /// Outlines of retail, commercial and industrial land: a glass roof or a
    /// blue tarp there is no garden's pool.
    no_pools: Vec<Vec<(f64, f64)>>,
    /// Indices into `lines` by `PAVED_CELL` cell.
    buckets: FnvHashMap<(i32, i32), Vec<usize>>,
}

impl Paved {
    fn of(existing: &[ProcessedElement], scale: f64) -> Self {
        let xz = |n: &ProcessedNode| (f64::from(n.x) + 0.5, f64::from(n.z) + 0.5);
        let closed = |w: &ProcessedWay| {
            w.nodes.len() >= 4
                && w.nodes.first().map(|n| (n.x, n.z)) == w.nodes.last().map(|n| (n.x, n.z))
        };
        // Drawn on the ground, not under it or over it.
        let on_ground = |t: &HashMap<String, String>| {
            !t.get("tunnel").is_some_and(|v| v != "no")
                && !t.get("bridge").is_some_and(|v| v != "no")
                && !t.get("indoor").is_some_and(|v| v != "no")
                && t.get("layer")
                    .and_then(|l| l.parse::<i32>().ok())
                    .is_none_or(|l| l == 0)
        };
        let is_area = |t: &HashMap<String, String>| {
            let garages = matches!(
                t.get("parking").map(String::as_str),
                Some("garage_boxes" | "carports")
            );
            (t.contains_key("highway") && t.get("area").map(String::as_str) == Some("yes"))
                || (matches!(
                    t.get("amenity").map(String::as_str),
                    Some("parking" | "bus_station")
                ) && !t.contains_key("building")
                    && !garages)
                || t.get("landuse").map(String::as_str) == Some("railway")
        };
        // A wagon is some 3 m across, so half of that and the half block a road gets.
        let track_reach = (1.5 * scale).max(1.0) + 0.5;
        let mut paved = Paved {
            lines: Vec::new(),
            driveway: Vec::new(),
            areas: Vec::new(),
            not_gardens: Vec::new(),
            no_pools: Vec::new(),
            buckets: FnvHashMap::default(),
        };
        let outers = |el: &ProcessedElement| -> Vec<Vec<(f64, f64)>> {
            match el {
                ProcessedElement::Way(w) if closed(w) => vec![w.nodes.iter().map(xz).collect()],
                ProcessedElement::Relation(r) => r
                    .members
                    .iter()
                    .filter(|m| m.role == ProcessedMemberRole::Outer && closed(&m.way))
                    .map(|m| m.way.nodes.iter().map(xz).collect())
                    .collect(),
                _ => Vec::new(),
            }
        };
        for el in existing {
            let t = el.tags();
            // Weirs, dams, lock gates, piers and bridges stand over water or over a
            // road, on any layer: what the laser sees there is no shed.
            let tag = |k: &str| t.get(k).map(String::as_str);
            let over_water = matches!(tag("waterway"), Some("weir" | "dam" | "lock_gate"))
                || tag("man_made") == Some("pier")
                || t.get("bridge").is_some_and(|v| v != "no");
            if over_water {
                if let ProcessedElement::Way(w) = el {
                    let reach = t.get("highway").map_or(OVER_WATER_REACH, |kind| {
                        f64::from(highway_block_range(kind, t, scale)) + 0.5
                    });
                    for pair in w.nodes.windows(2) {
                        paved.lines.push((xz(&pair[0]), xz(&pair[1]), reach));
                        paved.driveway.push(false);
                    }
                }
                paved.areas.extend(outers(el));
                continue;
            }
            if matches!(tag("leisure"), Some("marina")) || tag("landuse") == Some("harbour") {
                paved.areas.extend(outers(el));
                continue;
            }
            if !on_ground(t) {
                continue;
            }
            if not_a_garden(t) {
                paved.not_gardens.extend(outers(el));
            } else if matches!(tag("landuse"), Some("retail" | "commercial" | "industrial")) {
                paved.no_pools.extend(outers(el));
            }
            match el {
                ProcessedElement::Way(w) if is_area(t) && closed(w) => {
                    paved.areas.push(w.nodes.iter().map(xz).collect());
                }
                ProcessedElement::Way(w)
                    if t.get("railway")
                        .is_some_and(|r| TRACKS.contains(&r.as_str())) =>
                {
                    for pair in w.nodes.windows(2) {
                        paved.lines.push((xz(&pair[0]), xz(&pair[1]), track_reach));
                        paved.driveway.push(false);
                    }
                }
                ProcessedElement::Way(w) => {
                    let Some(kind) = t.get("highway") else {
                        continue;
                    };
                    if matches!(
                        kind.as_str(),
                        "proposed" | "abandoned" | "razed" | "disused" | "platform"
                    ) {
                        continue;
                    }
                    let reach = f64::from(highway_block_range(kind, t, scale)) + 0.5;
                    let driveway = kind == "service"
                        && t.get("service").map(String::as_str) == Some("driveway");
                    for pair in w.nodes.windows(2) {
                        paved.lines.push((xz(&pair[0]), xz(&pair[1]), reach));
                        paved.driveway.push(driveway);
                    }
                }
                ProcessedElement::Relation(r) if is_area(t) => {
                    for m in r
                        .members
                        .iter()
                        .filter(|m| m.role == ProcessedMemberRole::Outer && closed(&m.way))
                    {
                        paved.areas.push(m.way.nodes.iter().map(xz).collect());
                    }
                }
                _ => {}
            }
        }
        for (i, &(a, b, reach)) in paved.lines.iter().enumerate() {
            let cell = |v: f64| (v / f64::from(PAVED_CELL)).floor() as i32;
            let (x0, x1) = (cell(a.0.min(b.0) - reach), cell(a.0.max(b.0) + reach));
            let (z0, z1) = (cell(a.1.min(b.1) - reach), cell(a.1.max(b.1) + reach));
            for cx in x0..=x1 {
                for cz in z0..=z1 {
                    paved.buckets.entry((cx, cz)).or_default().push(i);
                }
            }
        }
        paved
    }

    /// The `lines` whose surface the cell centre (x, z) lies on.
    fn lines_at(&self, x: f64, z: f64) -> impl Iterator<Item = usize> + '_ {
        let cell = (
            (x / f64::from(PAVED_CELL)).floor() as i32,
            (z / f64::from(PAVED_CELL)).floor() as i32,
        );
        self.buckets
            .get(&cell)
            .into_iter()
            .flatten()
            .copied()
            .filter(move |&i| {
                let ((ax, az), (bx, bz), reach) = self.lines[i];
                let (dx, dz) = (bx - ax, bz - az);
                let len2 = dx * dx + dz * dz;
                let t = if len2 > 0.0 {
                    (((x - ax) * dx + (z - az) * dz) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (x - ax - t * dx).hypot(z - az - t * dz) <= reach
            })
    }

    /// Whether the cell centre (x, z) lies on a road, a square or a car park.
    fn covers(&self, x: f64, z: f64) -> bool {
        self.lines_at(x, z).any(|i| !self.driveway[i])
            || self.areas.iter().any(|ring| ring_contains(ring, x, z))
    }

    /// Whether the point (x, z) lies in mapped ground that is no garden, or for a
    /// pool, on land with no gardens' pools.
    fn off_gardens(&self, x: f64, z: f64, pool: bool) -> bool {
        self.not_gardens
            .iter()
            .any(|ring| ring_contains(ring, x, z))
            || (pool && self.no_pools.iter().any(|ring| ring_contains(ring, x, z)))
    }

    /// Whether the cell centre (x, z) lies on a driveway.
    fn on_driveway(&self, x: f64, z: f64) -> bool {
        self.lines_at(x, z).any(|i| self.driveway[i])
    }

    /// Whether more than half of the blocks inside or on the convex outline `ring`
    /// lie on a driveway.
    fn mostly_on_driveway(&self, ring: &[ProcessedNode]) -> bool {
        let pts: Vec<(f64, f64)> = ring
            .iter()
            .map(|n| (f64::from(n.x), f64::from(n.z)))
            .collect();
        let (Some(x0), Some(x1)) = (
            ring.iter().map(|n| n.x).min(),
            ring.iter().map(|n| n.x).max(),
        ) else {
            return false;
        };
        let (Some(z0), Some(z1)) = (
            ring.iter().map(|n| n.z).min(),
            ring.iter().map(|n| n.z).max(),
        ) else {
            return false;
        };
        let (mut all, mut on) = (0usize, 0usize);
        for z in z0..=z1 {
            for x in x0..=x1 {
                let (fx, fz) = (f64::from(x), f64::from(z));
                if in_convex(&pts, fx, fz) {
                    all += 1;
                    on += usize::from(self.on_driveway(fx + 0.5, fz + 0.5));
                }
            }
        }
        2 * on > all
    }
}

/// How far from a weir, dam, lock gate, pier or bridge without a road's width no
/// candidate stands, in blocks.
const OVER_WATER_REACH: f64 = 2.5;

/// Mapped ground no garden's shed or pool stands on: what fills the ground
/// itself (a playground, park, pitch, wood, meadow, building site), school
/// grounds, waste sites and squares. Homes, gardens, allotments, farmyards,
/// garage yards and the land of shops, offices, works, schools and churches
/// keep theirs.
fn not_a_garden(t: &HashMap<String, String>) -> bool {
    let tag = |k: &str| t.get(k).map(String::as_str);
    if t.contains_key("building") || t.contains_key("highway") {
        return false;
    }
    if let Some(leisure) = tag("leisure") {
        return !matches!(leisure, "garden" | "swimming_pool" | "resort");
    }
    if let Some(natural) = tag("natural") {
        return !matches!(natural, "tree" | "tree_row");
    }
    if let Some(landuse) = tag("landuse") {
        return !matches!(
            landuse,
            "residential"
                | "commercial"
                | "retail"
                | "industrial"
                | "education"
                | "religious"
                | "allotments"
                | "farmyard"
                | "garages"
        );
    }
    matches!(
        tag("amenity"),
        Some("school" | "kindergarten" | "waste_disposal" | "recycling")
    ) || tag("place") == Some("square")
}

/// Whether (x, z) lies inside or on a convex ring, wound either way.
fn in_convex(ring: &[(f64, f64)], x: f64, z: f64) -> bool {
    let (mut pos, mut neg) = (false, false);
    for pair in ring.windows(2) {
        let ((ax, az), (bx, bz)) = (pair[0], pair[1]);
        let c = (bx - ax) * (z - az) - (bz - az) * (x - ax);
        pos |= c > 1e-9;
        neg |= c < -1e-9;
    }
    !(pos && neg)
}

/// Even-odd point in a closed ring.
fn ring_contains(ring: &[(f64, f64)], x: f64, z: f64) -> bool {
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let ((xi, zi), (xj, zj)) = (ring[i], ring[j]);
        if (zi > z) != (zj > z) && x < (xj - xi) * (z - zi) / (zj - zi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Keep the candidates that meet nothing mapped: sheds clear of every building,
/// pools clear of every building and every mapped pool, and both off the mapped roads,
/// tracks, squares and car parks (see [`Paved`]), at the roads' drawn width for the
/// world's `scale`, off water (`water` at a block) and out of mapped ground that is
/// no garden. Also returns how many were left out for standing on roads and squares.
pub fn merge(
    candidates: Vec<ProcessedElement>,
    existing: &[ProcessedElement],
    scale: f64,
    water: &dyn Fn(i32, i32) -> bool,
) -> (Vec<ProcessedElement>, usize) {
    let paved = Paved::of(existing, scale);
    let bbox_of = |w: &ProcessedWay| {
        let xs = w.nodes.iter().map(|n| n.x);
        let zs = w.nodes.iter().map(|n| n.z);
        (
            xs.clone().min().unwrap_or(0),
            zs.clone().min().unwrap_or(0),
            xs.max().unwrap_or(0),
            zs.max().unwrap_or(0),
        )
    };
    let mut blockers: Vec<(i32, i32, i32, i32, bool)> = Vec::new();
    for el in existing {
        let mut push = |w: &ProcessedWay, building: bool| {
            if w.nodes.len() >= 3 {
                let (a, b, c, d) = bbox_of(w);
                blockers.push((a, b, c, d, building));
            }
        };
        let tags = el.tags();
        let building = tags.contains_key("building") || tags.contains_key("building:part");
        let pool = tags.get("leisure").map(String::as_str) == Some("swimming_pool");
        if !building && !pool {
            continue;
        }
        match el {
            ProcessedElement::Way(w) => push(w, building),
            ProcessedElement::Relation(r) => {
                for m in &r.members {
                    push(&m.way, building);
                }
            }
            ProcessedElement::Node(_) => {}
        }
    }
    let mut on_paved = 0;
    let kept = candidates
        .into_iter()
        .filter(|el| {
            let ProcessedElement::Way(w) = el else {
                return false;
            };
            let is_pool = w.tags.contains_key("leisure");
            let (a, b, c, d) = bbox_of(w);
            let n = w.nodes.len().max(1) as f64;
            let centre = (
                w.nodes.iter().map(|p| f64::from(p.x)).sum::<f64>() / n + 0.5,
                w.nodes.iter().map(|p| f64::from(p.z)).sum::<f64>() / n + 0.5,
            );
            let points = || {
                w.nodes
                    .iter()
                    .map(|p| (f64::from(p.x) + 0.5, f64::from(p.z) + 0.5))
                    .chain(std::iter::once(centre))
            };
            // On a driveway only where most of it stands on one.
            let on_paving =
                points().any(|(x, z)| paved.covers(x, z)) || paved.mostly_on_driveway(&w.nodes);
            let mapped = blockers.iter().any(|&(ba, bb, bc, bd, building)| {
                let overlaps = a <= bc && c >= ba && b <= bd && d >= bb;
                // A shed only yields to buildings; a pool to buildings and pools.
                overlaps && (building || is_pool)
            });
            on_paved += usize::from(on_paving && !mapped);
            // A boat, a pontoon or a weir is no shed, a patch of sea no pool.
            let wet = points().any(|(x, z)| water(x.floor() as i32, z.floor() as i32));
            let off_gardens = paved.off_gardens(centre.0, centre.1, is_pool);
            !on_paving && !mapped && !wet && !off_gardens
        })
        .collect();
    (kept, on_paved)
}

/// Convex hull of the cells, counter-clockwise, through block coordinates: an
/// outline's nodes are blocks of its wall, so a shed four blocks wide runs from
/// its first block to its fourth. For a rectangular shed that is its rectangle;
/// for a round pool, its outline.
fn hull(cells: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut pts: Vec<(i32, i32)> = cells.to_vec();
    pts.sort_unstable();
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (i32, i32), a: (i32, i32), b: (i32, i32)| {
        i64::from(a.0 - o.0) * i64::from(b.1 - o.1) - i64::from(a.1 - o.1) * i64::from(b.0 - o.0)
    };
    let mut lower: Vec<(i32, i32)> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<(i32, i32)> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_of_a_block_rectangle_is_its_corners() {
        let cells: Vec<(i32, i32)> = (0..4).flat_map(|x| (0..3).map(move |z| (x, z))).collect();
        let mut h = hull(&cells);
        h.sort_unstable();
        assert_eq!(h, vec![(0, 0), (0, 2), (3, 0), (3, 2)]);
    }

    fn way(id: u64, tags: &[(&str, &str)], x0: i32, z0: i32, x1: i32, z1: i32) -> ProcessedElement {
        let node = |k: u64, x: i32, z: i32| ProcessedNode {
            id: id + k,
            tags: HashMap::new(),
            x,
            z,
        };
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes: vec![
                node(1, x0, z0),
                node(2, x1, z0),
                node(3, x1, z1),
                node(4, x0, z1),
                node(1, x0, z0),
            ],
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    #[test]
    fn mapped_buildings_and_pools_win() {
        let existing = vec![
            way(1, &[("building", "house")], 0, 0, 10, 10),
            way(2, &[("leisure", "swimming_pool")], 30, 30, 36, 34),
            way(3, &[("highway", "residential")], 50, 0, 50, 40),
        ];
        let cands = vec![
            way(ID_BASE, &[("building", "shed")], 9, 9, 12, 12), // touches the house
            way(ID_BASE + 256, &[("building", "shed")], 20, 20, 23, 23), // free
            way(
                ID_BASE + 512,
                &[("leisure", "swimming_pool")],
                31,
                31,
                34,
                33,
            ), // mapped
            way(
                ID_BASE + 768,
                &[("leisure", "swimming_pool")],
                40,
                40,
                45,
                44,
            ), // free
            way(ID_BASE + 1024, &[("building", "shed")], 32, 31, 34, 33), // on a mapped pool
        ];
        let kept: Vec<u64> = merge(cands, &existing, 1.0, &|_, _| false)
            .0
            .into_iter()
            .map(|e| match e {
                ProcessedElement::Way(w) => w.id,
                _ => 0,
            })
            .collect();
        assert_eq!(kept, vec![ID_BASE + 256, ID_BASE + 768, ID_BASE + 1024]);
    }

    // A boat in a harbour, a weir in a river and anything on the land cover's water
    // are no sheds; the climbing frame of a playground is none either, and a glass
    // roof over a shop's yard no pool. A garden's shed and pool, and a works' shed,
    // are kept.
    #[test]
    fn nothing_stands_on_water_or_on_mapped_ground_that_is_no_garden() {
        let weir = ProcessedElement::Way(ProcessedWay {
            id: 5,
            nodes: [(1, 100), (2, 110)]
                .iter()
                .map(|&(id, x)| ProcessedNode {
                    id,
                    tags: HashMap::new(),
                    x,
                    z: 10,
                })
                .collect(),
            tags: [("waterway".to_string(), "weir".to_string())].into(),
        });
        let existing = vec![
            weir,
            way(1, &[("leisure", "marina")], 0, 0, 30, 30),
            way(2, &[("leisure", "playground")], 40, 0, 60, 20),
            way(3, &[("landuse", "retail")], 0, 40, 30, 70),
            way(4, &[("leisure", "garden")], 40, 40, 70, 70),
        ];
        let cands = vec![
            way(ID_BASE, &[("building", "shed")], 10, 10, 14, 13), // a boat in the marina
            way(ID_BASE + 256, &[("building", "garage")], 102, 9, 108, 12), // the weir
            way(ID_BASE + 512, &[("building", "shed")], 48, 8, 51, 11), // a climbing frame
            way(
                ID_BASE + 768,
                &[("leisure", "swimming_pool")],
                10,
                50,
                15,
                54,
            ), // a glass roof at the shop
            way(ID_BASE + 1024, &[("building", "shed")], 20, 50, 23, 53), // the shop's shed
            way(ID_BASE + 1280, &[("building", "shed")], 45, 45, 48, 48), // the garden's
            way(
                ID_BASE + 1536,
                &[("leisure", "swimming_pool")],
                55,
                55,
                60,
                59,
            ), // the garden's pool
            way(ID_BASE + 1792, &[("building", "shed")], 120, 120, 123, 123), // on the sea
        ];
        let sea = |x: i32, z: i32| x >= 115 && z >= 115;
        let kept: Vec<u64> = merge(cands, &existing, 1.0, &sea)
            .0
            .into_iter()
            .map(|e| match e {
                ProcessedElement::Way(w) => w.id,
                _ => 0,
            })
            .collect();
        assert_eq!(kept, vec![ID_BASE + 1024, ID_BASE + 1280, ID_BASE + 1536]);
    }

    // The laser saw a van parked in the street and a market's awnings on a square; a
    // shed beside the street is kept.
    #[test]
    fn nothing_stands_on_mapped_roads_or_squares() {
        let road = ProcessedElement::Way(ProcessedWay {
            id: 1,
            nodes: [(1, 0), (2, 100)]
                .iter()
                .map(|&(id, x)| ProcessedNode {
                    id,
                    tags: HashMap::new(),
                    x,
                    z: 20,
                })
                .collect(),
            tags: [("highway", "residential")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        });
        let existing = vec![
            road,
            way(
                3,
                &[("highway", "pedestrian"), ("area", "yes")],
                0,
                40,
                30,
                70,
            ),
            way(4, &[("amenity", "parking")], 60, 40, 90, 70),
        ];
        let cands = vec![
            way(ID_BASE, &[("building", "shed")], 10, 19, 13, 21), // on the street
            way(ID_BASE + 256, &[("building", "shed")], 30, 24, 33, 27), // beside it
            way(
                ID_BASE + 512,
                &[("leisure", "swimming_pool")],
                10,
                50,
                14,
                53,
            ), // on the square
            way(ID_BASE + 768, &[("building", "garage")], 70, 50, 74, 55), // in the car park
            way(
                ID_BASE + 1024,
                &[("leisure", "swimming_pool")],
                40,
                80,
                45,
                84,
            ), // in a garden
        ];
        let kept: Vec<u64> = merge(cands, &existing, 1.0, &|_, _| false)
            .0
            .into_iter()
            .map(|e| match e {
                ProcessedElement::Way(w) => w.id,
                _ => 0,
            })
            .collect();
        assert_eq!(kept, vec![ID_BASE + 256, ID_BASE + 1024]);
    }

    // A garage at the end of its driveway, the driveway drawn up to its door, a garage
    // the driveway passes, and the garages of a garage-box court are kept; a van
    // parked on a driveway is not.
    #[test]
    fn garages_stand_at_the_end_of_their_driveways() {
        let line = |id: u64, tags: &[(&str, &str)], from: (i32, i32), to: (i32, i32)| {
            ProcessedElement::Way(ProcessedWay {
                id,
                nodes: [(1, from), (2, to)]
                    .iter()
                    .map(|&(k, (x, z))| ProcessedNode {
                        id: id + k,
                        tags: HashMap::new(),
                        x,
                        z,
                    })
                    .collect(),
                tags: tags
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            })
        };
        let driveway = [("highway", "service"), ("service", "driveway")];
        let existing = vec![
            // From the street at z = 0 up to the garage door at z = 20.
            line(1, &driveway, (10, 0), (10, 20)),
            line(4, &driveway, (40, 0), (40, 20)),
            way(
                7,
                &[("amenity", "parking"), ("parking", "garage_boxes")],
                60,
                0,
                90,
                30,
            ),
        ];
        let cands = vec![
            way(ID_BASE, &[("building", "garage")], 8, 20, 13, 26), // its garage
            way(ID_BASE + 256, &[("building", "garage")], 39, 8, 41, 13), // the van
            way(ID_BASE + 512, &[("building", "garage")], 65, 5, 70, 10), // a box
            way(ID_BASE + 768, &[("building", "garage")], 12, 5, 16, 10), // beside
        ];
        let (kept, on_paved) = merge(cands, &existing, 1.0, &|_, _| false);
        let kept: Vec<u64> = kept
            .into_iter()
            .map(|e| match e {
                ProcessedElement::Way(w) => w.id,
                _ => 0,
            })
            .collect();
        assert_eq!(kept, vec![ID_BASE, ID_BASE + 512, ID_BASE + 768]);
        assert_eq!(on_paved, 1);
    }

    // A wagon standing on a siding and a bus in its station are no sheds; a shed
    // behind the station is.
    #[test]
    fn nothing_stands_on_tracks_or_in_bus_stations() {
        let track = ProcessedElement::Way(ProcessedWay {
            id: 1,
            nodes: [(1, 0), (2, 100)]
                .iter()
                .map(|&(id, x)| ProcessedNode {
                    id,
                    tags: HashMap::new(),
                    x,
                    z: 20,
                })
                .collect(),
            tags: [("railway", "rail"), ("service", "siding")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        });
        let existing = vec![track, way(2, &[("amenity", "bus_station")], 0, 40, 30, 70)];
        let cands = vec![
            way(ID_BASE, &[("building", "shed")], 10, 19, 22, 21), // the wagon
            way(ID_BASE + 256, &[("building", "garage")], 10, 50, 22, 53), // the bus
            way(ID_BASE + 512, &[("building", "shed")], 40, 50, 43, 53), // behind
        ];
        let (kept, on_paved) = merge(cands, &existing, 1.0, &|_, _| false);
        let kept: Vec<u64> = kept
            .into_iter()
            .map(|e| match e {
                ProcessedElement::Way(w) => w.id,
                _ => 0,
            })
            .collect();
        assert_eq!(kept, vec![ID_BASE + 512]);
        assert_eq!(on_paved, 2);
    }
}
