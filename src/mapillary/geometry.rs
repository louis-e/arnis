//! Footprints, the run frame and walls. Port of `tools/facade_lab/geo.py`.
//!
//! Overpass JSON goes in and [`Wall`] segments in the run frame come out: rings
//! oriented counter-clockwise, consecutive edges merged while they stay within
//! `merge_deg` of each other, walls under `min_wall_m` dropped, walls over
//! `split_wall_m` cut into equal pieces, and the OSM node ids of the merged span
//! kept on every piece. Node ids are how a wall is matched back to the world the
//! generator builds, so they survive every step here.
//!
//! This module also hosts the WGS84 geodesy that `sfm.rs` needs to bring an
//! OpenSfM cluster into lon/lat before it enters the run frame. That path
//! deliberately does not use the sphere formula of [`Frame`]: OpenSfM's
//! topocentric frame is ellipsoidal, and mixing the two costs 0.25 to 0.8 m.
//!
//! One deliberate difference from the Python. `geo._make_building` repairs a
//! self-intersecting ring with shapely's `buffer(0)` and then guesses the node
//! ids back by position, marking the vertices it cannot place as -1. A wall
//! whose nodes are -1 cannot be matched to anything the generator builds, so the
//! repair buys nothing downstream; here such a ring is dropped instead. It never
//! fires on the Munich box: all 170 building ways there are simple.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};

use geo::{Contains, Intersects};
use serde_json::Value;

use super::types::{
    building_key, wall_key, BBox, Building, Frame, HeightSource, OsmKind, Params, Wall, WallEdge,
};

// --------------------------------------------------------------------------- geodesy

const WGS84_A: f64 = 6_378_137.0;
const WGS84_E2: f64 = 6.694_379_990_14e-3;

/// Geodetic lon/lat in degrees plus ellipsoidal altitude to ECEF metres.
pub fn lla_to_ecef(lon: f64, lat: f64, alt: f64) -> [f64; 3] {
    let (lon, lat) = (lon.to_radians(), lat.to_radians());
    let (sl, cl) = lat.sin_cos();
    let n = WGS84_A / (1.0 - WGS84_E2 * sl * sl).sqrt();
    [
        (n + alt) * cl * lon.cos(),
        (n + alt) * cl * lon.sin(),
        (n * (1.0 - WGS84_E2) + alt) * sl,
    ]
}

/// ECEF metres to lon/lat degrees plus altitude. Six iterations put the error
/// under a nanometre, which is what the Python does.
pub fn ecef_to_lla(p: [f64; 3]) -> [f64; 3] {
    let (x, y, z) = (p[0], p[1], p[2]);
    let lon = y.atan2(x);
    let hyp = (x * x + y * y).sqrt();
    let mut lat = z.atan2(hyp * (1.0 - WGS84_E2));
    let mut alt = 0.0;
    for _ in 0..6 {
        let sl = lat.sin();
        let n = WGS84_A / (1.0 - WGS84_E2 * sl * sl).sqrt();
        alt = hyp / lat.cos() - n;
        lat = z.atan2(hyp * (1.0 - WGS84_E2 * n / (n + alt)));
    }
    [lon.to_degrees(), lat.to_degrees(), alt]
}

/// Rows are the east, north and up unit vectors in ECEF at `(lon0, lat0)`.
fn enu_rows(lon0: f64, lat0: f64) -> [[f64; 3]; 3] {
    let (lo, la) = (lon0.to_radians(), lat0.to_radians());
    let (slo, clo) = lo.sin_cos();
    let (sla, cla) = la.sin_cos();
    [
        [-slo, clo, 0.0],
        [-sla * clo, -sla * slo, cla],
        [cla * clo, cla * slo, sla],
    ]
}

/// OpenSfM topocentric metres about `ref_lla = (lon, lat, alt)` to lon/lat/alt.
///
/// The exact inverse of OpenSfM's `topocentric_from_lla`.
pub fn topocentric_to_lla(p: [f64; 3], ref_lla: (f64, f64, f64)) -> [f64; 3] {
    let (lon0, lat0, alt0) = ref_lla;
    let e0 = lla_to_ecef(lon0, lat0, alt0);
    let rows = enu_rows(lon0, lat0);
    let mut ecef = e0;
    for i in 0..3 {
        for j in 0..3 {
            ecef[j] += p[i] * rows[i][j];
        }
    }
    ecef_to_lla(ecef)
}

/// Inverse of [`topocentric_to_lla`].
pub fn lla_to_topocentric(lon: f64, lat: f64, alt: f64, ref_lla: (f64, f64, f64)) -> [f64; 3] {
    let (lon0, lat0, alt0) = ref_lla;
    let e0 = lla_to_ecef(lon0, lat0, alt0);
    let e = lla_to_ecef(lon, lat, alt);
    let d = [e[0] - e0[0], e[1] - e0[1], e[2] - e0[2]];
    let rows = enu_rows(lon0, lat0);
    [
        rows[0][0] * d[0] + rows[0][1] * d[1] + rows[0][2] * d[2],
        rows[1][0] * d[0] + rows[1][1] * d[1] + rows[1][2] * d[2],
        rows[2][0] * d[0] + rows[2][1] * d[1] + rows[2][2] * d[2],
    ]
}

