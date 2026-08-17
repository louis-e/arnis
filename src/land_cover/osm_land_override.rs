//! Trim ESA water back to land where OSM shows the shore is somewhere else.
//!
//! ESA WorldCover pixels are 10 m, and a shoreline pixel that is half water is classified
//! as water, so promenades, quays and the first row of a town end up under water. Within a
//! band of the ESA shore, water becomes land where OSM puts a road or a building on it, or
//! where a mapped water area ends further out. Cells inside OSM water are never touched.
use std::collections::HashMap;

use crate::bresenham::bresenham_line;
use crate::clipping::clip_water_ring_to_bbox;
use crate::coordinate_system::cartesian::XZBBox;
use crate::element_processing::bridges::is_bridge_way;
use crate::element_processing::highways::highway_block_range;
use crate::element_processing::waterways::{
    is_channel_waterway, is_underground_waterway, waterway_width, MAX_WATERWAY_WIDTH,
};
use crate::land_cover::{compute_water_distance, nearest_land_class, LandCoverData, LC_WATER};
use crate::osm_parser::{
    ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay,
};

/// How far from the ESA shore the classification may be corrected. One and a half ESA
/// pixels, which is the mixed-pixel band that gets misread as water.
const SHORE_BAND_M: f64 = 15.0;
const MIN_BAND_CELLS: i32 = 1;
const MAX_BAND_CELLS: i32 = 64;

pub fn apply_osm_land_override(
    land_cover: &mut LandCoverData,
    world_width: usize,
    world_height: usize,
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    scale: f64,
) {
    let width = land_cover.width;
    let height = land_cover.height;
    if width < 2 || height < 2 || world_width < 2 || world_height < 2 {
        return;
    }
    let n = width * height;
    let grid = &land_cover.grid;
    let is_water = |idx: usize| grid[idx / width][idx % width] == LC_WATER;
    if !(0..n).any(is_water) {
        return;
    }

    let map = GridMap {
        min_x: xzbbox.min_x(),
        min_z: xzbbox.min_z(),
        sx: (width as f64 - 1.0) / (world_width as f64 - 1.0),
        sz: (height as f64 - 1.0) / (world_height as f64 - 1.0),
        width,
        height,
    };
    let band = band_cells(land_cover.cells_per_meter);

    // OSM water areas: authoritative for where the shore is, and never trimmed.
    let mut water_area = vec![0u64; n.div_ceil(64)];
    // Mapped channels: never trimmed, but a centreline says nothing about the true width.
    let mut water_line = vec![0u64; n.div_ceil(64)];
    // Roads and buildings, which only stand on land.
    let mut land = vec![0u64; n.div_ceil(64)];
    // Structures that carry a road out over water, which do not.
    let mut over_water = vec![0u64; n.div_ceil(64)];
    let mut any_area = false;
    let mut any_land = false;

    for elem in elements {
        match elem {
            ProcessedElement::Way(way) => {
                if way.nodes.len() < 2 {
                    continue;
                }
                if let Some(half) = over_water_half_width(way, scale) {
                    stamp_line(&mut over_water, &way.nodes, half, &map, xzbbox);
                    if is_ring_closed(&way.nodes) {
                        if let Some(ring) = clip_water_ring_to_bbox(&way.nodes, xzbbox) {
                            fill_rings(&mut over_water, &[ring], &map);
                        }
                    }
                    continue;
                }
                if is_water_area_way(way) {
                    if is_ring_closed(&way.nodes) {
                        if let Some(ring) = clip_water_ring_to_bbox(&way.nodes, xzbbox) {
                            fill_rings(&mut water_area, &[ring], &map);
                            any_area = true;
                        }
                    }
                } else if let Some(kind) = way.tags.get("waterway") {
                    if is_channel_waterway(kind) && !is_underground_waterway(&way.tags) {
                        let half = (waterway_width(kind, &way.tags) / 2).max(0);
                        stamp_line(&mut water_line, &way.nodes, half, &map, xzbbox);
                    }
                } else if let Some(half) = land_line_half_width(way, scale) {
                    stamp_line(&mut land, &way.nodes, half, &map, xzbbox);
                    any_land = true;
                } else if is_building(&way.tags) && is_ring_closed(&way.nodes) {
                    if let Some(ring) = clip_water_ring_to_bbox(&way.nodes, xzbbox) {
                        fill_rings(&mut land, &[ring], &map);
                        any_land = true;
                    }
                }
            }
            ProcessedElement::Relation(rel) => {
                let target = if is_water_area_relation(rel) {
                    any_area = true;
                    &mut water_area
                } else if is_building(&rel.tags) {
                    any_land = true;
                    &mut land
                } else {
                    continue;
                };
                let rings = relation_rings(rel, xzbbox);
                if !rings.is_empty() {
                    fill_rings(target, &rings, &map);
                }
            }
            _ => {}
        }
    }
    if !any_area && !any_land {
        return;
    }

    // A road stamped across a pier is not evidence of land under it.
    for (l, o) in land.iter_mut().zip(over_water.iter()) {
        *l &= !o;
    }
    drop(over_water);

    // Water within `band` steps of ESA land: the rim the classification may have got wrong.
    let rim = dilate(n, width, height, band, |idx| !is_water(idx), is_water);
    // Water reachable from a mapped water area without leaving the water: the same body,
    // past the outline OSM drew for it. Walking over land instead would let a mapped lake
    // condemn a separate body across an isthmus.
    let past_outline = if any_area {
        dilate(
            n,
            width,
            height,
            band,
            |idx| get_bit(&water_area, idx),
            is_water,
        )
    } else {
        vec![0u64; n.div_ceil(64)]
    };

    let radius = band + 2;
    let mut changes: Vec<(u32, u32, u8)> = Vec::new();
    for idx in 0..n {
        if !get_bit(&rim, idx) || get_bit(&water_area, idx) || get_bit(&water_line, idx) {
            continue;
        }
        if !(get_bit(&past_outline, idx) || get_bit(&land, idx)) {
            continue;
        }
        let (x, z) = (idx % width, idx / width);
        if let Some(c) = nearest_land_class(grid, width, height, x, z, radius) {
            changes.push((x as u32, z as u32, c));
        }
    }
    if changes.is_empty() {
        return;
    }
    for &(x, z, c) in &changes {
        land_cover.grid[z as usize][x as usize] = c;
    }
    eprintln!(
        "OSM land override: {} shore cells reclassified from ESA water to land",
        changes.len()
    );
    land_cover.water_distance = compute_water_distance(&land_cover.grid, width, height);
    land_cover.invalidate_water_blend_grid();
}

