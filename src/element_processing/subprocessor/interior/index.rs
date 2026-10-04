//! Tenants, surrounding areas and overlaps per building, resolved before tiling.

use super::uses::{parse_level, use_from_area, use_from_tags, Tenant, Use};
use crate::coordinate_system::cartesian::XZBBox;
use crate::element_processing::buildings::relation_outer_rings;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode};
use fnv::FnvHashMap;
use std::collections::HashMap;

/// Side of one bucket of the building lookup grid, in blocks.
const BUCKET: i32 = 32;

#[derive(Default)]
pub struct InteriorUseIndex {
    tenants: FnvHashMap<u64, Vec<Tenant>>,
    areas: FnvHashMap<u64, Use>,
    /// Smaller ground-level outlines overlapping a building, whose cells it leaves to them.
    claims: FnvHashMap<u64, Vec<Claim>>,
}

/// A smaller building standing on another's floor, up to its own height.
#[derive(Clone, Debug)]
pub struct Claim {
    pub ring: Vec<(i32, i32)>,
    /// Height of its top in metres; one storey when untagged.
    pub top_m: f64,
}

struct Outline {
    id: u64,
    ring: Vec<(i32, i32)>,
    min: (i32, i32),
    max: (i32, i32),
    /// Stands on the ground rather than starting at an upper level.
    ground: bool,
    area: i64,
    top_m: f64,
}

/// Height of an outline's top in metres from its tags, one storey when untagged.
fn top_metres(tags: &HashMap<String, String>) -> f64 {
    let number = |key: &str| {
        tags.get(key)
            .and_then(|v| v.trim_end_matches('m').trim().parse::<f64>().ok())
            .filter(|v| *v > 0.0)
    };
    number("height")
        .or_else(|| number("building:levels").map(|l| l * 3.0 + 2.0))
        .unwrap_or(4.0)
}

/// Starts above the ground, like a tower part on a podium.
fn elevated(tags: &HashMap<String, String>) -> bool {
    let positive = |key: &str| {
        tags.get(key)
            .and_then(|v| v.trim_end_matches('m').trim().parse::<f64>().ok())
            .is_some_and(|v| v > 0.0)
    };
    positive("building:min_level") || positive("min_height")
}

impl Outline {
    fn new(id: u64, nodes: &[ProcessedNode], tags: &HashMap<String, String>) -> Option<Self> {
        if nodes.len() < 4 {
            return None;
        }
        let ring: Vec<(i32, i32)> = nodes.iter().map(|n| (n.x, n.z)).collect();
        let min = (
            ring.iter().map(|p| p.0).min()?,
            ring.iter().map(|p| p.1).min()?,
        );
        let max = (
            ring.iter().map(|p| p.0).max()?,
            ring.iter().map(|p| p.1).max()?,
        );
        let twice_area: i64 = ring
            .windows(2)
            .map(|w| w[0].0 as i64 * w[1].1 as i64 - w[1].0 as i64 * w[0].1 as i64)
            .sum();
        Some(Self {
            id,
            ring,
            min,
            max,
            ground: !elevated(tags),
            area: twice_area.abs(),
            top_m: top_metres(tags),
        })
    }

    /// Shares floor with `other`: a corner of either lies inside the other, or edges cross.
    fn overlaps(&self, other: &Outline) -> bool {
        self.min.0 <= other.max.0
            && other.min.0 <= self.max.0
            && self.min.1 <= other.max.1
            && other.min.1 <= self.max.1
            && (other
                .ring
                .iter()
                .any(|&(x, z)| point_in_ring(x, z, &self.ring))
                || self
                    .ring
                    .iter()
                    .any(|&(x, z)| point_in_ring(x, z, &other.ring))
                || self.ring.windows(2).any(|a| {
                    other
                        .ring
                        .windows(2)
                        .any(|b| segments_cross(a[0], a[1], b[0], b[1]))
                }))
    }

    fn contains(&self, x: i32, z: i32) -> bool {
        x >= self.min.0
            && x <= self.max.0
            && z >= self.min.1
            && z <= self.max.1
            && point_in_ring(x, z, &self.ring)
    }

    fn center(&self) -> (i32, i32) {
        let n = self.ring.len().saturating_sub(1).max(1) as i64;
        let (sx, sz) = self.ring[..self.ring.len() - 1]
            .iter()
            .fold((0i64, 0i64), |(sx, sz), &(x, z)| {
                (sx + x as i64, sz + z as i64)
            });
        ((sx / n) as i32, (sz / n) as i32)
    }

    fn bbox_area(&self) -> i64 {
        (self.max.0 - self.min.0 + 1) as i64 * (self.max.1 - self.min.1 + 1) as i64
    }
}