/// Cluster points and shot centres into the run frame: xy through lon/lat, z
/// kept as the cluster's own topocentric up because that datum is per cluster.
pub fn topocentric_to_run(p: [f64; 3], ref_lla: (f64, f64, f64), frame: &Frame) -> [f64; 3] {
    let lla = topocentric_to_lla(p, ref_lla);
    let xy = frame.to_enu(lla[0], lla[1]);
    [xy[0], xy[1], p[2]]
}

// --------------------------------------------------------------------------- frame

/// The run frame at the centre of the bbox.
pub fn build_frame(bbox: BBox) -> Frame {
    let (lon0, lat0) = bbox.centre();
    Frame::new(lon0, lat0)
}

/// The bbox as an axis-aligned rectangle in the run frame, optionally grown.
pub fn bbox_polygon_xy(bbox: BBox, frame: &Frame, margin_m: f64) -> [[f64; 2]; 4] {
    let lo = frame.to_enu(bbox.min_lon, bbox.min_lat);
    let hi = frame.to_enu(bbox.max_lon, bbox.max_lat);
    let (x0, y0) = (lo[0] - margin_m, lo[1] - margin_m);
    let (x1, y1) = (hi[0] + margin_m, hi[1] + margin_m);
    [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

// --------------------------------------------------------------------------- heights

/// A length tag as metres. Accepts `12`, `12 m`, `12.5`, `12,5` and `40 ft`.
pub fn parse_length_m(text: &str) -> Option<f64> {
    let t = text.trim();
    let split = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ',' || c == '+' || c == '-'))
        .unwrap_or(t.len());
    let (num, rest) = t.split_at(split);
    // The Python regex allows one sign, digits, and at most one decimal group.
    let num = num.replace(',', ".");
    if num.is_empty() || num.matches('.').count() > 1 {
        return None;
    }
    let value: f64 = num.parse().ok()?;
    let unit = rest.trim();
    if !unit.chars().all(|c| c.is_ascii_alphabetic() || c == '\'') {
        return None;
    }
    match unit.to_ascii_lowercase().as_str() {
        "" | "m" | "meter" | "meters" | "metre" | "metres" => Some(value),
        "ft" | "feet" | "foot" | "'" => Some(value * 0.3048),
        _ => None,
    }
}

/// `(height, source, min_height)` from the tags.
///
/// `height` first, then `building:levels * metres_per_level` with `roof:levels`
/// added on top. `None` means the caller applies the 9 m default and records the
/// source as `default`.
pub fn height_from_tags(
    tags: &BTreeMap<String, String>,
    params: &Params,
) -> (Option<f64>, HeightSource, f64) {
    let mpl = params.metres_per_level;
    let mut height = None;
    let mut source = HeightSource::Default;
    if let Some(h) = tags.get("height").and_then(|s| parse_length_m(s)) {
        if h > 1.0 && h < 400.0 {
            height = Some(h);
            source = HeightSource::Tag;
        }
    }
    if height.is_none() {
        if let Some(levels) = tags
            .get("building:levels")
            .and_then(|s| s.replace(',', ".").parse::<f64>().ok())
        {
            if (1.0..150.0).contains(&levels) {
                let roof = tags
                    .get("roof:levels")
                    .and_then(|s| s.replace(',', ".").parse::<f64>().ok())
                    .map(|v| v.max(0.0))
                    .unwrap_or(0.0);
                height = Some((levels + roof) * mpl);
                source = HeightSource::Levels;
            }
        }
    }
    let mut min_h = 0.0;
    if let Some(v) = tags.get("min_height").and_then(|s| parse_length_m(s)) {
        if v >= 0.0 {
            min_h = v;
        }
    } else if let Some(v) = tags.get("building:min_level") {
        min_h = v.parse::<f64>().map(|l| (l * mpl).max(0.0)).unwrap_or(0.0);
    }
    (height, source, min_h)
}

// --------------------------------------------------------------------------- rings

/// Shoelace area; positive means counter-clockwise in ENU.
pub fn signed_area(ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    let mut acc = 0.0;
    for i in 0..n {
        let p = ring[i];
        let q = ring[(i + 1) % n];
        acc += p[0] * q[1] - q[0] * p[1];
    }
    0.5 * acc
}

/// Reverses a clockwise ring in place, keeping the first node first.
pub fn ensure_ccw(ring: &mut [[f64; 2]], node_ids: &mut [i64]) {
    if signed_area(ring) >= 0.0 {
        return;
    }
    ring[1..].reverse();
    if node_ids.len() == ring.len() {
        node_ids[1..].reverse();
    }
}

/// Outward normal of an edge with unit tangent `t`.
///
/// For a counter-clockwise ring in ENU that is `n = (t_y, -t_x)`. The mirrored
/// rule in `facade.rs` belongs to the Arnis world frame, whose z points south.
pub fn outward_normal(t: [f64; 2], ccw: bool) -> [f64; 2] {
    if ccw {
        [t[1], -t[0]]
    } else {
        [-t[1], t[0]]
    }
}

fn dist2(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    dx * dx + dy * dy
}

