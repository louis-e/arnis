//! Suspension bridges whose towers are mapped in OSM as 3D building parts, like
//! the Golden Gate's. The parts render as buildings; this reads their tops and
//! deck level so the deck and the cables hang from the real towers.

use crate::block_definitions::Block;
use crate::element_processing::bridge_styles::BridgePathSample;
use crate::element_processing::buildings::{bridge_support_block, is_bridge_support};
use crate::element_processing::connected_blocks::stair_steps;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;

/// A cluster of support parts this tall (metres) is a cable tower.
const MIN_TOWER_M: f64 = 40.0;
/// Support parts this close (metres) belong to one tower.
const PART_GAP_M: f64 = 30.0;
/// A deck slab is at least this wide either way (metres); struts are thinner.
const MIN_DECK_PART_M: f64 = 8.0;
/// The main cable bottoms out this far above the deck.
const CABLE_LOW_ABOVE_DECK: f32 = 3.0;
/// Saddles sit this far under the tower top.
pub const SADDLE_BELOW_TOP: i32 = 3;
/// Suspender spacing (metres); the Golden Gate's is 50 ft.
const HANGER_SPACING_M: f64 = 15.24;
/// Side spans where no pier marks their end, as a share of the main span.
pub const DEFAULT_SIDE_SPAN: f32 = 0.27;

/// A cable tower mapped in OSM.
#[derive(Clone, Debug)]
pub struct MappedTower {
    pub x: i32,
    pub z: i32,
    pub top_y: i32,
    /// Road level through the portal, when a deck slab is mapped.
    pub deck_y: Option<i32>,
    pub paint: Block,
}

/// Mapped towers, and the lesser supports (piers, pylons) a side span can end at.
#[derive(Default)]
pub struct MappedSupports {
    pub towers: Vec<MappedTower>,
    pub minor: Vec<(i32, i32)>,
}

struct Part<'a> {
    way: &'a ProcessedWay,
    bbox: (i32, i32, i32, i32),
    height_m: Option<f64>,
    min_height_m: f64,
    /// Terrain the part stands on, as `buildings` places it.
    base_y: i32,
}

/// Metres, dropping heights buildings would not draw either.
fn metres(v: Option<&String>) -> Option<f64> {
    v?.trim()
        .trim_end_matches('m')
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|m| (0.0..=1000.0).contains(m))
}

fn part_of<'a>(way: &'a ProcessedWay, editor: &WorldEditor) -> Option<Part<'a>> {
    let first = way.nodes.first()?;
    let mut bbox = (first.x, first.z, first.x, first.z);
    let mut base: Option<i32> = None;
    for n in &way.nodes {
        bbox = (
            bbox.0.min(n.x),
            bbox.1.min(n.z),
            bbox.2.max(n.x),
            bbox.3.max(n.z),
        );
        if let Some(l) = editor.terrain_level(n.x, n.z) {
            base = Some(base.map_or(l, |b| b.max(l)));
        }
    }
    Some(Part {
        way,
        bbox,
        height_m: metres(way.tags.get("height")),
        min_height_m: metres(way.tags.get("min_height")).unwrap_or(0.0),
        base_y: base.unwrap_or_else(|| editor.get_ground_level(first.x, first.z)),
    })
}

