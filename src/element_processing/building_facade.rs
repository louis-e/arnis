//! Street and neighbor awareness: a `FacadePlan` classifies each wall
//! segment against the road and footprint bitmaps (street, rear, party wall,
//! open). Computed from precomputed global inputs only, never world reads,
//! so tile-parallel output stays deterministic.

use fnv::{FnvHashMap, FnvHashSet};

use crate::bresenham::bresenham_line;
use crate::element_processing::buildings::{compute_building_centroid, compute_outward_normal};
use crate::floodfill_cache::{CoordinateBitmap, FloodFillCache};
use crate::osm_parser::ProcessedWay;

/// Farthest outward distance sampled when testing for a fronting street.
fn street_setback_max(scale: f64) -> i32 {
    ((8.0 * scale).round() as i32).clamp(8, 24)
}

/// Fraction of a segment's columns that must touch a neighbor footprint for
/// the whole segment to classify as a party wall.
const PARTY_SEGMENT_MIN_FRACTION: f32 = 0.5;

/// Buildings smaller than this keep an empty plan (huts, sheds).
pub const MIN_FACADE_FOOTPRINT: usize = 12;

/// Minimum length for both legs of a street corner.
fn corner_min_seg_len(scale: f64) -> i32 {
    ((6.0 * scale) as i32).max(6)
}

/// Per-column facade context; the default reproduces legacy behavior.
#[derive(Copy, Clone)]
pub struct ColumnFacade {
    pub party: bool,
    pub street: bool,
}

impl Default for ColumnFacade {
    fn default() -> Self {
        Self {
            party: false,
            street: true,
        }
    }
}

/// Shared read-only context handed to the building generator.
pub struct BuildingContext<'a> {
    pub flood_fill_cache: &'a FloodFillCache,
    pub building_passages: &'a CoordinateBitmap,
    pub road_mask: &'a CoordinateBitmap,
    pub building_footprints: &'a CoordinateBitmap,
    /// group_seed -> sorted member way ids; only groups with >= 2 members.
    pub group_members: &'a FnvHashMap<u64, Vec<u64>>,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FacadeClass {
    /// Faces a rendered road/path surface within the setback distance.
    Street,
    /// No street on this side while another side has one.
    Rear,
    /// Attached to a neighboring building (terraced row, sloppy overlap).
    Party,
    /// Free-standing side with no street anywhere (rural default).
    Open,
}

/// Classification of one wall segment (nodes\[i\] → nodes\[i+1\]).
pub struct SegmentFacade {
    pub class: FacadeClass,
    /// Minimum sampled outward distance to a road surface, if any.
    pub road_dist: Option<i32>,
    /// Axis-snapped outward normal.
    pub normal: (i32, i32),
    /// Step direction along the segment.
    pub tangent: (i32, i32),
    /// Chebyshev length of the segment.
    pub len: i32,
}

/// A street corner: two consecutive perpendicular street-facing segments.
pub struct CornerPlan {
    pub vertex: (i32, i32),
    pub seg_a: usize,
    pub seg_b: usize,
}

/// Where a facade sign belongs. Only the building generator knows the wall base, the floor
/// grammar and where the entrance ended up.
#[derive(Copy, Clone, Debug)]
pub struct FacadeAnchor {
    /// Entrance column if there is one, else the middle of the street-facing wall.
    pub x: i32,
    pub z: i32,
    /// Axis-snapped outward normal of that wall.
    pub normal: (i32, i32),
    /// Fascia band, clear of the storefront glazing, awning and entrance canopy.
    pub fascia_y: i32,
    /// Absolute Y for a house-number plate, beside the door at door height.
    pub number_y: i32,
    /// Door column, if the building has an entrance on this wall.
    pub door: Option<(i32, i32)>,
}

/// Per-building facade classification, computed once before wall placement.
pub struct FacadePlan {
    /// Aligned with node pairs: segments\[i\] covers nodes\[i\]..nodes\[i+1\].
    /// None for degenerate segments (zero normal).
    pub segments: Vec<Option<SegmentFacade>>,
    party_columns: FnvHashSet<(i32, i32)>,
    street_columns: FnvHashSet<(i32, i32)>,
    /// Keep-clear columns, filled by the entrance planner.
    pub door_columns: FnvHashSet<(i32, i32)>,
    /// Street segment closest to a road (tie-break: longer, then first).
    pub front_segment: Option<usize>,
    pub corner: Option<CornerPlan>,
    pub has_any_street: bool,
}