/// The band in grid cells. Sized in real metres so it holds at any scale and latitude.
fn band_cells(cells_per_meter: f64) -> i32 {
    if !cells_per_meter.is_finite() || cells_per_meter <= 0.0 {
        return MIN_BAND_CELLS;
    }
    ((SHORE_BAND_M * cells_per_meter).round() as i64)
        .clamp(MIN_BAND_CELLS as i64, MAX_BAND_CELLS as i64) as i32
}

/// Cells within `band` 4-connected steps of a seed, expanding only through `pass` cells.
/// Seeds are excluded from the result.
fn dilate(
    n: usize,
    width: usize,
    height: usize,
    band: i32,
    seed: impl Fn(usize) -> bool,
    pass: impl Fn(usize) -> bool,
) -> Vec<u64> {
    let mut seen = vec![0u64; n.div_ceil(64)];
    let mut out = vec![0u64; n.div_ceil(64)];
    let mut frontier: Vec<u32> = Vec::new();
    // Only a seed touching a passable cell can reach anything, so the rest stay out of the
    // frontier. On a mostly-land grid that is the shore rather than every cell.
    for idx in 0..n {
        if !seed(idx) {
            continue;
        }
        set_bit(&mut seen, idx);
        let (x, z) = (idx % width, idx / width);
        let touches = (x > 0 && pass(idx - 1))
            || (x + 1 < width && pass(idx + 1))
            || (z > 0 && pass(idx - width))
            || (z + 1 < height && pass(idx + width));
        if touches {
            frontier.push(idx as u32);
        }
    }
    let mut next: Vec<u32> = Vec::new();
    for _ in 0..band.max(0) {
        next.clear();
        for &idx in &frontier {
            let idx = idx as usize;
            let (x, z) = (idx % width, idx / width);
            let mut around: [Option<usize>; 4] = [None; 4];
            if x > 0 {
                around[0] = Some(idx - 1);
            }
            if x + 1 < width {
                around[1] = Some(idx + 1);
            }
            if z > 0 {
                around[2] = Some(idx - width);
            }
            if z + 1 < height {
                around[3] = Some(idx + width);
            }
            for nb in around.into_iter().flatten() {
                if !get_bit(&seen, nb) && pass(nb) {
                    set_bit(&mut seen, nb);
                    set_bit(&mut out, nb);
                    next.push(nb as u32);
                }
            }
        }
        std::mem::swap(&mut frontier, &mut next);
        if frontier.is_empty() {
            break;
        }
    }
    out
}