/// Drops the closing node and consecutive duplicates.
fn dedupe_ring(ring: &[[f64; 2]], node_ids: &[i64]) -> (Vec<[f64; 2]>, Vec<i64>) {
    const TOL2: f64 = 1e-6 * 1e-6;
    let mut n = ring.len();
    if n > 1 && dist2(ring[0], ring[n - 1]) < TOL2 {
        n -= 1;
    }
    let mut keep: Vec<usize> = vec![0];
    for i in 1..n {
        if dist2(ring[i], ring[*keep.last().unwrap()]) >= TOL2 {
            keep.push(i);
        }
    }
    if keep.len() > 1 && dist2(ring[keep[keep.len() - 1]], ring[keep[0]]) < TOL2 {
        keep.pop();
    }
    (
        keep.iter().map(|&i| ring[i]).collect(),
        keep.iter().map(|&i| node_ids[i]).collect(),
    )
}

/// True when no two non-adjacent edges of the ring cross.
///
/// This stands in for shapely's validity test; see the module header for why the
/// repair path is not ported.
fn ring_is_simple(ring: &[[f64; 2]]) -> bool {
    let n = ring.len();
    if n < 3 {
        return false;
    }
    for i in 0..n {
        let (a1, a2) = (ring[i], ring[(i + 1) % n]);
        for j in (i + 1)..n {
            // Adjacent edges share an endpoint and always "touch".
            if j == i || (j + 1) % n == i || (i + 1) % n == j {
                continue;
            }
            let (b1, b2) = (ring[j], ring[(j + 1) % n]);
            if segments_cross(a1, a2, b1, b2) {
                return false;
            }
        }
    }
    true
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Proper or touching intersection of two closed segments.
fn segments_cross(a1: [f64; 2], a2: [f64; 2], b1: [f64; 2], b2: [f64; 2]) -> bool {
    let d1 = orient(b1, b2, a1);
    let d2 = orient(b1, b2, a2);
    let d3 = orient(a1, a2, b1);
    let d4 = orient(a1, a2, b2);
    if ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) && d1 != 0.0 && d2 != 0.0 {
        return true;
    }
    let on = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        orient(p, q, r) == 0.0
            && r[0] >= p[0].min(q[0])
            && r[0] <= p[0].max(q[0])
            && r[1] >= p[1].min(q[1])
            && r[1] <= p[1].max(q[1])
    };
    on(b1, b2, a1) || on(b1, b2, a2) || on(a1, a2, b1) || on(a1, a2, b2)
}

/// Joins open way node lists into closed rings by shared end node ids.
fn join_ways(way_nodes: &[Vec<i64>]) -> Vec<Vec<i64>> {
    let mut pieces: Vec<Vec<i64>> = way_nodes.iter().filter(|w| w.len() >= 2).cloned().collect();
    let mut rings = Vec::new();
    while !pieces.is_empty() {
        let mut cur = pieces.remove(0);
        let mut changed = true;
        while cur[0] != cur[cur.len() - 1] && changed {
            changed = false;
            for i in 0..pieces.len() {
                let p = &pieces[i];
                let (last, first) = (*cur.last().unwrap(), cur[0]);
                if p[0] == last {
                    cur.extend_from_slice(&p[1..]);
                } else if *p.last().unwrap() == last {
                    let mut rev: Vec<i64> = p.iter().rev().skip(1).copied().collect();
                    cur.append(&mut rev);
                } else if *p.last().unwrap() == first {
                    let mut head = p[..p.len() - 1].to_vec();
                    head.extend_from_slice(&cur);
                    cur = head;
                } else if p[0] == first {
                    let mut head: Vec<i64> = p.iter().rev().copied().collect();
                    head.pop();
                    head.extend_from_slice(&cur);
                    cur = head;
                } else {
                    continue;
                }
                pieces.remove(i);
                changed = true;
                break;
            }
        }
        if cur[0] == *cur.last().unwrap() && cur.len() >= 4 {
            rings.push(cur);
        }
    }
    rings
}

fn ring_xy(
    node_ids: &[i64],
    nodes: &HashMap<i64, (f64, f64)>,
    frame: &Frame,
) -> Option<Vec<[f64; 2]>> {
    node_ids
        .iter()
        .map(|id| nodes.get(id).map(|&(lon, lat)| frame.to_enu(lon, lat)))
        .collect()
}

fn to_geo_polygon(ring: &[[f64; 2]]) -> geo::Polygon<f64> {
    geo::Polygon::new(
        geo::LineString::from(ring.iter().map(|p| (p[0], p[1])).collect::<Vec<_>>()),
        vec![],
    )
}

/// Area weighted centroid of a ring, the stand-in for shapely's
/// `representative_point` when deciding which outer ring owns a hole. Any point
/// of the hole answers that question, and a hole always lies inside its outer.
fn ring_centroid(ring: &[[f64; 2]]) -> [f64; 2] {
    let n = ring.len();
    let mut a = 0.0;
    let mut cx = 0.0;
    let mut cy = 0.0;
    for i in 0..n {
        let p = ring[i];
        let q = ring[(i + 1) % n];
        let cross = p[0] * q[1] - q[0] * p[1];
        a += cross;
        cx += (p[0] + q[0]) * cross;
        cy += (p[1] + q[1]) * cross;
    }
    if a.abs() < 1e-12 {
        return ring[0];
    }
    [cx / (3.0 * a), cy / (3.0 * a)]
}