impl FacadePlan {
    pub fn empty() -> Self {
        Self {
            segments: Vec::new(),
            party_columns: FnvHashSet::default(),
            street_columns: FnvHashSet::default(),
            door_columns: FnvHashSet::default(),
            front_segment: None,
            corner: None,
            has_any_street: false,
        }
    }

    #[inline]
    pub fn is_party(&self, x: i32, z: i32) -> bool {
        !self.party_columns.is_empty() && self.party_columns.contains(&(x, z))
    }

    #[inline]
    pub fn is_street(&self, x: i32, z: i32) -> bool {
        self.street_columns.contains(&(x, z))
    }

    #[inline]
    pub fn is_door(&self, x: i32, z: i32) -> bool {
        !self.door_columns.is_empty() && self.door_columns.contains(&(x, z))
    }

    pub fn mark_door_column(&mut self, x: i32, z: i32) {
        self.door_columns.insert((x, z));
    }
}

/// Classifies every wall segment. `own_cells` must include the building's
/// own cells plus its part group mates.
pub fn compute_facade_plan(
    element: &ProcessedWay,
    ctx: &BuildingContext<'_>,
    scale: f64,
    own_cells: &FnvHashSet<(i32, i32)>,
) -> FacadePlan {
    let Some((cx, cz)) = compute_building_centroid(&element.nodes) else {
        return FacadePlan::empty();
    };
    let setback_max = street_setback_max(scale);

    let mut segments: Vec<Option<SegmentFacade>> = Vec::new();
    let mut party_columns: FnvHashSet<(i32, i32)> = FnvHashSet::default();
    let mut segment_columns: Vec<Vec<(i32, i32)>> = Vec::new();

    let mut previous_node: Option<(i32, i32)> = None;
    for node in &element.nodes {
        let (x2, z2) = (node.x, node.z);
        if let Some((x1, z1)) = previous_node {
            let (nx, nz) = compute_outward_normal(x1, z1, x2, z2, cx, cz);
            if nx == 0 && nz == 0 {
                segments.push(None);
                segment_columns.push(Vec::new());
                previous_node = Some((x2, z2));
                continue;
            }
            let tangent = ((x2 - x1).signum(), (z2 - z1).signum());
            let len = (x2 - x1).abs().max((z2 - z1).abs());
            let points: Vec<(i32, i32)> = bresenham_line(x1, 0, z1, x2, 0, z2)
                .into_iter()
                .map(|(x, _, z)| (x, z))
                .collect();

            // Party detection: outward cells at depth 1-2 belonging to a
            // foreign footprint.
            let mut party_cols = 0usize;
            for &(bx, bz) in &points {
                let is_party = (1..=2).any(|d| {
                    let c = (bx + nx * d, bz + nz * d);
                    ctx.building_footprints.contains(c.0, c.1) && !own_cells.contains(&c)
                });
                if is_party {
                    party_columns.insert((bx, bz));
                    party_cols += 1;
                }
            }
            let party_fraction = if points.is_empty() {
                0.0
            } else {
                party_cols as f32 / points.len() as f32
            };

            // Street detection: march outward from sample points along the
            // segment; a majority of samples must reach road surface.
            let sample_idx: Vec<usize> = if points.len() > 24 {
                (0..points.len()).step_by(8).collect()
            } else {
                let n = points.len();
                let mut v = vec![n / 4, n / 2, (3 * n) / 4];
                v.dedup();
                v
            };
            let mut hits = 0usize;
            let mut min_dist: Option<i32> = None;
            for &i in &sample_idx {
                let (bx, bz) = points[i.min(points.len().saturating_sub(1))];
                for d in 1..=setback_max {
                    let (px, pz) = (bx + nx * d, bz + nz * d);
                    if ctx.road_mask.contains(px, pz) {
                        hits += 1;
                        min_dist = Some(min_dist.map_or(d, |m: i32| m.min(d)));
                        break;
                    }
                    // A neighbour in the way means the road behind it belongs
                    // to that building, not to this wall.
                    if ctx.building_footprints.contains(px, pz) && !own_cells.contains(&(px, pz)) {
                        break;
                    }
                }
            }
            let needed = if len < 6 {
                1
            } else {
                sample_idx.len().div_ceil(2)
            };
            let is_street = hits >= needed && min_dist.is_some();

            let class = if party_fraction >= PARTY_SEGMENT_MIN_FRACTION {
                // Party beats street: a road behind the attached neighbor
                // must not open a storefront into a shared wall.
                FacadeClass::Party
            } else if is_street {
                FacadeClass::Street
            } else {
                FacadeClass::Open // placeholder; post-pass may demote to Rear
            };

            segments.push(Some(SegmentFacade {
                class,
                road_dist: if is_street { min_dist } else { None },
                normal: (nx, nz),
                tangent,
                len,
            }));
            segment_columns.push(points);
        }
        previous_node = Some((x2, z2));
    }

    let has_any_street = segments
        .iter()
        .flatten()
        .any(|s| s.class == FacadeClass::Street);

    // Non-street, non-party sides read as Rear once any street exists.
    if has_any_street {
        for seg in segments.iter_mut().flatten() {
            if seg.class == FacadeClass::Open {
                seg.class = FacadeClass::Rear;
            }
        }
    }

    // Street columns: bresenham columns of street segments, minus party cells.
    let mut street_columns: FnvHashSet<(i32, i32)> = FnvHashSet::default();
    for (seg, cols) in segments.iter().zip(&segment_columns) {
        if seg.as_ref().is_some_and(|s| s.class == FacadeClass::Street) {
            street_columns.extend(cols.iter().filter(|c| !party_columns.contains(*c)));
        }
    }

    // Front segment: street side closest to the road, longer wins ties.
    let mut front_segment: Option<usize> = None;
    for (i, seg) in segments.iter().enumerate() {
        let Some(s) = seg else { continue };
        if s.class != FacadeClass::Street {
            continue;
        }
        let better = match front_segment.and_then(|j| segments[j].as_ref()) {
            None => true,
            Some(best) => match (s.road_dist, best.road_dist) {
                (Some(a), Some(b)) => a < b || (a == b && s.len > best.len),
                (Some(_), None) => true,
                _ => false,
            },
        };
        if better {
            front_segment = Some(i);
        }
    }

    let corner = detect_street_corner(element, &segments, own_cells, ctx, scale);

    FacadePlan {
        segments,
        party_columns,
        street_columns,
        door_columns: FnvHashSet::default(),
        front_segment,
        corner,
        has_any_street,
    }
}

