use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::connected_blocks::{cross_cells, four_connected_line, stair_steps};
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::HashMap;

/// Visual style for a bridge. Beam is the legacy/default rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BridgeStyle {
    Beam,
    Arch,
    Truss,
    Suspension,
    CableStayed,
    Covered,
    Boardwalk,
}

impl BridgeStyle {
    pub fn foundation_block(self) -> Block {
        match self {
            BridgeStyle::Boardwalk => OAK_PLANKS,
            _ => STONE_BRICKS,
        }
    }

    pub fn rail_block(self) -> Block {
        match self {
            BridgeStyle::Boardwalk => OAK_FENCE,
            _ => LIGHT_GRAY_CONCRETE,
        }
    }

    /// Centerline cells between pier bents; 0 when the style carries its deck otherwise.
    pub fn pier_interval(self, half_width: i32) -> usize {
        match self {
            BridgeStyle::Boardwalk => BOARDWALK_POST_INTERVAL,
            // Spandrel walls and springer piers carry arches.
            BridgeStyle::Arch => 0,
            BridgeStyle::Truss => TRUSS_PIER_INTERVAL,
            // Approaches only; carriers hang from their pylons.
            BridgeStyle::Suspension | BridgeStyle::CableStayed => CABLE_FALLBACK_PIER_INTERVAL,
            BridgeStyle::Beam | BridgeStyle::Covered => {
                if half_width <= 2 {
                    BEAM_PILLAR_INTERVAL
                } else {
                    WIDE_BEAM_PIER_INTERVAL
                }
            }
        }
    }

    /// True when a lone way of this style hangs from its own pylons.
    pub fn cables_carry_deck(
        self,
        path_len: usize,
        start_is_boundary: bool,
        end_is_boundary: bool,
    ) -> bool {
        !default_pylons(self, path_len, start_is_boundary, end_is_boundary).is_empty()
    }
}

impl BridgeStyle {
    pub fn has_side_railing(self) -> bool {
        self != BridgeStyle::Boardwalk
    }

    pub fn parapet_block(self) -> Option<Block> {
        match self {
            BridgeStyle::Boardwalk => None,
            BridgeStyle::Covered => None,
            BridgeStyle::Truss | BridgeStyle::Suspension | BridgeStyle::CableStayed => {
                Some(IRON_BARS)
            }
            _ => Some(BRICK_WALL),
        }
    }

    pub fn rail_foundation_block(self) -> Block {
        match self {
            BridgeStyle::Boardwalk => OAK_PLANKS,
            _ => STONE_BRICKS,
        }
    }
}

#[derive(Clone, Debug)]
struct OutlineEntry {
    nodes: Vec<(i32, i32)>,
    bbox_min_x: i32,
    bbox_max_x: i32,
    bbox_min_z: i32,
    bbox_max_z: i32,
    structure: Option<String>,
    bridge: Option<String>,
}

/// Lets a bridge way inherit style tags from an overlapping `man_made=bridge` polygon.
#[derive(Default)]
pub struct BridgeOutlineIndex {
    entries: Vec<OutlineEntry>,
}

impl BridgeOutlineIndex {
    pub fn build(elements: &[ProcessedElement]) -> Self {
        let mut entries = Vec::new();
        for elem in elements {
            let ProcessedElement::Way(w) = elem else {
                continue;
            };
            if w.tags.get("man_made").map(|s| s.as_str()) != Some("bridge") {
                continue;
            }
            let structure = w.tags.get("bridge:structure").cloned();
            let bridge = w.tags.get("bridge").cloned();
            let has_style = structure.is_some()
                || bridge.as_deref().is_some_and(|v| {
                    matches!(
                        v,
                        "covered"
                            | "boardwalk"
                            | "cable-stayed"
                            | "cable_stayed"
                            | "suspension"
                            | "suspension_bridge"
                            | "truss"
                    )
                });
            if !has_style || w.nodes.len() < 3 {
                continue;
            }
            let mut nodes: Vec<(i32, i32)> = w.nodes.iter().map(|n| (n.x, n.z)).collect();
            // Close open ways so ray-cast doesn't see a gap.
            if nodes.first() != nodes.last() {
                if let Some(&first) = nodes.first() {
                    nodes.push(first);
                }
            }
            let (mut mnx, mut mnz, mut mxx, mut mxz) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
            for &(x, z) in &nodes {
                mnx = mnx.min(x);
                mxx = mxx.max(x);
                mnz = mnz.min(z);
                mxz = mxz.max(z);
            }
            entries.push(OutlineEntry {
                nodes,
                bbox_min_x: mnx,
                bbox_max_x: mxx,
                bbox_min_z: mnz,
                bbox_max_z: mxz,
                structure,
                bridge,
            });
        }
        Self { entries }
    }

