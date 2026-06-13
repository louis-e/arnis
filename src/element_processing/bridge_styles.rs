use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
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

    pub fn pillar_interval(self) -> usize {
        match self {
            BridgeStyle::Boardwalk => BOARDWALK_POST_INTERVAL,
            _ => BEAM_PILLAR_INTERVAL,
        }
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
                        "viaduct"
                            | "covered"
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
            "viaduct" => return BridgeStyle::Arch,
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
const SUSPENSION_INTER_PYLON_SPACING: usize = 100;
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

fn suspension_pylon_count(total: usize) -> usize {
    (2 + total / SUSPENSION_INTER_PYLON_SPACING).min(SUSPENSION_MAX_PYLONS)
}

fn cable_stayed_tower_height(total: usize) -> i32 {
    let extra = (total / CABLE_STAYED_TOWER_HEIGHT_DIVISOR) as i32;
    (CABLE_STAYED_TOWER_BASE_HEIGHT + extra).min(CABLE_STAYED_TOWER_MAX_HEIGHT)
}

/// Below-deck support for one cell. Caller decides centerline / pillar-grid hits.
#[allow(clippy::too_many_arguments)]
pub fn place_bridge_support_below_deck(
    editor: &mut WorldEditor,
    style: BridgeStyle,
    set_x: i32,
    cell_y: i32,
    set_z: i32,
    centerline_ground_y: i32,
    tds: usize,
    total: usize,
    use_absolute_y: bool,
    is_centerline: bool,
    is_pillar_position: bool,
) {
    match style {
        BridgeStyle::Arch => {
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
        BridgeStyle::Boardwalk => {
            if is_centerline && is_pillar_position {
                place_pillar(editor, set_x, cell_y, set_z, OAK_LOG, false);
            }
        }
        _ => {
            if is_centerline && is_pillar_position {
                place_pillar(editor, set_x, cell_y, set_z, STONE_BRICKS, true);
            }
        }
    }
}

fn place_pillar(
    editor: &mut WorldEditor,
    x: i32,
    deck_y: i32,
    z: i32,
    body: Block,
    with_base: bool,
) {
    let ground_y = editor.get_ground_level(x, z);
    if deck_y <= ground_y {
        return;
    }
    for y in (ground_y + 1)..deck_y {
        editor.set_block_absolute(body, x, y, z, None, None);
    }
    if with_base {
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
}

/// One centerline sample: (x, deck_y, z, unit_perp).
pub type BridgePathSample = (i32, i32, i32, (f32, f32));

/// Above-deck decoration; no-op for Beam/Arch/Boardwalk.
pub fn decorate_bridge_above_deck(
    editor: &mut WorldEditor,
    style: BridgeStyle,
    path: &[BridgePathSample],
    block_range: i32,
    start_is_boundary: bool,
    end_is_boundary: bool,
) {
    if path.len() < 4 {
        return;
    }
    match style {
        BridgeStyle::Truss => decorate_truss(
            editor,
            path,
            block_range,
            start_is_boundary,
            end_is_boundary,
        ),
        BridgeStyle::Suspension => decorate_suspension(
            editor,
            path,
            block_range,
            start_is_boundary,
            end_is_boundary,
        ),
        BridgeStyle::CableStayed => decorate_cable_stayed(editor, path, block_range),
        BridgeStyle::Covered => decorate_covered(
            editor,
            path,
            block_range,
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

fn decorate_truss(
    editor: &mut WorldEditor,
    path: &[BridgePathSample],
    block_range: i32,
    start_is_boundary: bool,
    end_is_boundary: bool,
) {
    let last = path.len() - 1;
    for (tds, &(cx, cy, cz, perp)) in path.iter().enumerate() {
        // Leave entry/exit clear at group boundaries only; mid-group seams stay closed.
        if (tds == 0 && start_is_boundary) || (tds == last && end_is_boundary) {
            continue;
        }
        let (left, right) = side_offsets(cx, cz, perp, block_range);
        let top_y = cy + 1 + TRUSS_TOP_HEIGHT;
        // Bottom and top chord at every cell.
        editor.set_block_absolute(IRON_BLOCK, left.0, cy + 1, left.1, None, None);
        editor.set_block_absolute(IRON_BLOCK, right.0, cy + 1, right.1, None, None);
        editor.set_block_absolute(IRON_BLOCK, left.0, top_y, left.1, None, None);
        editor.set_block_absolute(IRON_BLOCK, right.0, top_y, right.1, None, None);

        // Vertical posts.
        if tds.is_multiple_of(TRUSS_POST_INTERVAL) {
            for h in 1..=TRUSS_TOP_HEIGHT {
                editor.set_block_absolute(IRON_BLOCK, left.0, cy + 1 + h, left.1, None, None);
                editor.set_block_absolute(IRON_BLOCK, right.0, cy + 1 + h, right.1, None, None);
            }
        }

        // Warren-style sawtooth diagonal: 0,1,2,3,4,3,2,1 over period 8.
        let p = tds % TRUSS_DIAGONAL_PERIOD;
        let half = TRUSS_DIAGONAL_PERIOD / 2;
        let dh = if p <= half {
            p
        } else {
            TRUSS_DIAGONAL_PERIOD - p
        } as i32;
        let diag_y = cy + 1 + dh.min(TRUSS_TOP_HEIGHT);
        editor.set_block_absolute(IRON_BLOCK, left.0, diag_y, left.1, None, None);
        editor.set_block_absolute(IRON_BLOCK, right.0, diag_y, right.1, None, None);

        // Portal-style top cross-bracing every TRUSS_PORTAL_INTERVAL cells.
        if tds.is_multiple_of(TRUSS_PORTAL_INTERVAL) {
            for (bx, _, bz) in bresenham_line(left.0, top_y, left.1, right.0, top_y, right.1) {
                editor.set_block_absolute(IRON_BLOCK, bx, top_y, bz, None, None);
            }
        }
    }
}

fn decorate_suspension(
    editor: &mut WorldEditor,
    path: &[BridgePathSample],
    block_range: i32,
    start_is_boundary: bool,
    end_is_boundary: bool,
) {
    // Skip mid-group ways so we don't hang cables off phantom internal towers.
    if !start_is_boundary || !end_is_boundary {
        return;
    }
    let total = path.len();
    if total < SUSPENSION_MIN_LENGTH {
        return;
    }
    let last_idx = total - 1;
    let inset = (((total as f32) * SUSPENSION_TOWER_INSET_FRAC) as usize).max(2);
    if inset * 2 + 2 > total {
        return;
    }
    let height = suspension_tower_height(total);
    let n_pylons = suspension_pylon_count(total);
    let first = inset;
    let last = last_idx - inset;
    // Evenly distribute pylons between the two boundary insets.
    let pylons: Vec<usize> = (0..n_pylons)
        .map(|i| first + (last - first) * i / (n_pylons - 1).max(1))
        .collect();

    for &p in &pylons {
        let (cx, _, cz, perp) = path[p];
        let (left, right) = side_offsets(cx, cz, perp, block_range);
        let deck_y = path[p].1;
        place_pylon(editor, left.0, left.1, deck_y, height);
        place_pylon(editor, right.0, right.1, deck_y, height);
        place_pylon_crossbeam(editor, left, right, deck_y + height);
    }

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
            editor.set_block_absolute(chain, left.0, cable_y, left.1, None, None);
            editor.set_block_absolute(chain, right.0, cable_y, right.1, None, None);

            let on_hanger_step = (tds - a).is_multiple_of(SUSPENSION_HANGER_INTERVAL);
            if on_hanger_step && tds != a && tds != b {
                for hy in (cy + 2)..cable_y {
                    editor.set_block_absolute(IRON_BARS, left.0, hy, left.1, None, None);
                    editor.set_block_absolute(IRON_BARS, right.0, hy, right.1, None, None);
                }
            }
        }
    }

    // Anchor cables from end pylons to deck endpoints.
    let first_p = pylons[0];
    let last_p = *pylons.last().unwrap();
    let (cx_f, cy_f, cz_f, perp_f) = path[first_p];
    let (left_f, right_f) = side_offsets(cx_f, cz_f, perp_f, block_range);
    let top_f = cy_f + height;
    let (cx_s, cy_s, cz_s, perp_s) = path[0];
    let (left_s, right_s) = side_offsets(cx_s, cz_s, perp_s, block_range);
    draw_cable(
        editor,
        left_f.0,
        top_f,
        left_f.1,
        left_s.0,
        cy_s + 1,
        left_s.1,
    );
    draw_cable(
        editor,
        right_f.0,
        top_f,
        right_f.1,
        right_s.0,
        cy_s + 1,
        right_s.1,
    );

    let (cx_l, cy_l, cz_l, perp_l) = path[last_p];
    let (left_l, right_l) = side_offsets(cx_l, cz_l, perp_l, block_range);
    let top_l = cy_l + height;
    let (cx_e, cy_e, cz_e, perp_e) = path[last_idx];
    let (left_e, right_e) = side_offsets(cx_e, cz_e, perp_e, block_range);
    draw_cable(
        editor,
        left_l.0,
        top_l,
        left_l.1,
        left_e.0,
        cy_e + 1,
        left_e.1,
    );
    draw_cable(
        editor,
        right_l.0,
        top_l,
        right_l.1,
        right_e.0,
        cy_e + 1,
        right_e.1,
    );
}

fn decorate_cable_stayed(editor: &mut WorldEditor, path: &[BridgePathSample], block_range: i32) {
    let total = path.len();
    if total < CABLE_STAYED_MIN_LENGTH {
        return;
    }
    let last_idx = total - 1;
    let height = cable_stayed_tower_height(total);
    // Twin pylons split the deck in half so cables don't cross between them.
    let pylons: Vec<usize> = if total >= CABLE_STAYED_TWIN_PYLON_LENGTH {
        vec![total / 3, (2 * total) / 3]
    } else {
        vec![total / 2]
    };
    let split = total / 2;

    for (idx, &t_tds) in pylons.iter().enumerate() {
        let (cx_t, cy_t, cz_t, perp_t) = path[t_tds];
        let (left_t, right_t) = side_offsets(cx_t, cz_t, perp_t, block_range);
        let top_y = cy_t + height;
        place_pylon(editor, left_t.0, left_t.1, cy_t, height);
        place_pylon(editor, right_t.0, right_t.1, cy_t, height);
        place_pylon_crossbeam(editor, left_t, right_t, top_y);

        let (anchor_lo, anchor_hi) = if pylons.len() == 1 {
            (0usize, total)
        } else if idx == 0 {
            (0usize, split)
        } else {
            (split, total)
        };

        let mut tds = anchor_lo + CABLE_STAYED_ANCHOR_INTERVAL;
        while tds < anchor_hi {
            let gap = tds.abs_diff(t_tds);
            if gap < CABLE_STAYED_MIN_GAP || tds == 0 || tds == last_idx {
                tds += CABLE_STAYED_ANCHOR_INTERVAL;
                continue;
            }
            let (cx_a, cy_a, cz_a, perp_a) = path[tds];
            let (left_a, right_a) = side_offsets(cx_a, cz_a, perp_a, block_range);
            draw_cable(
                editor,
                left_t.0,
                top_y,
                left_t.1,
                left_a.0,
                cy_a + 1,
                left_a.1,
            );
            draw_cable(
                editor,
                right_t.0,
                top_y,
                right_t.1,
                right_a.0,
                cy_a + 1,
                right_a.1,
            );
            tds += CABLE_STAYED_ANCHOR_INTERVAL;
        }
    }
}

fn decorate_covered(
    editor: &mut WorldEditor,
    path: &[BridgePathSample],
    block_range: i32,
    start_is_boundary: bool,
    end_is_boundary: bool,
) {
    let total = path.len();
    if total < 4 {
        return;
    }
    let last = total - 1;
    for (tds, &(cx, cy, cz, perp)) in path.iter().enumerate() {
        if start_is_boundary && tds < COVERED_END_CLEAR {
            continue;
        }
        if end_is_boundary && tds + COVERED_END_CLEAR > last {
            continue;
        }
        let (left, right) = side_offsets(cx, cz, perp, block_range);
        for h in 1..=COVERED_WALL_HEIGHT {
            let block = if h == 2 && tds % COVERED_WINDOW_INTERVAL == 0 {
                GLASS
            } else {
                DARK_OAK_PLANKS
            };
            editor.set_block_absolute(block, left.0, cy + h, left.1, None, None);
            editor.set_block_absolute(block, right.0, cy + h, right.1, None, None);
        }
        // Roof spans deck width plus walls.
        let roof_y = cy + COVERED_WALL_HEIGHT + 1;
        let extent = block_range + 1;
        for offset in -extent..=extent {
            let rx = (cx as f32 + perp.0 * offset as f32).round() as i32;
            let rz = (cz as f32 + perp.1 * offset as f32).round() as i32;
            editor.set_block_absolute(DARK_OAK_PLANKS, rx, roof_y, rz, None, None);
        }
    }
}

fn place_pylon(editor: &mut WorldEditor, x: i32, z: i32, deck_y: i32, height: i32) {
    // 1x1 column avoids road-edge overlap on bridges with negative perp components.
    let ground_y = editor.get_ground_level(x, z);
    let base_y = ground_y.min(deck_y);
    let top_y = deck_y + height;
    for y in (base_y + 1)..=top_y {
        editor.set_block_absolute(SMOOTH_STONE, x, y, z, None, None);
    }
}

fn place_pylon_crossbeam(
    editor: &mut WorldEditor,
    left: (i32, i32),
    right: (i32, i32),
    top_y: i32,
) {
    for (cx, cy, cz) in bresenham_line(left.0, top_y, left.1, right.0, top_y, right.1) {
        editor.set_block_absolute(SMOOTH_STONE, cx, cy, cz, None, None);
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