struct GridMap {
    min_x: i32,
    min_z: i32,
    sx: f64,
    sz: f64,
    width: usize,
    height: usize,
}

impl GridMap {
    fn gx(&self, x: i32) -> f64 {
        (x - self.min_x) as f64 * self.sx
    }
    fn gz(&self, z: i32) -> f64 {
        (z - self.min_z) as f64 * self.sz
    }
}

/// Even-odd scanline fill of rings given in world coordinates into a grid bitset.
/// Spans are inclusive, matching the fill in `osm_water_override`.
fn fill_rings(bits: &mut [u64], rings: &[Vec<ProcessedNode>], map: &GridMap) {
    let (gw, gh) = (map.width, map.height);
    let mut rows: HashMap<usize, Vec<f64>> = HashMap::new();
    for ring in rings {
        let m = ring.len();
        if m < 3 {
            continue;
        }
        for i in 0..m {
            let a = &ring[i];
            let b = &ring[(i + 1) % m];
            let (ay, by) = (map.gz(a.z), map.gz(b.z));
            if ay == by {
                continue;
            }
            let (ax, bx) = (map.gx(a.x), map.gx(b.x));
            let (y_lo, y_hi) = if ay < by { (ay, by) } else { (by, ay) };
            let z_start = y_lo.ceil().max(0.0) as i64;
            let z_end = (y_hi.ceil() as i64).min(gh as i64);
            let mut z = z_start;
            while z < z_end {
                let t = (z as f64 - ay) / (by - ay);
                rows.entry(z as usize).or_default().push(ax + t * (bx - ax));
                z += 1;
            }
        }
    }
    for (z, xs) in rows.iter_mut() {
        xs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        let mut i = 0;
        while i + 1 < xs.len() {
            let x_start = xs[i].ceil().max(0.0) as i64;
            let x_end = (xs[i + 1].floor() as i64).min(gw as i64 - 1);
            let mut x = x_start;
            while x <= x_end {
                set_bit(bits, z * gw + x as usize);
                x += 1;
            }
            i += 2;
        }
    }
}

/// Stamp a polyline of the given half width (in world blocks) into a grid bitset.
fn stamp_line(
    bits: &mut [u64],
    nodes: &[ProcessedNode],
    half_width: i32,
    map: &GridMap,
    xzbbox: &XZBBox,
) {
    let half_width = half_width.clamp(0, MAX_WATERWAY_WIDTH);
    let (w, h) = (map.width as i64, map.height as i64);
    for pair in nodes.windows(2) {
        let (x1, z1, x2, z2) = (pair[0].x, pair[0].z, pair[1].x, pair[1].z);
        if x1.max(x2).saturating_add(half_width) < xzbbox.min_x()
            || x1.min(x2).saturating_sub(half_width) > xzbbox.max_x()
            || z1.max(z2).saturating_add(half_width) < xzbbox.min_z()
            || z1.min(z2).saturating_sub(half_width) > xzbbox.max_z()
        {
            continue;
        }
        for (bx, _, bz) in bresenham_line(x1, 0, z1, x2, 0, z2) {
            for dz in -half_width..=half_width {
                for dx in -half_width..=half_width {
                    let gx = map.gx(bx.saturating_add(dx)).round() as i64;
                    let gz = map.gz(bz.saturating_add(dz)).round() as i64;
                    if gx >= 0 && gz >= 0 && gx < w && gz < h {
                        set_bit(bits, gz as usize * map.width + gx as usize);
                    }
                }
            }
        }
    }
}

fn relation_rings(rel: &ProcessedRelation, xzbbox: &XZBBox) -> Vec<Vec<ProcessedNode>> {
    let mut outer: Vec<Vec<ProcessedNode>> = Vec::new();
    let mut inner: Vec<Vec<ProcessedNode>> = Vec::new();
    for member in &rel.members {
        if member.way.nodes.len() < 2 {
            continue;
        }
        match member.role {
            ProcessedMemberRole::Outer => outer.push(member.way.nodes.clone()),
            ProcessedMemberRole::Inner => inner.push(member.way.nodes.clone()),
            _ => {}
        }
    }
    crate::element_processing::merge_way_segments(&mut outer);
    crate::element_processing::merge_way_segments(&mut inner);
    outer
        .into_iter()
        .chain(inner)
        .filter(|r| is_ring_closed(r))
        .filter_map(|r| clip_water_ring_to_bbox(&r, xzbbox))
        .collect()
}

fn is_ring_closed(nodes: &[ProcessedNode]) -> bool {
    if nodes.len() < 3 {
        return false;
    }
    let (first, last) = (&nodes[0], nodes.last().unwrap());
    first.id == last.id || ((first.x - last.x).abs() <= 1 && (first.z - last.z).abs() <= 1)
}