    pub fn style_for_way(&self, way: &ProcessedWay) -> Option<BridgeStyle> {
        if self.entries.is_empty() || way.nodes.is_empty() {
            return None;
        }
        let (cx, cz) = centroid_xz(way);
        for entry in &self.entries {
            if cx < entry.bbox_min_x
                || cx > entry.bbox_max_x
                || cz < entry.bbox_min_z
                || cz > entry.bbox_max_z
            {
                continue;
            }
            if !point_in_polygon(cx, cz, &entry.nodes) {
                continue;
            }
            let style =
                resolve_bridge_style_from_pair(entry.structure.as_deref(), entry.bridge.as_deref());
            if style != BridgeStyle::Beam {
                return Some(style);
            }
        }
        None
    }
}

/// Resolve a way's style, falling back to an overlapping `man_made=bridge` outline.
pub fn resolve_bridge_style_with_outline(
    way: &ProcessedWay,
    outlines: &BridgeOutlineIndex,
) -> BridgeStyle {
    let direct = resolve_bridge_style(&way.tags);
    if direct != BridgeStyle::Beam {
        return direct;
    }
    outlines.style_for_way(way).unwrap_or(BridgeStyle::Beam)
}

fn centroid_xz(way: &ProcessedWay) -> (i32, i32) {
    let n = way.nodes.len() as i64;
    if n == 0 {
        return (0, 0);
    }
    let mut sx: i64 = 0;
    let mut sz: i64 = 0;
    for node in &way.nodes {
        sx += node.x as i64;
        sz += node.z as i64;
    }
    ((sx / n) as i32, (sz / n) as i32)
}