#[allow(clippy::too_many_arguments)]
fn make_building(
    key: String,
    osm_id: i64,
    kind: OsmKind,
    ring: &[[f64; 2]],
    node_ids: &[i64],
    holes: Vec<Vec<[f64; 2]>>,
    tags: BTreeMap<String, String>,
    member_ways: Vec<i64>,
    params: &Params,
    target_poly: Option<&geo::Polygon<f64>>,
) -> Option<Building> {
    let (mut ring, mut node_ids) = dedupe_ring(ring, node_ids);
    if ring.len() < 3 || !ring_is_simple(&ring) {
        return None;
    }
    if signed_area(&ring).abs() < params.min_ring_area_m2 {
        return None;
    }
    ensure_ccw(&mut ring, &mut node_ids);
    let holes: Vec<Vec<[f64; 2]>> = holes
        .into_iter()
        .filter(|h| h.len() >= 3 && signed_area(h).abs() > 0.5)
        .collect();
    let (height_osm, height_source, min_height) = height_from_tags(&tags, params);
    let target = match target_poly {
        Some(t) => to_geo_polygon(&ring).intersects(t),
        None => true,
    };
    Some(Building {
        key,
        osm_id,
        kind,
        ring,
        holes,
        node_ids,
        tags,
        height_osm,
        height_source,
        min_height,
        target,
        member_ways,
    })
}