/// True when the segments properly cross, each splitting the other's ends.
fn segments_cross(a: (i32, i32), b: (i32, i32), c: (i32, i32), d: (i32, i32)) -> bool {
    let side = |p: (i32, i32), q: (i32, i32), r: (i32, i32)| {
        ((q.0 - p.0) as i64 * (r.1 - p.1) as i64 - (q.1 - p.1) as i64 * (r.0 - p.0) as i64).signum()
    };
    side(a, b, c) * side(a, b, d) < 0 && side(c, d, a) * side(c, d, b) < 0
}

/// True when a cell lies inside a ring or on its outline.
pub(super) fn covers(ring: &[(i32, i32)], x: i32, z: i32) -> bool {
    if ring.len() < 3 {
        return false;
    }
    let (min_x, max_x) = ring
        .iter()
        .fold((i32::MAX, i32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
    let (min_z, max_z) = ring
        .iter()
        .fold((i32::MAX, i32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
    if x < min_x - 1 || x > max_x + 1 || z < min_z - 1 || z > max_z + 1 {
        return false;
    }
    if point_in_ring(x, z, ring) {
        return true;
    }
    // On the outline: within reach of an edge, where its wall stands.
    let (px, pz) = (x as f64, z as f64);
    ring.windows(2).any(|w| {
        let (ax, az) = (w[0].0 as f64, w[0].1 as f64);
        let (bx, bz) = (w[1].0 as f64, w[1].1 as f64);
        let (dx, dz) = (bx - ax, bz - az);
        let len2 = dx * dx + dz * dz;
        let t = if len2 > 0.0 {
            (((px - ax) * dx + (pz - az) * dz) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (cx, cz) = (ax + t * dx - px, az + t * dz - pz);
        cx * cx + cz * cz <= 0.75 * 0.75
    })
}

/// Even-odd test on the block lattice.
fn point_in_ring(x: i32, z: i32, ring: &[(i32, i32)]) -> bool {
    let (xf, zf) = (x as f64, z as f64);
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (xi, zi) = (ring[i].0 as f64, ring[i].1 as f64);
        let (xj, zj) = (ring[j].0 as f64, ring[j].1 as f64);
        if (zi > zf) != (zj > zf) && xf < (xj - xi) * (zf - zi) / (zj - zi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Buckets of building outlines, so each point tests only its neighbours.
struct Grid {
    outlines: Vec<Outline>,
    buckets: FnvHashMap<(i32, i32), Vec<u32>>,
}

impl Grid {
    fn new(outlines: Vec<Outline>) -> Self {
        let mut buckets: FnvHashMap<(i32, i32), Vec<u32>> = FnvHashMap::default();
        for (i, o) in outlines.iter().enumerate() {
            for bx in o.min.0.div_euclid(BUCKET)..=o.max.0.div_euclid(BUCKET) {
                for bz in o.min.1.div_euclid(BUCKET)..=o.max.1.div_euclid(BUCKET) {
                    buckets.entry((bx, bz)).or_default().push(i as u32);
                }
            }
        }
        Self { outlines, buckets }
    }

    fn containing(&self, x: i32, z: i32) -> impl Iterator<Item = &Outline> {
        self.buckets
            .get(&(x.div_euclid(BUCKET), z.div_euclid(BUCKET)))
            .into_iter()
            .flatten()
            .map(|&i| &self.outlines[i as usize])
            .filter(move |o| o.contains(x, z))
    }

    fn within(&self, min: (i32, i32), max: (i32, i32)) -> Vec<&Outline> {
        let mut seen: Vec<u32> = Vec::new();
        for bx in min.0.div_euclid(BUCKET)..=max.0.div_euclid(BUCKET) {
            for bz in min.1.div_euclid(BUCKET)..=max.1.div_euclid(BUCKET) {
                if let Some(ids) = self.buckets.get(&(bx, bz)) {
                    seen.extend(ids);
                }
            }
        }
        seen.sort_unstable();
        seen.dedup();
        seen.into_iter()
            .map(|i| &self.outlines[i as usize])
            .collect()
    }
}

fn is_building(tags: &HashMap<String, String>) -> bool {
    ["building", "building:part"]
        .iter()
        .any(|k| tags.get(*k).is_some_and(|v| v != "no"))
}

fn closed(nodes: &[ProcessedNode]) -> bool {
    nodes.len() >= 4 && nodes.first().map(|n| (n.x, n.z)) == nodes.last().map(|n| (n.x, n.z))
}

/// Campuses outrank land use; smaller areas outrank larger ones.
fn area_rank(use_: Use) -> u8 {
    match use_ {
        Use::Factory | Use::Barn | Use::Shop(_) => 1,
        _ => 2,
    }
}

impl InteriorUseIndex {
    /// An index with nothing in it, for tests that build a context by hand.
    #[cfg(test)]
    pub fn empty() -> &'static Self {
        static EMPTY: once_cell::sync::Lazy<InteriorUseIndex> =
            once_cell::sync::Lazy::new(InteriorUseIndex::default);
        &EMPTY
    }

    pub fn tenants(&self, building_id: u64) -> &[Tenant] {
        self.tenants
            .get(&building_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn area(&self, building_id: u64) -> Option<Use> {
        self.areas.get(&building_id).copied()
    }

    /// Outlines whose cells a building leaves to them.
    pub fn claims(&self, building_id: u64) -> &[Claim] {
        self.claims
            .get(&building_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn build(elements: &[ProcessedElement], xzbbox: &XZBBox) -> Self {
        let mut outlines = Vec::new();
        for element in elements {
            match element {
                ProcessedElement::Way(way) if is_building(&way.tags) => {
                    outlines.extend(Outline::new(way.id, &way.nodes, &way.tags));
                }
                ProcessedElement::Relation(rel) if is_building(&rel.tags) => {
                    for (id, ring) in relation_outer_rings(rel, xzbbox) {
                        outlines.extend(Outline::new(id, &ring, &rel.tags));
                    }
                }
                _ => {}
            }
        }
        if outlines.is_empty() {
            return Self::default();
        }
        let grid = Grid::new(outlines);

        let mut index = Self::default();
        // Where outlines overlap on the ground, the smaller one furnishes the shared
        // floor: a kiosk inside a mall outline, a tower part standing in its podium.
        for o in grid.outlines.iter().filter(|o| o.ground) {
            for other in grid.within(o.min, o.max) {
                if other.id != o.id
                    && other.ground
                    && (other.area, other.id) < (o.area, o.id)
                    && o.overlaps(other)
                {
                    index.claims.entry(o.id).or_default().push(Claim {
                        ring: other.ring.clone(),
                        top_m: other.top_m,
                    });
                }
            }
        }
        // A POI belongs to the smallest ground building around it, as that one furnishes
        // the floor there, and to any raised part above it that its level may reach.
        let add_tenant = |index: &mut Self, use_: Use, x: i32, z: i32, level: Option<i32>| {
            let around: Vec<&Outline> = grid.containing(x, z).collect();
            let ground = around
                .iter()
                .filter(|o| o.ground)
                .min_by_key(|o| (o.area, o.id))
                .map(|o| o.id);
            for o in around {
                if !o.ground || Some(o.id) == ground {
                    index
                        .tenants
                        .entry(o.id)
                        .or_default()
                        .push(Tenant { use_, x, z, level });
                }
            }
        };
        // Best area so far per building: (rank, area size, use).
        let mut best_area: FnvHashMap<u64, (u8, i64, Use)> = FnvHashMap::default();
        let mut add_area = |rings: &[Outline], use_: Use| {
            for ring in rings {
                for o in grid.within(ring.min, ring.max) {
                    let (cx, cz) = o.center();
                    if !rings.iter().any(|r| r.contains(cx, cz)) {
                        continue;
                    }
                    let candidate = (area_rank(use_), -ring.area, use_);
                    let better = best_area
                        .get(&o.id)
                        .is_none_or(|b| (candidate.0, candidate.1) > (b.0, b.1));
                    if better {
                        best_area.insert(o.id, candidate);
                    }
                }
            }
        };

        for element in elements {
            match element {
                ProcessedElement::Node(node) => {
                    if let Some(use_) = use_from_tags(&node.tags) {
                        let level = node.tags.get("level").and_then(|l| parse_level(l));
                        add_tenant(&mut index, use_, node.x, node.z, level);
                    }
                }
                ProcessedElement::Way(way) if !is_building(&way.tags) && closed(&way.nodes) => {
                    let Some(outline) = Outline::new(way.id, &way.nodes, &way.tags) else {
                        continue;
                    };
                    // A small shop area mapped inside a mall is a tenant, a campus is an area.
                    if let Some(use_) = use_from_tags(&way.tags) {
                        let (cx, cz) = outline.center();
                        let fits_inside = grid
                            .containing(cx, cz)
                            .any(|b| b.bbox_area() > outline.bbox_area());
                        if fits_inside {
                            let level = way.tags.get("level").and_then(|l| parse_level(l));
                            add_tenant(&mut index, use_, cx, cz, level);
                            continue;
                        }
                    }
                    if let Some(use_) = use_from_area(&way.tags) {
                        add_area(std::slice::from_ref(&outline), use_);
                    }
                }
                ProcessedElement::Relation(rel) if !is_building(&rel.tags) => {
                    let Some(use_) = use_from_area(&rel.tags) else {
                        continue;
                    };
                    let mut rings: Vec<Vec<ProcessedNode>> = rel
                        .members
                        .iter()
                        .filter(|m| m.role == ProcessedMemberRole::Outer)
                        .map(|m| m.way.nodes.clone())
                        .collect();
                    crate::element_processing::merge_way_segments(&mut rings);
                    let outlines: Vec<Outline> = rings
                        .iter()
                        .filter(|r| r.len() >= 4)
                        .filter_map(|r| Outline::new(rel.id, r, &rel.tags))
                        .collect();
                    add_area(&outlines, use_);
                }
                _ => {}
            }
        }
        index.areas = best_area
            .into_iter()
            .map(|(id, (_, _, u))| (id, u))
            .collect();
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element_processing::building_test_support::{rect_way, tag_map};
    use crate::element_processing::subprocessor::interior::uses::Goods;
    use crate::osm_parser::ProcessedNode;

    fn node(id: u64, x: i32, z: i32, tags: &[(&str, &str)]) -> ProcessedElement {
        ProcessedElement::Node(ProcessedNode {
            id,
            tags: tag_map(tags),
            x,
            z,
        })
    }

    #[test]
    fn a_poi_belongs_to_the_building_around_it() {
        let xz = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let elements = vec![
            ProcessedElement::Way(rect_way(1, 10, 10, 30, 20, &[("building", "yes")])),
            ProcessedElement::Way(rect_way(2, 40, 10, 60, 20, &[("building", "yes")])),
            node(100, 15, 15, &[("shop", "bakery"), ("level", "0")]),
            node(101, 50, 12, &[("amenity", "cafe")]),
            node(102, 80, 80, &[("shop", "books")]),
            node(103, 20, 18, &[("amenity", "bench")]),
        ];
        let index = InteriorUseIndex::build(&elements, &xz);
        assert_eq!(
            index.tenants(1),
            &[Tenant {
                use_: Use::Shop(Goods::Bakery),
                x: 15,
                z: 15,
                level: Some(0),
            }]
        );
        assert_eq!(index.tenants(2).len(), 1);
        assert!(index.tenants(3).is_empty());
    }

    #[test]
    fn a_campus_lends_its_use_to_the_buildings_on_it() {
        let xz = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let elements = vec![
            ProcessedElement::Way(rect_way(1, 10, 10, 30, 20, &[("building", "yes")])),
            ProcessedElement::Way(rect_way(2, 120, 120, 130, 130, &[("building", "yes")])),
            ProcessedElement::Way(rect_way(5, 0, 0, 100, 100, &[("amenity", "school")])),
            ProcessedElement::Way(rect_way(6, 0, 0, 150, 150, &[("landuse", "industrial")])),
        ];
        let index = InteriorUseIndex::build(&elements, &xz);
        assert_eq!(
            index.area(1),
            Some(Use::School),
            "the school outranks the land use"
        );
        assert_eq!(index.area(2), Some(Use::Factory));
    }

    #[test]
    fn a_poi_in_a_pavilion_is_not_a_tenant_of_the_mall_around_it() {
        let xz = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let elements = vec![
            ProcessedElement::Way(rect_way(1, 10, 10, 90, 60, &[("building", "retail")])),
            ProcessedElement::Way(rect_way(2, 40, 30, 50, 40, &[("building", "yes")])),
            node(100, 45, 35, &[("amenity", "cafe")]),
        ];
        let index = InteriorUseIndex::build(&elements, &xz);
        assert_eq!(index.tenants(2).len(), 1);
        assert!(index.tenants(1).is_empty());
        assert_eq!(
            index.claims(1).len(),
            1,
            "the mall leaves the pavilion its floor"
        );
    }

    #[test]
    fn crossing_outlines_overlap_without_a_corner_inside() {
        let xz = XZBBox::rect_from_xz_lengths(200.0, 200.0).unwrap();
        let elements = vec![
            ProcessedElement::Way(rect_way(1, 10, 30, 90, 40, &[("building", "yes")])),
            ProcessedElement::Way(rect_way(
                2,
                45,
                10,
                55,
                60,
                &[("building", "yes"), ("building:levels", "2")],
            )),
        ];
        let index = InteriorUseIndex::build(&elements, &xz);
        let claims = index.claims(1);
        assert_eq!(
            claims.len(),
            1,
            "the smaller cross bar claims the shared floor"
        );
        assert_eq!(
            claims[0].top_m, 8.0,
            "two storeys of three metres plus the base"
        );
    }
}