fn point_in_polygon(x: i32, z: i32, poly: &[(i32, i32)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let xf = x as f64;
    let zf = z as f64;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, zi) = (poly[i].0 as f64, poly[i].1 as f64);
        let (xj, zj) = (poly[j].0 as f64, poly[j].1 as f64);
        let intersects =
            ((zi > zf) != (zj > zf)) && xf < (xj - xi) * (zf - zi) / (zj - zi + f64::EPSILON) + xi;
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub fn resolve_bridge_style(tags: &HashMap<String, String>) -> BridgeStyle {
    resolve_bridge_style_from_pair(
        tags.get("bridge:structure").map(String::as_str),
        tags.get("bridge").map(String::as_str),
    )
}

fn resolve_bridge_style_from_pair(structure: Option<&str>, bridge: Option<&str>) -> BridgeStyle {
    if let Some(s) = structure {
        match s {
            "arch" => return BridgeStyle::Arch,
            "truss" => return BridgeStyle::Truss,
            "suspension" | "simple-suspension" => return BridgeStyle::Suspension,
            "cable-stayed" | "cable_stayed" => return BridgeStyle::CableStayed,
            "beam" => return BridgeStyle::Beam,
            _ => {}
        }
    }
    if let Some(b) = bridge {
        match b {
            // Plain viaducts fall through to Beam so they get pillar deck modules.
            "covered" => return BridgeStyle::Covered,
            "boardwalk" => return BridgeStyle::Boardwalk,
            // Discouraged but still mapped in the wild.
            "cable-stayed" | "cable_stayed" => return BridgeStyle::CableStayed,
            "suspension" | "suspension_bridge" => return BridgeStyle::Suspension,
            "truss" => return BridgeStyle::Truss,
            _ => {}
        }
    }
    BridgeStyle::Beam
}

const BEAM_PILLAR_INTERVAL: usize = 8;
const WIDE_BEAM_PIER_INTERVAL: usize = 12;
const TRUSS_PIER_INTERVAL: usize = 32;
const CABLE_FALLBACK_PIER_INTERVAL: usize = 24;
const BOARDWALK_POST_INTERVAL: usize = 4;
const ARCH_SPAN: usize = 20;
const ARCH_RISE_FRACTION: f32 = 0.85;
const TRUSS_TOP_HEIGHT: i32 = 5;
const TRUSS_DIAGONAL_PERIOD: usize = 8;
const TRUSS_POST_INTERVAL: usize = 4;
const TRUSS_PORTAL_INTERVAL: usize = 8;
const SUSPENSION_TOWER_BASE_HEIGHT: i32 = 8;
const SUSPENSION_TOWER_HEIGHT_DIVISOR: usize = 6;
const SUSPENSION_TOWER_MAX_HEIGHT: i32 = 32;
const SUSPENSION_HANGER_INTERVAL: usize = 4;
const SUSPENSION_TOWER_INSET_FRAC: f32 = 0.12;
const SUSPENSION_MIN_LENGTH: usize = 18;
// Two towers except on very long spans.
const SUSPENSION_INTER_PYLON_SPACING: usize = 1000;
const SUSPENSION_MAX_PYLONS: usize = 5;
const CABLE_STAYED_TOWER_BASE_HEIGHT: i32 = 12;
const CABLE_STAYED_TOWER_HEIGHT_DIVISOR: usize = 5;
const CABLE_STAYED_TOWER_MAX_HEIGHT: i32 = 40;
const CABLE_STAYED_ANCHOR_INTERVAL: usize = 14;
const CABLE_STAYED_MIN_LENGTH: usize = 18;
const CABLE_STAYED_MIN_GAP: usize = 4;
const CABLE_STAYED_TWIN_PYLON_LENGTH: usize = 100;
const COVERED_WALL_HEIGHT: i32 = 4;
const COVERED_WINDOW_INTERVAL: usize = 4;
const COVERED_END_CLEAR: usize = 1;

fn suspension_tower_height(total: usize) -> i32 {
    let extra = (total / SUSPENSION_TOWER_HEIGHT_DIVISOR) as i32;
    (SUSPENSION_TOWER_BASE_HEIGHT + extra).min(SUSPENSION_TOWER_MAX_HEIGHT)
}

/// Pylon inset from each end, or None when the span is too short to hang cables from.
fn suspension_inset(total: usize) -> Option<usize> {
    if total < SUSPENSION_MIN_LENGTH {
        return None;
    }
    let inset = (((total as f32) * SUSPENSION_TOWER_INSET_FRAC) as usize).max(2);
    (inset * 2 + 2 <= total).then_some(inset)
}

fn suspension_pylon_count(total: usize) -> usize {
    (2 + total / SUSPENSION_INTER_PYLON_SPACING).min(SUSPENSION_MAX_PYLONS)
}

fn cable_stayed_tower_height(total: usize) -> i32 {
    let extra = (total / CABLE_STAYED_TOWER_HEIGHT_DIVISOR) as i32;
    (CABLE_STAYED_TOWER_BASE_HEIGHT + extra).min(CABLE_STAYED_TOWER_MAX_HEIGHT)
}

/// Arch spandrel fill for one deck cell, plus springer piers on the centerline.
#[allow(clippy::too_many_arguments)]
pub fn place_arch_below_deck(
    editor: &mut WorldEditor,
    set_x: i32,
    cell_y: i32,
    set_z: i32,
    centerline_ground_y: i32,
    tds: usize,
    total: usize,
    use_absolute_y: bool,
    is_centerline: bool,
) {
    place_arch_spandrel_cell(
        editor,
        set_x,
        cell_y,
        set_z,
        centerline_ground_y,
        tds,
        total,
        use_absolute_y,
    );
    if is_centerline {
        let (start, span) = arch_segment(tds, total);
        if tds == start || tds + 1 == start + span {
            place_pillar(editor, set_x, cell_y, set_z, STONE_BRICKS, true);
        }
    }
}

/// One pier across the deck; columns over a road, track or lower deck are left out.
#[allow(clippy::too_many_arguments)]
pub fn place_pier_bent(
    editor: &mut WorldEditor,
    surface: &BridgeSurfaceMap,
    style: BridgeStyle,
    x: i32,
    z: i32,
    deck_y: i32,
    perp: (f32, f32),
    half_width: i32,
) {
    let at = |off: i32| {
        (
            (x as f32 + perp.0 * off as f32).round() as i32,
            (z as f32 + perp.1 * off as f32).round() as i32,
        )
    };
    let (body, footing, mut cells): (Block, bool, Vec<(i32, i32)>) = match style {
        BridgeStyle::Boardwalk if half_width >= 1 => {
            (OAK_LOG, false, vec![at(-half_width), at(half_width)])
        }
        BridgeStyle::Boardwalk => (OAK_LOG, false, vec![at(0)]),
        BridgeStyle::Truss => (
            STONE_BRICKS,
            true,
            cross_cells(x, z, perp, -half_width, half_width),
        ),
        _ => {
            let edge = half_width - 1;
            let offsets = match half_width {
                ..=2 => vec![0],
                3..=4 => vec![-edge, edge],
                _ => vec![-edge, 0, edge],
            };
            (STONE_BRICKS, true, offsets.into_iter().map(at).collect())
        }
    };
    cells.dedup();
    for (px, pz) in cells {
        if !surface.support_blocked(px, pz, deck_y) {
            place_pillar(editor, px, deck_y, pz, body, footing);
        }
    }
}

/// Deck rows above the ground below which a pier gets a 3x3 footing.
const PIER_FOOTING_MIN_HEIGHT: i32 = 5;

/// Pier column from the terrain to the deck; a water carve later extends it to the bed.
pub(crate) fn place_pillar(
    editor: &mut WorldEditor,
    x: i32,
    deck_y: i32,
    z: i32,
    body: Block,
    with_base: bool,
) {
    let ground_y = editor.get_ground_level(x, z);
    // The foundation row under the deck already rests on the terrain.
    if deck_y - ground_y <= 2 {
        return;
    }
    for y in (ground_y + 1)..deck_y {
        editor.set_block_absolute(body, x, y, z, None, None);
    }
    editor.register_support_column(x, z, body);
    if with_base && deck_y - ground_y >= PIER_FOOTING_MIN_HEIGHT {
        for bx in -1..=1 {
            for bz in -1..=1 {
                editor.set_block_absolute(body, x + bx, ground_y, z + bz, None, None);
            }
        }
    }
}

// Returns (arch_start_tds, arch_span_in_cells) for the arch this cell belongs to.
fn arch_segment(tds: usize, total: usize) -> (usize, usize) {
    if total < 2 {
        return (0, total);
    }
    let n_arches = ((total + ARCH_SPAN / 2) / ARCH_SPAN).max(1);
    let arch_idx = (tds * n_arches) / total;
    let arch_start = (total * arch_idx) / n_arches;
    let arch_end = (total * (arch_idx + 1)) / n_arches;
    (arch_start, arch_end - arch_start)
}

// Position within current arch in [0.0, 1.0]; 0.5 is the crown.
fn arch_local_t(tds: usize, total: usize) -> f32 {
    let (start, span) = arch_segment(tds, total);
    if span <= 1 {
        return 0.0;
    }
    (tds - start) as f32 / (span - 1) as f32
}

#[allow(clippy::too_many_arguments)]
fn place_arch_spandrel_cell(
    editor: &mut WorldEditor,
    set_x: i32,
    cell_y: i32,
    set_z: i32,
    centerline_ground_y: i32,
    tds: usize,
    total: usize,
    use_absolute_y: bool,
) {
    let dist_to_deck = (cell_y - 2 - centerline_ground_y).max(0);
    if dist_to_deck <= 0 {
        return;
    }
    let max_rise = ((dist_to_deck as f32) * ARCH_RISE_FRACTION) as i32;
    let t = arch_local_t(tds, total);
    // Parabola: 0 at springer, max_rise at crown.
    let rise_at_cell = ((max_rise as f32) * 4.0 * t * (1.0 - t)) as i32;
    let arch_under_y = centerline_ground_y + rise_at_cell;
    let fill_top = cell_y - 2;
    if arch_under_y > fill_top {
        return;
    }
    for fy in arch_under_y..=fill_top {
        if use_absolute_y {
            editor.set_block_absolute(STONE_BRICKS, set_x, fy, set_z, None, Some(&[WATER]));
        } else {
            editor.set_block(STONE_BRICKS, set_x, fy, set_z, None, Some(&[WATER]));
        }
    }
    // Springer walls continue to the bed in a river.
    if use_absolute_y && arch_under_y <= centerline_ground_y + 1 {
        editor.register_support_column(set_x, set_z, STONE_BRICKS);
    }
}

/// One centerline sample: (x, deck_y, z, unit_perp).
pub type BridgePathSample = (i32, i32, i32, (f32, f32));

/// Deck sides facing open air; a side against another deck gets no truss, pylon or wall.
#[derive(Clone, Copy)]
struct OpenSides {
    left: bool,
    right: bool,
}

impl OpenSides {
    fn of(surface: &BridgeSurfaceMap, path: &[BridgePathSample], block_range: i32) -> Self {
        let (mut left, mut right) = (0usize, 0usize);
        for &(cx, cy, cz, perp) in path {
            let (l, r) = side_offsets(cx, cz, perp, block_range);
            left += side_faces_open(surface, l, perp, cy) as usize;
            right += side_faces_open(surface, r, (-perp.0, -perp.1), cy) as usize;
        }
        // Decided per member so the truss isn't ragged.
        Self {
            left: left * 2 >= path.len(),
            right: right * 2 >= path.len(),
        }
    }

    fn pick<T>(self, left: T, right: T) -> impl Iterator<Item = T> {
        self.indexed(left, right).map(|(_, v)| v)
    }

    /// Open sides with their index (0 left, 1 right).
    fn indexed<T>(self, left: T, right: T) -> impl Iterator<Item = (usize, T)> {
        [(self.left, left), (self.right, right)]
            .into_iter()
            .enumerate()
            .filter_map(|(i, (open, v))| open.then_some((i, v)))
    }
}

/// True when no deck continues outward from the edge at (x, z).
pub(crate) fn side_faces_open(
    surface: &BridgeSurfaceMap,
    (x, z): (i32, i32),
    outward: (f32, f32),
    deck_y: i32,
) -> bool {
    (0..=2).all(|k| {
        let px = (x as f32 + outward.0 * k as f32).round() as i32;
        let pz = (z as f32 + outward.1 * k as f32).round() as i32;
        !surface.deck_near(px, pz, deck_y, 2)
    })
}

/// Pylon path indices for a lone cable-carried way; empty when it can't carry itself.
pub fn default_pylons(
    style: BridgeStyle,
    total: usize,
    start_is_boundary: bool,
    end_is_boundary: bool,
) -> Vec<usize> {
    match style {
        BridgeStyle::Suspension if start_is_boundary && end_is_boundary => {
            let Some(inset) = suspension_inset(total) else {
                return Vec::new();
            };
            let n_pylons = suspension_pylon_count(total);
            let (first, last) = (inset, total - 1 - inset);
            // Evenly distribute pylons between the two boundary insets.
            (0..n_pylons)
                .map(|i| first + (last - first) * i / (n_pylons - 1).max(1))
                .collect()
        }
        BridgeStyle::CableStayed if total >= CABLE_STAYED_MIN_LENGTH => {
            // Twin pylons split the deck so their cable fans don't cross.
            if total >= CABLE_STAYED_TWIN_PYLON_LENGTH {
                vec![total / 3, (2 * total) / 3]
            } else {
                vec![total / 2]
            }
        }
        _ => Vec::new(),
    }
}

/// Above-deck decoration. `pylon_points` puts cable pylons where the main span has them.
#[allow(clippy::too_many_arguments)]
pub fn decorate_bridge_above_deck(
    editor: &mut WorldEditor,
    surface: &BridgeSurfaceMap,
    style: BridgeStyle,
    path: &[BridgePathSample],
    block_range: i32,
    start_is_boundary: bool,
    end_is_boundary: bool,
    pylon_points: Option<&[(i32, i32)]>,
) {
    if path.len() < 4 {
        return;
    }
    let sides = OpenSides::of(surface, path, block_range);
    let pylons = || -> Vec<usize> {
        let mut pylons = match pylon_points {
            Some(points) => points
                .iter()
                .filter_map(|&(px, pz)| {
                    (0..path.len()).min_by_key(|&i| {
                        let (x, _, z, _) = path[i];
                        (x - px).pow(2) + (z - pz).pow(2)
                    })
                })
                .collect(),
            None => default_pylons(style, path.len(), start_is_boundary, end_is_boundary),
        };
        pylons.sort_unstable();
        pylons.dedup();
        pylons
    };
    match style {
        BridgeStyle::Truss => decorate_truss(
            editor,
            path,
            block_range,
            sides,
            start_is_boundary,
            end_is_boundary,
        ),
        BridgeStyle::Suspension => decorate_suspension(editor, path, block_range, sides, &pylons()),
        BridgeStyle::CableStayed => {
            decorate_cable_stayed(editor, path, block_range, sides, &pylons())
        }
        BridgeStyle::Covered => decorate_covered(
            editor,
            path,
            block_range,
            sides,
            start_is_boundary,
            end_is_boundary,
        ),
        _ => {}
    }
}

fn side_offsets(cx: i32, cz: i32, perp: (f32, f32), block_range: i32) -> ((i32, i32), (i32, i32)) {
    let (px, pz) = perp;
    let rail_dist = block_range as f32 * (px.abs() + pz.abs()) + 1.0;
    let lx = (cx as f32 + px * rail_dist).round() as i32;
    let lz = (cz as f32 + pz * rail_dist).round() as i32;
    let rx = (cx as f32 - px * rail_dist).round() as i32;
    let rz = (cz as f32 - pz * rail_dist).round() as i32;
    ((lx, lz), (rx, rz))
}

/// Per-side trail of edge cells, so chords, cables and walls stay joined on diagonal decks.
#[derive(Default)]
struct SideRuns {
    prev: [Option<(i32, i32)>; 2],
}

impl SideRuns {
    /// Cells from the side's previous edge cell up to `cell`.
    fn step(&mut self, side: usize, cell: (i32, i32)) -> Vec<(i32, i32)> {
        let cells = match self.prev[side] {
            Some(p) if p != cell => stair_steps(p, cell),
            _ => vec![cell],
        };
        self.prev[side] = Some(cell);
        cells
    }
}

/// Cells from `a` to `b` without corner-to-corner steps.
fn span_cells(a: (i32, i32), b: (i32, i32)) -> Vec<(i32, i32)> {
    let line: Vec<(i32, i32)> = bresenham_line(a.0, 0, a.1, b.0, 0, b.1)
        .into_iter()
        .map(|(x, _, z)| (x, z))
        .collect();
    four_connected_line(&line)
}

fn decorate_truss(
    editor: &mut WorldEditor,
    path: &[BridgePathSample],
    block_range: i32,
    sides: OpenSides,
    start_is_boundary: bool,
    end_is_boundary: bool,
) {
    let last = path.len() - 1;
    let mut runs = SideRuns::default();
    for (tds, &(cx, cy, cz, perp)) in path.iter().enumerate() {
        let (left, right) = side_offsets(cx, cz, perp, block_range);
        // Leave entry/exit clear at group boundaries only; mid-group seams stay closed.
        let open_end = (tds == 0 && start_is_boundary) || (tds == last && end_is_boundary);
        let top_y = cy + 1 + TRUSS_TOP_HEIGHT;
        // Warren-style sawtooth diagonal: 0,1,2,3,4,3,2,1 over period 8.
        let p = tds % TRUSS_DIAGONAL_PERIOD;
        let half = TRUSS_DIAGONAL_PERIOD / 2;
        let dh = if p <= half {
            p
        } else {
            TRUSS_DIAGONAL_PERIOD - p
        } as i32;
        let diag_y = cy + 1 + dh.min(TRUSS_TOP_HEIGHT);
        for (side, (sx, sz)) in sides.indexed(left, right) {
            let run = runs.step(side, (sx, sz));
            if open_end {
                continue;
            }
            for (rx, rz) in run {
                editor.set_block_absolute(IRON_BLOCK, rx, cy + 1, rz, None, None);
                editor.set_block_absolute(IRON_BLOCK, rx, top_y, rz, None, None);
            }
            if tds.is_multiple_of(TRUSS_POST_INTERVAL) {
                for h in 1..=TRUSS_TOP_HEIGHT {
                    editor.set_block_absolute(IRON_BLOCK, sx, cy + 1 + h, sz, None, None);
                }
            }
            editor.set_block_absolute(IRON_BLOCK, sx, diag_y, sz, None, None);
        }

        // Portal bracing only across a deck trussed on both sides.
        if !open_end && sides.left && sides.right && tds.is_multiple_of(TRUSS_PORTAL_INTERVAL) {
            for (bx, bz) in span_cells(left, right) {
                editor.set_block_absolute(IRON_BLOCK, bx, top_y, bz, None, None);
            }
        }
    }
}

fn decorate_suspension(
    editor: &mut WorldEditor,
    path: &[BridgePathSample],
    block_range: i32,
    sides: OpenSides,
    pylons: &[usize],
) {
    let total = path.len();
    let (Some(&first_p), Some(&last_p)) = (pylons.first(), pylons.last()) else {
        return;
    };
    let last_idx = total - 1;
    let height = suspension_tower_height(total);

    for &p in pylons {
        let (cx, deck_y, cz, perp) = path[p];
        place_pylon_pair(editor, (cx, cz), perp, block_range, deck_y, height, sides);
    }
    let sided = |left: (i32, i32), right: (i32, i32)| sides.pick(left, right).collect::<Vec<_>>();

    // One catenary cable per inter-pylon span.
    let dip = (height - 2) as f32;
    for w in pylons.windows(2) {
        let a = w[0];
        let b = w[1];
        let span_len = (b - a) as f32;
        if span_len < 1.0 {
            continue;
        }
        let cy_a = path[a].1;
        let cy_b = path[b].1;
        let top_a = cy_a + height;
        let top_b = cy_b + height;
        let mut runs = SideRuns::default();
        for (tds, sample) in path.iter().enumerate().take(b + 1).skip(a) {
            let &(cx, cy, cz, perp) = sample;
            let (left, right) = side_offsets(cx, cz, perp, block_range);
            let t = (tds - a) as f32 / span_len;
            let base_y = (top_a as f32) + ((top_b - top_a) as f32) * t;
            let cable_y = (base_y - dip * 4.0 * t * (1.0 - t)).round() as i32;
            let chain = if perp.0.abs() > perp.1.abs() {
                CHAIN_Z
            } else {
                CHAIN_X
            };
            let on_hanger_step = (tds - a).is_multiple_of(SUSPENSION_HANGER_INTERVAL);
            for (side, (sx, sz)) in sides.indexed(left, right) {
                for (rx, rz) in runs.step(side, (sx, sz)) {
                    editor.set_block_absolute(chain, rx, cable_y, rz, None, None);
                }
                if on_hanger_step && tds != a && tds != b {
                    for hy in (cy + 2)..cable_y {
                        editor.set_block_absolute(IRON_BARS, sx, hy, sz, None, None);
                    }
                }
            }
        }
    }

    // Anchor cables from end pylons to deck endpoints.
    let anchors = [(first_p, 0), (last_p, last_idx)];
    for (pylon, end) in anchors {
        if pylon == end {
            continue;
        }
        let (cx_p, cy_p, cz_p, perp_p) = path[pylon];
        let (left_p, right_p) = side_offsets(cx_p, cz_p, perp_p, block_range);
        let (cx_e, cy_e, cz_e, perp_e) = path[end];
        let (left_e, right_e) = side_offsets(cx_e, cz_e, perp_e, block_range);
        for (top, foot) in sided(left_p, right_p)
            .into_iter()
            .zip(sided(left_e, right_e))
        {
            draw_cable(
                editor,
                top.0,
                cy_p + height,
                top.1,
                foot.0,
                cy_e + 1,
                foot.1,
            );
        }
    }
}

fn decorate_cable_stayed(
    editor: &mut WorldEditor,
    path: &[BridgePathSample],
    block_range: i32,
    sides: OpenSides,
    pylons: &[usize],
) {
    let total = path.len();
    if pylons.is_empty() {
        return;
    }
    let last_idx = total - 1;
    let height = cable_stayed_tower_height(total);

    for (idx, &t_tds) in pylons.iter().enumerate() {
        let (cx_t, cy_t, cz_t, perp_t) = path[t_tds];
        let (left_t, right_t) = side_offsets(cx_t, cz_t, perp_t, block_range);
        let top_y = cy_t + height;
        place_pylon_pair(
            editor,
            (cx_t, cz_t),
            perp_t,
            block_range,
            cy_t,
            height,
            sides,
        );
        let tops: Vec<(i32, i32)> = sides.pick(left_t, right_t).collect();

        // Each pylon's fan ends halfway to its neighbour, so fans never cross.
        let anchor_lo = idx
            .checked_sub(1)
            .map_or(0, |prev| (pylons[prev] + t_tds) / 2);
        let anchor_hi = pylons
            .get(idx + 1)
            .map_or(total, |&next| (t_tds + next) / 2);

        // Symmetric fan around the pylon.
        let anchors = (1..)
            .map(|k| k * CABLE_STAYED_ANCHOR_INTERVAL)
            .take_while(|&d| d < total)
            .flat_map(|d| [t_tds.checked_sub(d), Some(t_tds + d)])
            .flatten()
            .filter(|&tds| {
                (anchor_lo..anchor_hi).contains(&tds)
                    && tds != 0
                    && tds < last_idx
                    && tds.abs_diff(t_tds) >= CABLE_STAYED_MIN_GAP
            });
        for tds in anchors {
            let (cx_a, cy_a, cz_a, perp_a) = path[tds];
            let (left_a, right_a) = side_offsets(cx_a, cz_a, perp_a, block_range);
            for (top, foot) in tops.iter().zip(sides.pick(left_a, right_a)) {
                draw_cable(editor, top.0, top_y, top.1, foot.0, cy_a + 1, foot.1);
            }
        }
    }
}

fn decorate_covered(
    editor: &mut WorldEditor,
    path: &[BridgePathSample],
    block_range: i32,
    sides: OpenSides,
    start_is_boundary: bool,
    end_is_boundary: bool,
) {
    let total = path.len();
    if total < 4 {
        return;
    }
    let last = total - 1;
    let mut runs = SideRuns::default();
    for (tds, &(cx, cy, cz, perp)) in path.iter().enumerate() {
        let (left, right) = side_offsets(cx, cz, perp, block_range);
        let open_end = (start_is_boundary && tds < COVERED_END_CLEAR)
            || (end_is_boundary && tds + COVERED_END_CLEAR > last);
        for (side, sample) in sides.indexed(left, right) {
            let run = runs.step(side, sample);
            if open_end {
                continue;
            }
            for cell in run {
                for h in 1..=COVERED_WALL_HEIGHT {
                    let window = h == 2 && cell == sample && tds % COVERED_WINDOW_INTERVAL == 0;
                    let block = if window { GLASS } else { DARK_OAK_PLANKS };
                    editor.set_block_absolute(block, cell.0, cy + h, cell.1, None, None);
                }
            }
        }
        if open_end {
            continue;
        }
        let roof_y = cy + COVERED_WALL_HEIGHT + 1;
        for (rx, rz) in span_cells(left, right) {
            editor.set_block_absolute(DARK_OAK_PLANKS, rx, roof_y, rz, None, None);
        }
    }
}

/// Pylons on the open sides at one path sample, joined by a crossbeam when both stand.
#[allow(clippy::too_many_arguments)]
fn place_pylon_pair(
    editor: &mut WorldEditor,
    (cx, cz): (i32, i32),
    perp: (f32, f32),
    block_range: i32,
    deck_y: i32,
    height: i32,
    sides: OpenSides,
) {
    let (left, right) = side_offsets(cx, cz, perp, block_range);
    let (px, pz) = perp;
    for (base, outward) in sides.pick((left, (px, pz)), (right, (-px, -pz))) {
        place_pylon(editor, base, outward, deck_y, height);
    }
    if sides.left && sides.right {
        place_pylon_crossbeam(editor, left, right, deck_y + height);
    }
}

/// A 2x2 tower grown outward from the deck edge, from the ground up.
fn place_pylon(
    editor: &mut WorldEditor,
    (x, z): (i32, i32),
    outward: (f32, f32),
    deck_y: i32,
    height: i32,
) {
    let along = (outward.1, -outward.0);
    let top_y = deck_y + height;
    let mut cells: Vec<(i32, i32)> = Vec::with_capacity(4);
    for (a, b) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let cell = (
            (x as f32 + outward.0 * a + along.0 * b).round() as i32,
            (z as f32 + outward.1 * a + along.1 * b).round() as i32,
        );
        if !cells.contains(&cell) {
            cells.push(cell);
        }
    }
    for (px, pz) in cells {
        let base_y = editor.get_ground_level(px, pz).min(deck_y);
        for y in (base_y + 1)..=top_y {
            editor.set_block_absolute(SMOOTH_STONE, px, y, pz, None, None);
        }
        editor.register_support_column(px, pz, SMOOTH_STONE);
    }
}