fn tags_of(el: &Value) -> BTreeMap<String, String> {
    el.get("tags")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn node_list(el: &Value) -> Vec<i64> {
    el.get("nodes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

/// Overpass `out body; >; out skel qt;` JSON to buildings in the run frame.
///
/// Ways tagged `building=*` become one exterior ring each. Relations tagged
/// `building=*` have their outer member ways joined by shared node ids into one
/// building per outer ring (`r<id>`, then `r<id>_1` and so on), with the inner
/// rings as holes; member ways that carry `building=*` themselves are skipped,
/// because the relation owns that footprint. `building:part` is ignored.
/// `target_bbox` marks the buildings that touch the unpadded box; the rest are
/// only ever occluders.
pub fn parse_overpass(
    osm: &Value,
    frame: &Frame,
    params: &Params,
    target_bbox: Option<BBox>,
) -> Vec<Building> {
    let empty = Vec::new();
    let elements = osm
        .get("elements")
        .and_then(Value::as_array)
        .or_else(|| osm.as_array())
        .unwrap_or(&empty);

    let mut nodes: HashMap<i64, (f64, f64)> = HashMap::new();
    let mut ways: HashMap<i64, &Value> = HashMap::new();
    let mut way_order: Vec<i64> = Vec::new();
    let mut relations: Vec<&Value> = Vec::new();
    for el in elements {
        match el.get("type").and_then(Value::as_str) {
            Some("node") => {
                if let (Some(id), Some(lon), Some(lat)) = (
                    el.get("id").and_then(Value::as_i64),
                    el.get("lon").and_then(Value::as_f64),
                    el.get("lat").and_then(Value::as_f64),
                ) {
                    nodes.insert(id, (lon, lat));
                }
            }
            Some("way") => {
                if let Some(id) = el.get("id").and_then(Value::as_i64) {
                    if ways.insert(id, el).is_none() {
                        way_order.push(id);
                    }
                }
            }
            Some("relation") => relations.push(el),
            _ => {}
        }
    }

    let target_poly = target_bbox.map(|b| {
        let rect = bbox_polygon_xy(b, frame, 0.0);
        to_geo_polygon(&rect)
    });

    let mut member_ways_of_relations: HashSet<i64> = HashSet::new();
    let mut buildings: Vec<Building> = Vec::new();

    for rel in &relations {
        let tags = tags_of(rel);
        if !tags.contains_key("building") {
            continue;
        }
        let rel_id = rel.get("id").and_then(Value::as_i64).unwrap_or(0);
        let mut outers: Vec<Vec<i64>> = Vec::new();
        let mut inners: Vec<Vec<i64>> = Vec::new();
        let mut own_members: Vec<i64> = Vec::new();
        for m in rel
            .get("members")
            .and_then(Value::as_array)
            .unwrap_or(&empty)
        {
            if m.get("type").and_then(Value::as_str) != Some("way") {
                continue;
            }
            let Some(reference) = m.get("ref").and_then(Value::as_i64) else {
                continue;
            };
            let Some(way) = ways.get(&reference) else {
                continue;
            };
            let role = m.get("role").and_then(Value::as_str).unwrap_or("outer");
            if role == "outer" || role.is_empty() {
                outers.push(node_list(way));
            } else {
                inners.push(node_list(way));
            }
            if !own_members.contains(&reference) {
                own_members.push(reference);
            }
        }
        own_members.sort_unstable();
        member_ways_of_relations.extend(own_members.iter().copied());

        let outer_rings = join_ways(&outers);
        let inner_xy: Vec<Vec<[f64; 2]>> = join_ways(&inners)
            .iter()
            .filter_map(|ids| ring_xy(ids, &nodes, frame))
            .collect();
        for (k, ids) in outer_rings.iter().enumerate() {
            let Some(xy) = ring_xy(ids, &nodes, frame) else {
                continue;
            };
            let outer = to_geo_polygon(&xy);
            let holes: Vec<Vec<[f64; 2]>> = inner_xy
                .iter()
                .filter(|h| {
                    let c = ring_centroid(h);
                    outer.contains(&geo::Point::new(c[0], c[1]))
                })
                .cloned()
                .collect();
            let mut key = building_key(OsmKind::Relation, rel_id);
            if k > 0 {
                key = format!("{key}_{k}");
            }
            if let Some(b) = make_building(
                key,
                rel_id,
                OsmKind::Relation,
                &xy,
                ids,
                holes,
                tags.clone(),
                own_members.clone(),
                params,
                target_poly.as_ref(),
            ) {
                buildings.push(b);
            }
        }
    }

    for wid in &way_order {
        let way = ways[wid];
        let tags = tags_of(way);
        if !tags.contains_key("building") {
            continue;
        }
        // The relation owns this footprint.
        if member_ways_of_relations.contains(wid) {
            continue;
        }
        let ids = node_list(way);
        if ids.len() < 4 || ids[0] != ids[ids.len() - 1] {
            continue;
        }
        let Some(xy) = ring_xy(&ids, &nodes, frame) else {
            continue;
        };
        if let Some(b) = make_building(
            building_key(OsmKind::Way, *wid),
            *wid,
            OsmKind::Way,
            &xy,
            &ids,
            Vec::new(),
            tags,
            Vec::new(),
            params,
            target_poly.as_ref(),
        ) {
            buildings.push(b);
        }
    }

    buildings.sort_by(|a, b| {
        (a.kind != OsmKind::Way, a.osm_id).cmp(&(b.kind != OsmKind::Way, b.osm_id))
    });
    buildings
}

// --------------------------------------------------------------------------- walls

/// Groups of consecutive edge indices whose directions differ by at most
/// `merge_deg`.
///
/// The scan starts at an edge whose predecessor is not collinear so a group
/// never wraps past the start, and an edge joins a group only if it is within
/// tolerance of both the previous edge and the group's first edge, which stops a
/// long curve from drifting into one straight wall.
fn merge_edges(ring: &[[f64; 2]], merge_deg: f64) -> Vec<Vec<usize>> {
    let n = ring.len();
    let dirs: Vec<[f64; 2]> = (0..n)
        .map(|i| {
            let p = ring[i];
            let q = ring[(i + 1) % n];
            let d = [q[0] - p[0], q[1] - p[1]];
            let len = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-12);
            [d[0] / len, d[1] / len]
        })
        .collect();
    let dot = |a: [f64; 2], b: [f64; 2]| a[0] * b[0] + a[1] * b[1];
    let thr = merge_deg.to_radians().cos();
    let mut start = 0usize;
    for i in 0..n {
        if dot(dirs[i], dirs[(i + n - 1) % n]) < thr {
            start = i;
            break;
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut cur = vec![start];
    for k in 1..n {
        let i = (start + k) % n;
        if dot(dirs[i], dirs[*cur.last().unwrap()]) >= thr && dot(dirs[i], dirs[cur[0]]) >= thr {
            cur.push(i);
        } else {
            groups.push(std::mem::take(&mut cur));
            cur = vec![i];
        }
    }
    groups.push(cur);
    groups
}

/// Walls of one footprint: merged, short ones dropped, long ones split.
///
/// A wall over `split_wall_m` is cut into equal pieces rather than into a run of
/// full-length pieces plus a stub, so a 45 m wall becomes two 22.5 m halves and
/// not 30 m plus 15 m. The wall index counts kept walls only, which is what puts
/// the same key on the same wall as the Python.
pub fn walls_from_building(b: &Building, params: &Params) -> Vec<Wall> {
    let ring = &b.ring;
    let n = ring.len();
    if n < 3 {
        return Vec::new();
    }
    let node_ids: Vec<i64> = if b.node_ids.len() == n {
        b.node_ids.clone()
    } else {
        vec![-1; n]
    };
    let ccw = signed_area(ring) > 0.0;
    let mut walls = Vec::new();
    let mut idx = 0usize;
    for group in merge_edges(ring, params.merge_deg) {
        let i0 = group[0];
        let i1 = (group[group.len() - 1] + 1) % n;
        let a = ring[i0];
        let bpt = ring[i1];
        let len = ((bpt[0] - a[0]).powi(2) + (bpt[1] - a[1]).powi(2)).sqrt();
        if len < params.min_wall_m {
            continue;
        }
        let t = [(bpt[0] - a[0]) / len, (bpt[1] - a[1]) / len];
        let nrm = outward_normal(t, ccw);
        let edges: Vec<WallEdge> = group
            .iter()
            .map(|&e| {
                let p0 = ring[e];
                let p1 = ring[(e + 1) % n];
                WallEdge {
                    edge_idx: e,
                    node_a: node_ids[e],
                    node_b: node_ids[(e + 1) % n],
                    s0: (p0[0] - a[0]) * t[0] + (p0[1] - a[1]) * t[1],
                    s1: (p1[0] - a[0]) * t[0] + (p1[1] - a[1]) * t[1],
                }
            })
            .collect();
        let n_pieces = if len > params.split_wall_m {
            (len / params.split_wall_m).ceil() as usize
        } else {
            1
        };
        let piece_len = len / n_pieces as f64;
        for k in 0..n_pieces {
            let s0 = k as f64 * piece_len;
            let s1 = (k + 1) as f64 * piece_len;
            walls.push(Wall {
                key: wall_key(&b.key, idx, k, n_pieces),
                building_key: b.key.clone(),
                idx,
                node_a: node_ids[i0],
                node_b: node_ids[i1],
                a: [a[0] + s0 * t[0], a[1] + s0 * t[1]],
                b: [a[0] + s1 * t[0], a[1] + s1 * t[1]],
                n: nrm,
                length: s1 - s0,
                merged_idx: group.clone(),
                piece: k,
                n_pieces,
                height_osm: b.height_osm,
                height_source: b.height_source,
                reachable: true,
                unreachable_reason: String::new(),
                edges: edges
                    .iter()
                    .map(|e| WallEdge {
                        s0: e.s0 - s0,
                        s1: e.s1 - s0,
                        ..*e
                    })
                    .collect(),
                s_offset: s0,
            });
        }
        idx += 1;
    }
    walls
}

/// Every wall of every building, in building order.
pub fn walls_from_buildings(buildings: &[Building], params: &Params) -> Vec<Wall> {
    buildings
        .iter()
        .flat_map(|b| walls_from_building(b, params))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Params {
        Params::default()
    }

    #[test]
    fn outward_normal_points_out_of_a_ccw_square() {
        // A CCW unit square: the bottom edge runs east, so its outward normal
        // points south.
        let ring = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!(signed_area(&ring) > 0.0);
        let n = outward_normal([1.0, 0.0], true);
        assert!((n[0] - 0.0).abs() < 1e-12 && (n[1] + 1.0).abs() < 1e-12);
        let n = outward_normal([0.0, 1.0], true);
        assert!((n[0] - 1.0).abs() < 1e-12 && (n[1] - 0.0).abs() < 1e-12);
    }

    #[test]
    fn ensure_ccw_keeps_the_first_node() {
        let mut ring = vec![[0.0, 0.0], [0.0, 10.0], [10.0, 10.0], [10.0, 0.0]];
        let mut ids = vec![1, 2, 3, 4];
        assert!(signed_area(&ring) < 0.0);
        ensure_ccw(&mut ring, &mut ids);
        assert!(signed_area(&ring) > 0.0);
        assert_eq!(ids, vec![1, 4, 3, 2]);
        assert_eq!(ring[0], [0.0, 0.0]);
    }

    #[test]
    fn lengths_parse_the_way_osm_writes_them() {
        assert_eq!(parse_length_m("12"), Some(12.0));
        assert_eq!(parse_length_m(" 12 m "), Some(12.0));
        assert_eq!(parse_length_m("12,5"), Some(12.5));
        assert_eq!(parse_length_m("12.5 metres"), Some(12.5));
        assert!((parse_length_m("40 ft").unwrap() - 12.192).abs() < 1e-9);
        assert_eq!(parse_length_m("about 12"), None);
        assert_eq!(parse_length_m("12 storeys"), None);
    }

    #[test]
    fn heights_prefer_the_tag_then_the_levels() {
        let p = params();
        let mut tags = BTreeMap::new();
        tags.insert("height".into(), "18".into());
        tags.insert("building:levels".into(), "4".into());
        let (h, src, _) = height_from_tags(&tags, &p);
        assert_eq!((h, src), (Some(18.0), HeightSource::Tag));

        let mut tags = BTreeMap::new();
        tags.insert("building:levels".into(), "5".into());
        tags.insert("roof:levels".into(), "1".into());
        let (h, src, _) = height_from_tags(&tags, &p);
        assert_eq!((h, src), (Some(18.0), HeightSource::Levels));

        // Out of range values fall through to the default.
        let mut tags = BTreeMap::new();
        tags.insert("height".into(), "0.5".into());
        let (h, src, _) = height_from_tags(&tags, &p);
        assert_eq!((h, src), (None, HeightSource::Default));

        let mut tags = BTreeMap::new();
        tags.insert("building:min_level".into(), "2".into());
        let (_, _, min_h) = height_from_tags(&tags, &p);
        assert!((min_h - 6.0).abs() < 1e-12);
    }

    #[test]
    fn merge_joins_collinear_edges_and_splitting_is_even() {
        // A 45 m south edge broken into three collinear 15 m pieces plus a
        // 20 m return, all within merge_deg of each other where collinear.
        let b = Building {
            key: "w1".into(),
            osm_id: 1,
            kind: OsmKind::Way,
            ring: vec![
                [0.0, 0.0],
                [15.0, 0.0],
                [30.0, 0.0],
                [45.0, 0.0],
                [45.0, 20.0],
                [0.0, 20.0],
            ],
            holes: vec![],
            node_ids: vec![1, 2, 3, 4, 5, 6],
            tags: BTreeMap::new(),
            height_osm: Some(12.0),
            height_source: HeightSource::Tag,
            min_height: 0.0,
            target: true,
            member_ways: vec![],
        };
        let walls = walls_from_building(&b, &params());
        // The 45 m run merges into one wall and then splits into two pieces.
        let merged: Vec<&Wall> = walls.iter().filter(|w| w.merged_idx.len() == 3).collect();
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].key, "w1_0p0");
        assert_eq!(merged[1].key, "w1_0p1");
        assert!((merged[0].length - 22.5).abs() < 1e-9);
        assert!((merged[1].length - 22.5).abs() < 1e-9);
        // Node ids of the whole merged span, not of the piece.
        assert_eq!((merged[0].node_a, merged[0].node_b), (1, 4));
        assert_eq!((merged[1].node_a, merged[1].node_b), (1, 4));
        // Three original edges are carried, with the piece's own s interval.
        assert_eq!(merged[1].edges.len(), 3);
        assert!((merged[1].edges[0].s0 + 22.5).abs() < 1e-9);
        assert!((merged[1].s_offset - 22.5).abs() < 1e-9);
        // Six walls: the 45 m south run and the 45 m north side are two pieces
        // each, plus the two 20 m ends.
        assert_eq!(walls.len(), 6);
    }

    #[test]
    fn short_walls_are_dropped_without_spending_an_index() {
        let b = Building {
            key: "w1".into(),
            osm_id: 1,
            kind: OsmKind::Way,
            // A 10 x 10 square with a 1 m notch cut out of one corner.
            ring: vec![
                [0.0, 0.0],
                [10.0, 0.0],
                [10.0, 9.0],
                [9.0, 9.0],
                [9.0, 10.0],
                [0.0, 10.0],
            ],
            holes: vec![],
            node_ids: vec![1, 2, 3, 4, 5, 6],
            tags: BTreeMap::new(),
            height_osm: None,
            height_source: HeightSource::Default,
            min_height: 0.0,
            target: true,
            member_ways: vec![],
        };
        let walls = walls_from_building(&b, &params());
        assert!(walls.iter().all(|w| w.length >= 3.0));
        // The two 1 m notch edges are gone and the indices of what is left run
        // 0, 1, 2, 3 with no hole where they were.
        let idx: Vec<usize> = walls.iter().map(|w| w.idx).collect();
        assert_eq!(idx, (0..walls.len()).collect::<Vec<_>>());
    }

    #[test]
    fn ecef_round_trips() {
        for (lon, lat, alt) in [(11.58, 48.136, 520.0), (-74.0, 40.7, 10.0), (0.0, 0.0, 0.0)] {
            let back = ecef_to_lla(lla_to_ecef(lon, lat, alt));
            assert!((back[0] - lon).abs() < 1e-9, "lon {} vs {}", back[0], lon);
            assert!((back[1] - lat).abs() < 1e-9, "lat {} vs {}", back[1], lat);
            assert!((back[2] - alt).abs() < 1e-6, "alt {} vs {}", back[2], alt);
        }
    }

    #[test]
    fn topocentric_round_trips() {
        let reference = (11.5795305, 48.13643, 520.0);
        let p = [12.5, -30.25, 3.75];
        let lla = topocentric_to_lla(p, reference);
        let back = lla_to_topocentric(lla[0], lla[1], lla[2], reference);
        for i in 0..3 {
            assert!(
                (back[i] - p[i]).abs() < 1e-6,
                "axis {i}: {} vs {}",
                back[i],
                p[i]
            );
        }
    }

    #[test]
    fn self_intersecting_rings_are_dropped() {
        let bowtie = [[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]];
        assert!(!ring_is_simple(&bowtie));
        let square = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!(ring_is_simple(&square));
    }

    // ----------------------------------------------------------------- golden

    use crate::mapillary::golden;

    /// Buildings and walls of the Munich box, straight from the Overpass answer
    /// the reference run consumed.
    fn munich() -> (Vec<Building>, Vec<Wall>) {
        let gf: golden::GoldenFrame = golden::load("frame.json");
        let frame = build_frame(gf.bbox());
        assert!((frame.lon0 - gf.lon0).abs() < 1e-12);
        assert!((frame.lat0 - gf.lat0).abs() < 1e-12);
        let osm = golden::load_value("osm.json");
        let p = params();
        let buildings = parse_overpass(&osm, &frame, &p, Some(gf.bbox()));
        let walls = walls_from_buildings(&buildings, &p);
        (buildings, walls)
    }

    #[test]
    fn golden_buildings_match_the_python_run() {
        if golden::absent() {
            return;
        }

        let (buildings, _) = munich();
        let want: Vec<golden::GoldenBuilding> = golden::load("buildings.json");
        assert_eq!(buildings.len(), want.len(), "building count");
        assert_eq!(buildings.len(), 173);
        assert_eq!(want.iter().filter(|b| b.target).count(), 69);

        let mut worst_ring = 0.0f64;
        for (got, want) in buildings.iter().zip(&want) {
            assert_eq!(got.key, want.key, "building order");
            assert_eq!(got.kind.as_str(), want.kind, "{}", want.key);
            assert_eq!(got.node_ids, want.node_ids, "{} node ids", want.key);
            assert_eq!(got.target, want.target, "{} target flag", want.key);
            assert_eq!(got.holes.len(), want.holes.len(), "{} holes", want.key);
            assert_eq!(
                got.height_source.as_str(),
                want.height_source,
                "{} height source",
                want.key
            );
            match (got.height_osm, want.height_osm) {
                (Some(a), Some(b)) => assert!((a - b).abs() < 1e-3, "{} height", want.key),
                (None, None) => {}
                other => panic!("{} height {other:?}", want.key),
            }
            assert!((got.min_height - want.min_height).abs() < 1e-3);
            assert_eq!(got.ring.len(), want.ring.len(), "{} ring length", want.key);
            for (p, q) in got.ring.iter().zip(&want.ring) {
                worst_ring = worst_ring.max(golden::max_abs_diff(p, q));
            }
        }
        assert!(worst_ring < 0.1, "worst ring vertex offset {worst_ring} m");
        println!("173 buildings, worst ring vertex {worst_ring:.2e} m");
    }

    /// Every wall of the Munich box to 0.1 m in position and 0.5 degrees in
    /// normal direction, which are the tolerances in PORT_TO_RUST.md.
    #[test]
    fn golden_walls_match_the_python_run() {
        if golden::absent() {
            return;
        }

        let (_, walls) = munich();
        let want: Vec<golden::GoldenWall> = golden::load("walls.json");
        assert_eq!(walls.len(), want.len(), "wall count");
        assert_eq!(walls.len(), 1030);

        let mut worst_pos = 0.0f64;
        let mut worst_normal = 0.0f64;
        let mut worst_len = 0.0f64;
        let mut worst_s = 0.0f64;
        for (got, want) in walls.iter().zip(&want) {
            assert_eq!(got.key, want.key, "wall order");
            assert_eq!(got.building_key, want.building_key);
            assert_eq!(got.idx, want.idx, "{} index", want.key);
            assert_eq!((got.piece, got.n_pieces), (want.piece, want.n_pieces));
            assert_eq!(
                (got.node_a, got.node_b),
                (want.node_a, want.node_b),
                "{} node ids",
                want.key
            );
            assert_eq!(got.merged_idx, want.merged_idx, "{} merged", want.key);
            assert_eq!(
                got.height_source.as_str(),
                want.height_source,
                "{} height source",
                want.key
            );
            match (got.height_osm, want.height_osm) {
                (Some(a), Some(b)) => assert!((a - b).abs() < 1e-3, "{} height", want.key),
                (None, None) => {}
                other => panic!("{} height {other:?}", want.key),
            }
            worst_pos = worst_pos
                .max(golden::max_abs_diff(&got.a, &want.a))
                .max(golden::max_abs_diff(&got.b, &want.b));
            worst_normal = worst_normal.max(golden::angle_between_deg(got.n, want.n));
            worst_len = worst_len.max((got.length - want.length).abs());
            worst_s = worst_s.max((got.s_offset - want.s_offset).abs());

            assert_eq!(got.edges.len(), want.edges.len(), "{} edges", want.key);
            for (e, w) in got.edges.iter().zip(&want.edges) {
                assert_eq!(e.edge_idx, w.edge_idx, "{} edge index", want.key);
                assert_eq!((e.node_a, e.node_b), (w.node_a, w.node_b));
                worst_s = worst_s.max((e.s0 - w.s0).abs()).max((e.s1 - w.s1).abs());
            }
        }
        assert!(worst_pos < 0.1, "worst endpoint offset {worst_pos} m");
        assert!(worst_normal < 0.5, "worst normal {worst_normal} deg");
        assert!(worst_len < 0.1, "worst length {worst_len} m");
        assert!(worst_s < 0.1, "worst s coordinate {worst_s} m");
        println!(
            "1030 walls: worst endpoint {worst_pos:.2e} m, normal {worst_normal:.2e} deg, \
             length {worst_len:.2e} m, s {worst_s:.2e} m"
        );
    }

    /// The Rust defaults are the values the reference run used. A threshold that
    /// drifts during the port should fail here rather than in a facade.
    #[test]
    fn golden_params_match_the_python_defaults() {
        if golden::absent() {
            return;
        }

        let want = golden::load_value("params.json");
        let got = serde_json::to_value(params()).expect("Params serialises");
        let want = want.as_object().expect("params.json is an object");
        let got = got.as_object().expect("Params is an object");
        let mut missing: Vec<&str> = want
            .keys()
            .filter(|k| !got.contains_key(*k))
            .map(String::as_str)
            .collect();
        missing.sort_unstable();
        assert!(missing.is_empty(), "Params is missing {missing:?}");
        let mut extra: Vec<&str> = got
            .keys()
            .filter(|k| !want.contains_key(*k))
            .map(String::as_str)
            .collect();
        extra.sort_unstable();
        assert!(
            extra.is_empty(),
            "Params has fields the Python has not: {extra:?}"
        );
        for (key, w) in want {
            let g = &got[key];
            // Numbers compare by value so that 3 and 3.0 agree; everything else
            // has to be equal outright.
            match (w.as_f64(), g.as_f64()) {
                (Some(a), Some(b)) => assert!((a - b).abs() < 1e-12, "{key}: {a} vs {b}"),
                _ => assert_eq!(w, g, "{key}"),
            }
        }
        println!("{} tunables match", want.len());
    }
}