fn bbox_gap(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> i32 {
    let dx = (b.0 - a.2).max(a.0 - b.2).max(0);
    let dz = (b.1 - a.3).max(a.1 - b.3).max(0);
    dx.max(dz)
}

/// Groups the bridge support parts into towers and lesser supports.
pub fn collect(elements: &[ProcessedElement], editor: &WorldEditor, scale: f64) -> MappedSupports {
    let is_part =
        |w: &ProcessedWay| w.tags.contains_key("building") || w.tags.contains_key("building:part");
    let supports: Vec<Part> = elements
        .iter()
        .filter_map(|e| match e {
            ProcessedElement::Way(w) if is_part(w) && is_bridge_support(&w.tags) => {
                part_of(w, editor)
            }
            _ => None,
        })
        .collect();
    if supports.is_empty() {
        return MappedSupports::default();
    }

    let gap = (PART_GAP_M * scale).ceil() as i32;
    let mut cluster: Vec<usize> = (0..supports.len()).collect();
    fn root(c: &mut [usize], mut i: usize) -> usize {
        while c[i] != i {
            c[i] = c[c[i]];
            i = c[i];
        }
        i
    }
    for i in 0..supports.len() {
        for j in (i + 1)..supports.len() {
            if bbox_gap(supports[i].bbox, supports[j].bbox) <= gap {
                let (a, b) = (root(&mut cluster, i), root(&mut cluster, j));
                cluster[a] = b;
            }
        }
    }
    let mut groups: std::collections::HashMap<usize, Vec<usize>> = Default::default();
    for i in 0..supports.len() {
        let r = root(&mut cluster, i);
        groups.entry(r).or_default().push(i);
    }
    let mut ids: Vec<Vec<usize>> = groups.into_values().collect();
    ids.sort_by_key(|g| supports[g[0]].way.id);

    let mut out = MappedSupports::default();
    for g in ids {
        let parts: Vec<&Part> = g.iter().map(|&i| &supports[i]).collect();
        let bbox = parts
            .iter()
            .map(|p| p.bbox)
            .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
            .unwrap_or_default();
        let (cx, cz) = ((bbox.0 + bbox.2) / 2, (bbox.1 + bbox.3) / 2);
        let tallest = parts
            .iter()
            .filter_map(|p| p.height_m.map(|h| (h, *p)))
            .max_by(|a, b| a.0.total_cmp(&b.0));
        let Some((top_m, top_part)) = tallest.filter(|(h, _)| *h >= MIN_TOWER_M) else {
            out.minor.push((cx, cz));
            continue;
        };
        // Untagged parts of the same tower, such as its deck slab.
        let near = 3;
        let inside = |w: &ProcessedWay| {
            let n = w.nodes.len().max(1) as i64;
            let ox = (w.nodes.iter().map(|p| p.x as i64).sum::<i64>() / n) as i32;
            let oz = (w.nodes.iter().map(|p| p.z as i64).sum::<i64>() / n) as i32;
            ox >= bbox.0 - near && ox <= bbox.2 + near && oz >= bbox.1 - near && oz <= bbox.3 + near
        };
        let others: Vec<Part> = elements
            .iter()
            .filter_map(|e| match e {
                ProcessedElement::Way(w)
                    if w.tags.contains_key("building:part")
                        && !is_bridge_support(&w.tags)
                        && inside(w) =>
                {
                    part_of(w, editor)
                }
                _ => None,
            })
            .collect();
        // The road runs through the portal on the widest raised slab around the centre.
        let min_side = (MIN_DECK_PART_M * scale) as i32;
        let deck = parts
            .iter()
            .copied()
            .chain(others.iter())
            .filter(|p| {
                let (w, d) = (p.bbox.2 - p.bbox.0, p.bbox.3 - p.bbox.1);
                p.min_height_m >= 2.0
                    && p.height_m.is_some_and(|h| h <= 0.6 * top_m)
                    && w.min(d) >= min_side
                    && (p.bbox.0..=p.bbox.2).contains(&cx)
                    && (p.bbox.1..=p.bbox.3).contains(&cz)
            })
            .max_by_key(|p| (p.bbox.2 - p.bbox.0) * (p.bbox.3 - p.bbox.1))
            .and_then(|p| Some(p.base_y + (p.height_m? * scale).round() as i32 - 1));
        out.towers.push(MappedTower {
            x: cx,
            z: cz,
            top_y: top_part.base_y + (top_m * scale).round() as i32,
            deck_y: deck,
            paint: bridge_support_block(&top_part.way.tags),
        });
    }
    out
}

/// The cables of one suspension structure, shared by every way that hangs from them.
#[derive(Clone, Debug)]
pub struct CableSpan {
    origin: (f32, f32),
    dir: (f32, f32),
    /// Saddles and anchors along the axis, (u, y): anchor, towers, anchor.
    points: Vec<(f32, f32)>,
    /// Sag under the chord at the middle of each stretch between points.
    sags: Vec<f32>,
    hanger_spacing: f32,
    pub paint: Block,
    /// Stiffening girder depth under the deck edges.
    pub girder_depth: i32,
}

impl CableSpan {
    /// `towers` as (u, saddle y) in axis order; `deck_at` gives the deck Y at u.
    pub fn new(
        origin: (f32, f32),
        dir: (f32, f32),
        towers: &[(f32, i32)],
        anchors: (f32, f32),
        deck_at: impl Fn(f32) -> i32,
        paint: Block,
        scale: f64,
    ) -> Option<Self> {
        if towers.len() < 2 || anchors.0 >= towers[0].0 || anchors.1 <= towers[towers.len() - 1].0 {
            return None;
        }
        let mut points = vec![(anchors.0, deck_at(anchors.0) as f32 + 1.0)];
        points.extend(towers.iter().map(|&(u, y)| (u, y as f32)));
        points.push((anchors.1, deck_at(anchors.1) as f32 + 1.0));
        // Two towers at one spot along the axis leave no span to hang.
        if points.windows(2).any(|w| w[1].0 - w[0].0 < 1.0) {
            return None;
        }

        let n = points.len();
        let mut sags = vec![0.0f32; n - 1];
        for i in 1..n - 2 {
            let (a, b) = (points[i], points[i + 1]);
            let low = deck_at((a.0 + b.0) / 2.0) as f32 + CABLE_LOW_ABOVE_DECK;
            sags[i] = ((a.1 + b.1) / 2.0 - low).max(0.0);
        }
        // Same pull on both sides of a saddle, so side sag goes with span squared.
        let main_len = points[2].0 - points[1].0;
        let main_sag = sags[1];
        for i in [0, n - 2] {
            let len = points[i + 1].0 - points[i].0;
            sags[i] = main_sag * (len / main_len).powi(2);
        }
        let main_span = towers[towers.len() - 1].0 - towers[0].0;
        let girder_depth = ((main_span / towers.len().saturating_sub(1).max(1) as f32) / 170.0)
            .round()
            .clamp(2.0, 8.0) as i32;
        Some(Self {
            origin,
            dir,
            points,
            sags,
            hanger_spacing: (HANGER_SPACING_M * scale).max(2.0) as f32,
            paint,
            girder_depth,
        })
    }

    pub fn u(&self, x: i32, z: i32) -> f32 {
        (x as f32 - self.origin.0) * self.dir.0 + (z as f32 - self.origin.1) * self.dir.1
    }

    /// True where the deck hangs from the cables rather than standing on piers.
    pub fn suspends(&self, x: i32, z: i32) -> bool {
        let u = self.u(x, z);
        u >= self.points[0].0 && u <= self.points[self.points.len() - 1].0
    }

    /// Cable height over axis position `u`.
    pub fn cable_y(&self, u: f32) -> Option<f32> {
        let i = self
            .points
            .windows(2)
            .position(|w| u >= w[0].0 && u <= w[1].0)?;
        let (a, b) = (self.points[i], self.points[i + 1]);
        let t = if b.0 > a.0 {
            (u - a.0) / (b.0 - a.0)
        } else {
            0.0
        };
        Some(a.1 + (b.1 - a.1) * t - self.sags[i] * 4.0 * t * (1.0 - t))
    }

    /// Index of the suspender bay at `u`; a hanger stands where it changes.
    pub fn hanger_bay(&self, u: f32) -> i64 {
        (u / self.hanger_spacing).floor() as i64
    }
}

/// Suspension decoration under real towers: cables from saddle to saddle and down
/// to the anchors, suspenders on a fixed pitch, girders under the deck edges.
/// `sides(i)` lists the open deck edges at path sample `i` as (side, cell).
pub fn decorate(
    editor: &mut WorldEditor,
    path: &[BridgePathSample],
    sides: impl Fn(usize) -> Vec<(usize, (i32, i32))>,
    span: &CableSpan,
    hanger: Block,
) {
    let mut prev: [Option<((i32, i32), i32)>; 2] = [None, None];
    let mut prev_bay: Option<i64> = None;
    for (i, &(cx, cy, cz, _)) in path.iter().enumerate() {
        let u = span.u(cx, cz);
        let Some(cable) = span.cable_y(u) else {
            prev = [None, None];
            prev_bay = None;
            continue;
        };
        let cable_y = (cable.round() as i32).max(cy + 2);
        let bay = span.hanger_bay(u);
        let new_bay = prev_bay.is_some_and(|b| b != bay);
        prev_bay = Some(bay);
        for (side, cell) in sides(i) {
            let run = match prev[side] {
                Some((pc, _)) if pc != cell => stair_steps(pc, cell),
                _ => vec![cell],
            };
            // Climbing faster than a block per cell, the cable fills the rise.
            let (lo, hi) = match prev[side] {
                Some((_, py)) if py < cable_y => (py + 1, cable_y),
                Some((_, py)) if py > cable_y => (cable_y, py - 1),
                _ => (cable_y, cable_y),
            };
            for &(rx, rz) in &run {
                for y in (cy - span.girder_depth)..cy {
                    editor.set_block_absolute(span.paint, rx, y, rz, None, Some(&[]));
                }
                editor.set_block_absolute(span.paint, rx, cable_y, rz, None, Some(&[]));
            }
            for y in lo..=hi {
                editor.set_block_absolute(span.paint, cell.0, y, cell.1, None, Some(&[]));
            }
            prev[side] = Some((cell, cable_y));
            if new_bay && cable_y > cy + 3 {
                for y in (cy + 2)..cable_y {
                    editor.set_block_absolute(hanger, cell.0, y, cell.1, None, None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_definitions::ORANGE_TERRACOTTA;

    fn span() -> CableSpan {
        // Golden Gate in blocks: towers 1280 apart with saddles 150 over a level deck at 0.
        CableSpan::new(
            (0.0, 0.0),
            (1.0, 0.0),
            &[(343.0, 150), (1623.0, 150)],
            (0.0, 1966.0),
            |_| 0,
            ORANGE_TERRACOTTA,
            1.0,
        )
        .expect("span")
    }

    #[test]
    fn main_cable_hangs_down_to_the_deck_at_midspan() {
        let s = span();
        let mid = s.cable_y(983.0).unwrap();
        assert!((mid - CABLE_LOW_ABOVE_DECK).abs() < 0.5, "mid {mid}");
        assert!((s.cable_y(343.0).unwrap() - 150.0).abs() < 1e-3);
        assert!((s.cable_y(1623.0).unwrap() - 150.0).abs() < 1e-3);
    }

    #[test]
    fn side_spans_run_from_saddle_to_anchor_with_a_little_sag() {
        let s = span();
        assert!((s.cable_y(0.0).unwrap() - 1.0).abs() < 1e-3);
        let chord = 1.0 + (150.0 - 1.0) * 0.5;
        let side = s.cable_y(171.5).unwrap();
        assert!(
            side < chord && side > chord - 15.0,
            "side {side} chord {chord}"
        );
        assert!(s.cable_y(-1.0).is_none() && s.cable_y(1967.0).is_none());
    }

    #[test]
    fn only_the_suspended_stretch_hangs() {
        let s = span();
        assert!(s.suspends(10, 0));
        assert!(!s.suspends(-10, 0));
        assert!(!s.suspends(2000, 0));
    }

    #[test]
    fn coincident_towers_hang_no_span() {
        let s = CableSpan::new(
            (0.0, 0.0),
            (1.0, 0.0),
            &[(343.0, 150), (343.2, 150)],
            (0.0, 1966.0),
            |_| 0,
            ORANGE_TERRACOTTA,
            1.0,
        );
        assert!(s.is_none());
    }

    #[test]
    fn heights_past_what_buildings_draw_are_dropped() {
        let m = |v: &str| metres(Some(&v.to_string()));
        assert_eq!(m("227"), Some(227.0));
        assert_eq!(m("67.5 m"), Some(67.5));
        for junk in ["inf", "1e10", "NaN", "-5", "1001"] {
            assert_eq!(m(junk), None, "{junk}");
        }
    }

    #[test]
    fn girders_and_hangers_follow_the_real_proportions() {
        let s = span();
        assert_eq!(s.girder_depth, 8);
        assert_eq!(s.hanger_bay(15.0), 0);
        assert_eq!(s.hanger_bay(16.0), 1);
    }
}