fn has_explicit_water_tag(tags: &HashMap<String, String>) -> bool {
    tags.get("water")
        .is_some_and(|v| !matches!(v.as_str(), "no" | "0" | "false"))
}

fn is_water_area_way(way: &ProcessedWay) -> bool {
    let tags = &way.tags;
    matches!(tags.get("natural").map(String::as_str), Some("water"))
        || has_explicit_water_tag(tags)
        || matches!(tags.get("landuse").map(String::as_str), Some("reservoir"))
        || matches!(
            tags.get("waterway").map(String::as_str),
            Some("dock" | "riverbank")
        )
}

fn is_water_area_relation(rel: &ProcessedRelation) -> bool {
    matches!(
        rel.tags.get("natural").map(String::as_str),
        Some("water" | "bay")
    ) || has_explicit_water_tag(&rel.tags)
        || matches!(
            rel.tags.get("landuse").map(String::as_str),
            Some("reservoir")
        )
}

fn is_building(tags: &HashMap<String, String>) -> bool {
    tags.get("building")
        .is_some_and(|v| !matches!(v.as_str(), "no" | "0" | "false"))
        || tags.contains_key("building:part")
}

/// Half width of a structure that carries ground out over water, so anything stamped on
/// it proves nothing about the shore.
fn over_water_half_width(way: &ProcessedWay, scale: f64) -> Option<i32> {
    let tags = &way.tags;
    let structure = matches!(
        tags.get("man_made").map(String::as_str),
        Some("pier" | "breakwater" | "groyne" | "dolphin" | "quay")
    ) || is_bridge_way(way)
        || tags.get("floating").map(String::as_str) == Some("yes");
    if !structure {
        return None;
    }
    let road = tags
        .get("highway")
        .map(|kind| highway_block_range(kind, tags, scale));
    Some(road.unwrap_or(2).max(2))
}

/// Half width of a road or rail line that only stands on land, or None for anything else.
fn land_line_half_width(way: &ProcessedWay, scale: f64) -> Option<i32> {
    let tags = &way.tags;
    if tags
        .get("tunnel")
        .is_some_and(|v| !matches!(v.as_str(), "no" | "0" | "false"))
        || tags.get("area").map(String::as_str) == Some("yes")
        || tags.contains_key("man_made")
    {
        return None;
    }
    if let Some(kind) = tags.get("highway") {
        if matches!(kind.as_str(), "proposed" | "construction" | "raceway") {
            return None;
        }
        return Some(highway_block_range(kind, tags, scale).max(0));
    }
    if let Some(kind) = tags.get("railway") {
        if matches!(
            kind.as_str(),
            "rail" | "light_rail" | "tram" | "subway" | "narrow_gauge" | "monorail"
        ) && tags.get("location").map(String::as_str) != Some("underground")
        {
            return Some(1);
        }
    }
    None
}

#[inline(always)]
fn get_bit(mask: &[u64], idx: usize) -> bool {
    (mask[idx >> 6] >> (idx & 63)) & 1 != 0
}