/// Two consecutive street segments with perpendicular normals and a convex
/// shared vertex make an urban corner.
fn detect_street_corner(
    element: &ProcessedWay,
    segments: &[Option<SegmentFacade>],
    own_cells: &FnvHashSet<(i32, i32)>,
    ctx: &BuildingContext<'_>,
    scale: f64,
) -> Option<CornerPlan> {
    let min_len = corner_min_seg_len(scale);
    // The wrap pair (last segment -> first) only exists on closed rings.
    let closed = match (element.nodes.first(), element.nodes.last()) {
        (Some(a), Some(b)) => a.x == b.x && a.z == b.z,
        _ => false,
    };
    let mut best: Option<(i32, CornerPlan)> = None;
    for i in 0..segments.len() {
        let j = (i + 1) % segments.len();
        if j < i && !closed {
            continue;
        }
        let (Some(a), Some(b)) = (&segments[i], &segments[j]) else {
            continue;
        };
        if a.class != FacadeClass::Street || b.class != FacadeClass::Street {
            continue;
        }
        if a.len < min_len || b.len < min_len {
            continue;
        }
        // Perpendicular, distinct normals.
        if a.normal.0 * b.normal.0 + a.normal.1 * b.normal.1 != 0 {
            continue;
        }
        // Shared vertex is nodes[j] (end of segment i).
        let vertex_node = element.nodes.get(j)?;
        let (vx, vz) = (vertex_node.x, vertex_node.z);
        // Convexity: the diagonal outward cell is outside the building.
        let dx = vx + a.normal.0 + b.normal.0;
        let dz = vz + a.normal.1 + b.normal.1;
        if own_cells.contains(&(dx, dz)) || ctx.building_footprints.contains(dx, dz) {
            continue;
        }
        let score = a.road_dist.unwrap_or(i32::MAX / 2) + b.road_dist.unwrap_or(i32::MAX / 2);
        if best.as_ref().is_none_or(|(s, _)| score < *s) {
            best = Some((
                score,
                CornerPlan {
                    vertex: (vx, vz),
                    seg_a: i,
                    seg_b: j,
                },
            ));
        }
    }
    best.map(|(_, plan)| plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{
        bitmap_with_rect, rect_way, test_editor,
    };
    use crate::floodfill_cache::FloodFillCache;

    fn own_cells_rect(x0: i32, z0: i32, x1: i32, z1: i32) -> FnvHashSet<(i32, i32)> {
        let mut set = FnvHashSet::default();
        for x in x0..=x1 {
            for z in z0..=z1 {
                set.insert((x, z));
            }
        }
        set
    }

    struct Fixtures {
        cache: FloodFillCache,
        passages: CoordinateBitmap,
        groups: FnvHashMap<u64, Vec<u64>>,
    }

    impl Fixtures {
        fn new() -> Self {
            Self {
                cache: FloodFillCache::new(),
                passages: CoordinateBitmap::new_empty(),
                groups: FnvHashMap::default(),
            }
        }
        fn ctx<'a>(
            &'a self,
            road: &'a CoordinateBitmap,
            footprints: &'a CoordinateBitmap,
        ) -> BuildingContext<'a> {
            BuildingContext {
                flood_fill_cache: &self.cache,
                building_passages: &self.passages,
                road_mask: road,
                building_footprints: footprints,
                group_members: &self.groups,
            }
        }
    }

    // rect_way segments: 0 = north (z0), 1 = east (x1), 2 = south (z1), 3 = west (x0)
    fn seg_class(plan: &FacadePlan, i: usize) -> FacadeClass {
        plan.segments[i].as_ref().unwrap().class
    }

    #[test]
    fn street_rear_and_open_classification() {
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        // keep the editor helper linked into this module's test build
        let _ = test_editor(&xz);
        let road = bitmap_with_rect(&xz, 0, 12, 59, 14);
        let footprints = CoordinateBitmap::new(&xz);
        let fx = Fixtures::new();
        let way = rect_way(7, 20, 20, 40, 32, &[("building", "house")]);
        let own = own_cells_rect(20, 20, 40, 32);

        let plan = compute_facade_plan(&way, &fx.ctx(&road, &footprints), 1.0, &own);
        assert!(plan.has_any_street);
        assert_eq!(seg_class(&plan, 0), FacadeClass::Street);
        assert_eq!(seg_class(&plan, 2), FacadeClass::Rear);
        assert_eq!(plan.front_segment, Some(0));

        // Without any road everything is Open.
        let no_road = CoordinateBitmap::new(&xz);
        let plan = compute_facade_plan(&way, &fx.ctx(&no_road, &footprints), 1.0, &own);
        assert!(!plan.has_any_street);
        assert_eq!(seg_class(&plan, 0), FacadeClass::Open);
        assert_eq!(seg_class(&plan, 2), FacadeClass::Open);
    }

    #[test]
    fn attached_neighbor_makes_a_party_wall_unless_grouped() {
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let road = bitmap_with_rect(&xz, 0, 12, 59, 14);
        let footprints = bitmap_with_rect(&xz, 41, 20, 52, 32);
        let fx = Fixtures::new();
        let way = rect_way(8, 20, 20, 40, 32, &[("building", "house")]);
        let own = own_cells_rect(20, 20, 40, 32);

        let plan = compute_facade_plan(&way, &fx.ctx(&road, &footprints), 1.0, &own);
        assert_eq!(seg_class(&plan, 1), FacadeClass::Party);
        assert!(plan.is_party(40, 26));

        // Same cells inside own_cells (a building:part group mate): no party wall.
        let mut own_with_mate = own.clone();
        own_with_mate.extend(own_cells_rect(41, 20, 52, 32));
        let plan = compute_facade_plan(&way, &fx.ctx(&road, &footprints), 1.0, &own_with_mate);
        assert_ne!(seg_class(&plan, 1), FacadeClass::Party);
        assert!(!plan.is_party(40, 26));
    }

    #[test]
    fn perpendicular_streets_make_a_corner() {
        let xz = XZBBox::rect_from_xz_lengths(60.0, 60.0).unwrap();
        let mut road = bitmap_with_rect(&xz, 0, 12, 59, 14);
        for x in 12..=14 {
            for z in 0..=59 {
                road.set(x, z);
            }
        }
        let footprints = CoordinateBitmap::new(&xz);
        let fx = Fixtures::new();
        let way = rect_way(9, 20, 20, 40, 32, &[("building", "commercial")]);
        let own = own_cells_rect(20, 20, 40, 32);

        let plan = compute_facade_plan(&way, &fx.ctx(&road, &footprints), 1.0, &own);
        let corner = plan
            .corner
            .expect("two perpendicular streets form a corner");
        assert_eq!(corner.vertex, (20, 20));

        // Streets on opposite sides only: no corner.
        let mut opposite = bitmap_with_rect(&xz, 0, 12, 59, 14);
        for x in 0..=59 {
            for z in 38..=40 {
                opposite.set(x, z);
            }
        }
        let plan = compute_facade_plan(&way, &fx.ctx(&opposite, &footprints), 1.0, &own);
        assert!(plan.corner.is_none());
    }
}