fn place_pylon_crossbeam(
    editor: &mut WorldEditor,
    left: (i32, i32),
    right: (i32, i32),
    top_y: i32,
) {
    for (cx, cz) in span_cells(left, right) {
        editor.set_block_absolute(SMOOTH_STONE, cx, top_y, cz, None, None);
    }
}

fn draw_cable(editor: &mut WorldEditor, x1: i32, y1: i32, z1: i32, x2: i32, y2: i32, z2: i32) {
    let dx = x2 - x1;
    let dz = z2 - z1;
    let chain = if dx.abs() >= dz.abs() {
        CHAIN_X
    } else {
        CHAIN_Z
    };
    let mut prev: Option<(i32, i32, i32)> = None;
    for (cx, cy, cz) in bresenham_line(x1, y1, z1, x2, y2, z2) {
        editor.set_block_absolute(chain, cx, cy, cz, None, None);
        if let Some((px, py, pz)) = prev {
            let axes_changed = (cx != px) as i32 + (cy != py) as i32 + (cz != pz) as i32;
            // Fill the L-corner left by multi-axis bresenham steps so the line reads continuously.
            if axes_changed >= 2 {
                editor.set_block_absolute(chain, cx, py, cz, None, None);
                if axes_changed == 3 {
                    editor.set_block_absolute(chain, cx, py, pz, None, None);
                }
            }
        }
        prev = Some((cx, cy, cz));
    }
}