#[inline(always)]
fn set_bit(mask: &mut [u64], idx: usize) {
    mask[idx >> 6] |= 1u64 << (idx & 63);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::land_cover::LC_GRASSLAND;
    use once_cell::sync::OnceCell;

    fn node(id: u64, x: i32, z: i32) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: HashMap::new(),
            x,
            z,
        }
    }

    fn way(id: u64, nodes: Vec<ProcessedNode>, tags: &[(&str, &str)]) -> ProcessedElement {
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    fn grid_from(f: impl Fn(usize, usize) -> u8) -> (LandCoverData, XZBBox) {
        let grid: Vec<Vec<u8>> = (0..60)
            .map(|z| (0..60).map(|x| f(x, z)).collect())
            .collect();
        let water_distance = compute_water_distance(&grid, 60, 60);
        let lc = LandCoverData {
            grid,
            water_distance,
            water_blend_cache: OnceCell::new(),
            width: 60,
            height: 60,
            cells_per_meter: 1.0,
        };
        (lc, XZBBox::rect_from_min_max(0, 0, 59, 59).unwrap())
    }

    /// 60x60 grid over a 60x60 world; water for x >= 30, land west of it.
    fn coast() -> (LandCoverData, XZBBox) {
        grid_from(|x, _| if x >= 30 { LC_WATER } else { LC_GRASSLAND })
    }

    fn run(lc: &mut LandCoverData, bbox: &XZBBox, elements: &[ProcessedElement]) {
        apply_osm_land_override(lc, 60, 60, elements, bbox, 1.0);
    }

    #[test]
    fn road_along_the_shore_reclaims_water_under_it() {
        let (mut lc, bbox) = coast();
        let elements = vec![way(
            1,
            vec![node(1, 32, 0), node(2, 32, 59)],
            &[("highway", "footway")],
        )];
        run(&mut lc, &bbox, &elements);
        assert_eq!(lc.grid[30][32], LC_GRASSLAND);
        assert_eq!(lc.grid[30][50], LC_WATER);
    }

    #[test]
    fn water_area_boundary_trims_the_esa_rim() {
        let (mut lc, bbox) = coast();
        let ring = vec![
            node(1, 34, -5),
            node(2, 80, -5),
            node(3, 80, 65),
            node(4, 34, 65),
            node(1, 34, -5),
        ];
        let elements = vec![way(2, ring, &[("natural", "water")])];
        run(&mut lc, &bbox, &elements);
        for x in 30..34 {
            assert_ne!(lc.grid[30][x], LC_WATER, "x={x} should be land");
        }
        for x in 34..60 {
            assert_eq!(lc.grid[30][x], LC_WATER, "x={x} should stay water");
        }
    }

    #[test]
    fn a_waterway_centreline_never_defines_the_shore() {
        // ESA sees a 20 cell wide river, OSM maps only its centreline. The banks must not
        // close in to the tagged channel width.
        let (mut lc, bbox) = grid_from(|x, _| {
            if (20..40).contains(&x) {
                LC_WATER
            } else {
                LC_GRASSLAND
            }
        });
        let elements = vec![way(
            3,
            vec![node(1, 30, 0), node(2, 30, 59)],
            &[("waterway", "stream")],
        )];
        run(&mut lc, &bbox, &elements);
        for x in 20..40 {
            assert_eq!(lc.grid[30][x], LC_WATER, "x={x} should stay water");
        }
    }

    #[test]
    fn a_mapped_pond_does_not_condemn_water_across_land() {
        // Two bodies separated by land. Only the mapped one may lose its rim.
        let (mut lc, bbox) = grid_from(|x, _| {
            if (5..15).contains(&x) || (40..55).contains(&x) {
                LC_WATER
            } else {
                LC_GRASSLAND
            }
        });
        let ring = vec![
            node(1, 7, 5),
            node(2, 13, 5),
            node(3, 13, 55),
            node(4, 7, 55),
            node(1, 7, 5),
        ];
        let elements = vec![way(4, ring, &[("natural", "water")])];
        run(&mut lc, &bbox, &elements);
        for x in 40..55 {
            assert_eq!(
                lc.grid[30][x], LC_WATER,
                "unmapped body at x={x} was trimmed"
            );
        }
    }

    #[test]
    fn piers_and_bridges_are_not_land() {
        let (mut lc, bbox) = coast();
        let elements = vec![
            way(
                5,
                vec![node(1, 32, 10), node(2, 32, 20)],
                &[("highway", "footway"), ("bridge", "yes")],
            ),
            way(
                6,
                vec![node(3, 34, 30), node(4, 34, 40)],
                &[("man_made", "pier")],
            ),
            // The footway drawn on top of that pier shares its geometry.
            way(
                7,
                vec![node(5, 34, 30), node(6, 34, 40)],
                &[("highway", "footway")],
            ),
        ];
        run(&mut lc, &bbox, &elements);
        assert_eq!(lc.grid[15][32], LC_WATER, "bridge deck became land");
        assert_eq!(lc.grid[35][34], LC_WATER, "pier became land");
    }

    #[test]
    fn open_water_far_from_shore_is_never_trimmed() {
        let (mut lc, bbox) = coast();
        let ring = vec![
            node(1, 0, 0),
            node(2, 59, 0),
            node(3, 59, 59),
            node(4, 0, 59),
            node(1, 0, 0),
        ];
        let elements = vec![way(8, ring, &[("building", "yes")])];
        run(&mut lc, &bbox, &elements);
        assert_ne!(lc.grid[30][31], LC_WATER);
        assert_eq!(lc.grid[30][50], LC_WATER);
    }

    #[test]
    fn an_absurd_width_tag_is_clamped() {
        let (mut lc, bbox) = coast();
        let elements = vec![way(
            9,
            vec![node(1, 32, 0), node(2, 32, 59)],
            &[("waterway", "river"), ("width", "999999999")],
        )];
        run(&mut lc, &bbox, &elements);
        assert_eq!(lc.grid[30][50], LC_WATER);
    }
}
