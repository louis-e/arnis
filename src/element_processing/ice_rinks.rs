//! Ice rinks: rounded corners, dasher boards with glass above, and hockey lines where played.

use crate::block_definitions::*;
use crate::element_processing::area_layout::{AreaMask, Frame};
use crate::element_processing::connected_blocks::connected_fence;
use crate::floodfill_cache::{in_hole, Holes};
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;

fn plays_hockey(way: &ProcessedWay) -> bool {
    way.tags
        .get("sport")
        .is_some_and(|s| s.split(';').any(|v| v.trim() == "ice_hockey"))
}

/// The rink inside its outline: a rectangle's corners are rounded off, as a rink's are.
struct Rink {
    frame: Frame,
    mask: AreaMask,
    corner: f64,
}

impl Rink {
    fn contains(&self, x: i32, z: i32) -> bool {
        if !self.mask.contains(x, z) {
            return false;
        }
        let (a, c) = self.frame.local(x, z);
        let (l, w, r) = (self.frame.length(), self.frame.width(), self.corner);
        let da = (r - a).max(a - (l - r)).max(0.0);
        let dc = (r - c).max(c - (w - r)).max(0.0);
        da.hypot(dc) <= r
    }
}

pub fn generate_ice_rink(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    fill: &[(i32, i32)],
    holes: Option<&Holes>,
) {
    let Some(frame) = Frame::of(&way.nodes) else {
        return;
    };
    let s = editor.scale().clamp(0.1, 4.0);
    // An outline drawn as a plain rectangle gets the 8.5 m corners of a hockey rink.
    let boxy = way.nodes.len() <= 6;
    let corner = if boxy {
        (8.5 * s).min(frame.width() / 3.0)
    } else {
        0.0
    };
    let rink = Rink {
        frame,
        mask: AreaMask::new(fill, |x, z| !in_hole(holes, x, z)),
        corner,
    };
    let (l, w) = (rink.frame.length(), rink.frame.width());
    // A mapped surface wins: roller rinks and rinks mapped out of season.
    let surface = way
        .tags
        .get("surface")
        .and_then(|s| super::surfaces::get_blocks_for_surface(s))
        .map_or(PACKED_ICE, |b| b[0]);
    // Lines only on a rink-shaped outline; a free-form one is a public skating rink.
    let cells = fill.iter().filter(|&&(x, z)| !in_hole(holes, x, z)).count() as f64;
    let rectangular = cells >= 0.85 * l * w;
    let hockey = plays_hockey(way) && rectangular && l >= 30.0 * s && w >= 15.0 * s;

    for &(x, z) in fill {
        if in_hole(holes, x, z) || !editor.in_bounds(x, z) || !rink.contains(x, z) {
            continue;
        }
        let board = [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .iter()
            .any(|(dx, dz)| !rink.contains(x + dx, z + dz));
        let (a, c) = rink.frame.local(x, z);
        let line = if hockey && !board {
            marking(a, c, l, w, s)
        } else {
            None
        };
        editor.set_block(line.unwrap_or(surface), x, 0, z, Some(&[GRASS_BLOCK]), None);
        if !editor.owns(x, z) {
            continue;
        }
        if board {
            editor.set_block(WHITE_CONCRETE, x, 1, z, None, None);
            if hockey {
                let board_at = |nx: i32, nz: i32| {
                    rink.contains(nx, nz)
                        && [(1, 0), (-1, 0), (0, 1), (0, -1)]
                            .iter()
                            .any(|(dx, dz)| !rink.contains(nx + dx, nz + dz))
                };
                let pane = connected_fence(
                    GLASS_PANE,
                    board_at(x, z - 1),
                    board_at(x, z + 1),
                    board_at(x + 1, z),
                    board_at(x - 1, z),
                );
                let y = editor.get_absolute_y(x, 2, z);
                editor.set_block_with_properties_absolute(pane, x, y, z, None, None);
            }
        } else if hockey {
            goal(editor, x, z, a, c, l, w, s);
        }
    }
}

/// The painted line at (a, c) on a rink of length `l` and width `w`, if any.
fn marking(a: f64, c: f64, l: f64, w: f64, s: f64) -> Option<Block> {
    let near = |v: f64, at: f64| (v - at).abs() < 0.5;
    let goal_line = 0.055 * l;
    let (mid_a, mid_c) = (l / 2.0, w / 2.0);
    let radius = (4.57 * s).min(w * 0.2).min(0.08 * l);
    let ring = |ca: f64, cc: f64| {
        let d = (a - ca).hypot(c - cc);
        (d - radius).abs() < 0.5 || d < 0.6
    };
    if near(a, mid_a) || ring(mid_a, mid_c) {
        return Some(if near(a, mid_a) && !ring(mid_a, mid_c) {
            RED_CONCRETE
        } else {
            BLUE_CONCRETE
        });
    }
    if near(a, 0.375 * l) || near(a, 0.625 * l) {
        return Some(BLUE_CONCRETE);
    }
    for end in [goal_line, l - goal_line] {
        // The crease, a half disc in front of the goal.
        let inward = if end < mid_a { a - end } else { end - a };
        if (0.0..(1.83 * s).max(1.5)).contains(&inward)
            && (a - end).hypot(c - mid_c) < (1.83 * s).max(1.5)
        {
            return Some(LIGHT_BLUE_CONCRETE);
        }
        if near(a, end) {
            return Some(RED_CONCRETE);
        }
        // End-zone faceoff circles.
        let fa = if end < mid_a {
            end + 6.1 * s
        } else {
            end - 6.1 * s
        };
        for fc in [mid_c - 6.7 * s, mid_c + 6.7 * s] {
            if ring(fa, fc) {
                return Some(RED_CONCRETE);
            }
        }
    }
    None
}

/// A goal on the goal line, its net behind it.
#[allow(clippy::too_many_arguments)]
fn goal(editor: &mut WorldEditor, x: i32, z: i32, a: f64, c: f64, l: f64, w: f64, s: f64) {
    let goal_line = 0.055 * l;
    let half = (0.9 * s).max(1.0);
    if (c - w / 2.0).abs() > half + 0.5 {
        return;
    }
    for end in [goal_line, l - goal_line] {
        let behind = if end < l / 2.0 { end - a } else { a - end };
        if (0.5..1.5).contains(&behind) {
            let post = (c - w / 2.0).abs() > half - 0.5;
            editor.set_block(
                if post { NETHER_BRICK_FENCE } else { WHITE_WOOL },
                x,
                1,
                z,
                None,
                None,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rink_has_its_lines_where_hockey_puts_them() {
        let (l, w) = (61.0, 26.0);
        assert_eq!(
            marking(30.5, 2.0, l, w, 1.0),
            Some(RED_CONCRETE),
            "centre line"
        );
        assert_eq!(
            marking(22.9, 5.0, l, w, 1.0),
            Some(BLUE_CONCRETE),
            "blue line"
        );
        assert_eq!(
            marking(3.36, 2.0, l, w, 1.0),
            Some(RED_CONCRETE),
            "goal line"
        );
        assert_eq!(
            marking(4.0, 13.0, l, w, 1.0),
            Some(LIGHT_BLUE_CONCRETE),
            "crease"
        );
        assert_eq!(marking(12.0, 20.0, l, w, 1.0), None, "open ice");
    }
}

#[cfg(test)]
mod rink_tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::*;

    #[test]
    fn a_rectangular_hockey_rink_gets_round_corners_boards_and_lines() {
        let xzbbox = XZBBox::rect_from_xz_lengths(70.0, 35.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let way = rect_way(
            51,
            2,
            2,
            63,
            28,
            &[("leisure", "ice_rink"), ("sport", "ice_hockey")],
        );
        let fill = crate::floodfill::flood_fill_area(
            &way.nodes.iter().map(|n| (n.x, n.z)).collect::<Vec<_>>(),
            None,
        );
        generate_ice_rink(&mut editor, &way, &fill, None);
        assert!(
            !editor.check_for_block(3, 0, 3, Some(&[PACKED_ICE])),
            "corner cut"
        );
        // The first row of the rink is its board, with glass on top.
        let edge = (2..10)
            .find(|&z| editor.check_for_block(32, 0, z, Some(&[PACKED_ICE, RED_CONCRETE])))
            .expect("ice");
        assert!(
            editor.check_for_block(32, 1, edge, Some(&[WHITE_CONCRETE])),
            "board"
        );
        assert!(
            editor.check_for_block(32, 2, edge, Some(&[GLASS_PANE])),
            "glass"
        );
        assert!(!editor.block_exists_absolute(
            32,
            editor.get_absolute_y(32, 1, edge + 2),
            edge + 2
        ));
        assert!(
            count_blocks(&editor, 3..63, 3..28, 0, &[RED_CONCRETE]) > 20,
            "centre and goal lines"
        );
        assert!(count_blocks(&editor, 3..63, 3..28, 0, &[PACKED_ICE]) > 1000);
    }
}
