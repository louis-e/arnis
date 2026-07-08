use crate::block_definitions::*;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;
use std::collections::HashSet;

enum PitchKind {
    Soccer,
    Basketball,
    Tennis,
    Generic,
}

// First recognized sport wins; unknown/untagged pitches get no markings.
fn pitch_kind(way: &ProcessedWay) -> Option<PitchKind> {
    let sport = way.tags.get("sport")?;
    for s in sport.split(';') {
        match s.trim() {
            "soccer" | "football" => return Some(PitchKind::Soccer),
            "basketball" => return Some(PitchKind::Basketball),
            "tennis" | "paddle_tennis" | "padel" => return Some(PitchKind::Tennis),
            "handball" | "futsal" | "hockey" | "field_hockey" | "ice_hockey" | "volleyball"
            | "beachvolleyball" | "badminton" | "netball" | "korfball" | "team_handball"
            | "multi" => return Some(PitchKind::Generic),
            _ => {}
        }
    }
    None
}

// Unit direction of the way's longest edge, or None for degenerate geometry.
fn longest_edge_dir(way: &ProcessedWay) -> Option<(f64, f64)> {
    let mut best: Option<((f64, f64), f64)> = None;
    for w in way.nodes.windows(2) {
        let dx = (w[1].x - w[0].x) as f64;
        let dz = (w[1].z - w[0].z) as f64;
        let len2 = dx * dx + dz * dz;
        if len2 > best.map(|(_, l)| l).unwrap_or(0.0) {
            best = Some(((dx, dz), len2));
        }
    }
    let ((dx, dz), len2) = best?;
    if len2 < 1.0 {
        return None;
    }
    let len = len2.sqrt();
    Some((dx / len, dz / len))
}

pub fn draw_pitch_markings(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    filled_area: &[(i32, i32)],
    surface: Block,
) {
    let Some(kind) = pitch_kind(way) else {
        return;
    };
    if way.tags.get("indoor").map(String::as_str) == Some("yes") {
        return;
    }
    if filled_area.len() < 40 {
        return;
    }
    let Some((mut ux, mut uz)) = longest_edge_dir(way) else {
        return;
    };

    let n = filled_area.len() as f64;
    let cx = filled_area.iter().map(|&(x, _)| x as f64).sum::<f64>() / n;
    let cz = filled_area.iter().map(|&(_, z)| z as f64).sum::<f64>() / n;

    // Local frame: u along the long axis, v perpendicular.
    let (mut vx, mut vz) = (-uz, ux);

    let (mut u_min, mut u_max, mut v_min, mut v_max) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for &(x, z) in filled_area {
        let dx = x as f64 - cx;
        let dz = z as f64 - cz;
        let (u, v) = (dx * ux + dz * uz, dx * vx + dz * vz);
        u_min = u_min.min(u);
        u_max = u_max.max(u);
        v_min = v_min.min(v);
        v_max = v_max.max(v);
    }
    let mut a = (u_max - u_min) / 2.0;
    let mut b = (v_max - v_min) / 2.0;
    if b > a {
        std::mem::swap(&mut a, &mut b);
        std::mem::swap(&mut ux, &mut vx);
        std::mem::swap(&mut uz, &mut vz);
    }
    if a < 6.0 || b < 4.0 {
        return;
    }
    // Recenter on the extent midpoint so markings stay symmetric on offset shapes.
    let (u_mid, v_mid) = ((u_min + u_max) / 2.0, (v_min + v_max) / 2.0);

    // Defined after the axis swap so the captured frame stays immutable
    let uv = |x: i32, z: i32| -> (f64, f64) {
        let dx = x as f64 - cx;
        let dz = z as f64 - cz;
        (dx * ux + dz * uz, dx * vx + dz * vz)
    };

    let cells: HashSet<(i32, i32)> = filled_area.iter().copied().collect();
    let line = |d: f64| d.abs() <= 0.6;

    for &(x, z) in filled_area {
        let (mut u, mut v) = uv(x, z);
        u -= u_mid;
        v -= v_mid;

        let boundary = !(cells.contains(&(x + 1, z))
            && cells.contains(&(x - 1, z))
            && cells.contains(&(x, z + 1))
            && cells.contains(&(x, z - 1)));

        let marked = boundary
            || match kind {
                PitchKind::Soccer => {
                    let r = (u * u + v * v).sqrt();
                    let circle_r = (b * 0.3).clamp(3.0, 9.0);
                    let box_depth = (a * 0.22).min(12.0);
                    let box_hw = (b * 0.65).min(15.0);
                    line(u)
                        || line(r - circle_r)
                        || (line(u.abs() - (a - box_depth)) && v.abs() <= box_hw)
                        || (line(v.abs() - box_hw) && u.abs() >= a - box_depth)
                }
                PitchKind::Basketball => {
                    let r = (u * u + v * v).sqrt();
                    let circle_r = (b * 0.25).clamp(2.0, 5.0);
                    let key_depth = (a * 0.28).min(9.0);
                    let key_hw = (b * 0.35).min(4.0);
                    let ft_u = a - key_depth;
                    let du = u.abs() - ft_u;
                    let ft_r = ((du * du + v * v).sqrt() - circle_r).abs();
                    line(u)
                        || line(r - circle_r)
                        || (line(du) && v.abs() <= key_hw)
                        || (line(v.abs() - key_hw) && u.abs() >= ft_u)
                        || (ft_r <= 0.6 && u.abs() <= ft_u)
                }
                PitchKind::Tennis => {
                    let singles = b * 0.75;
                    let service_u = a * 0.54;
                    line(u)
                        || line(v.abs() - singles)
                        || (line(u.abs() - service_u) && v.abs() <= singles)
                        || (line(v) && u.abs() <= service_u)
                }
                PitchKind::Generic => {
                    let r = (u * u + v * v).sqrt();
                    line(u) || line(r - (b * 0.3).clamp(2.0, 8.0))
                }
            };

        if marked {
            editor.set_block(WHITE_CONCRETE, x, 0, z, Some(&[surface]), None);
        }
    }

    // Simple goals / net above the surface.
    let place_uv = |editor: &mut WorldEditor, u: f64, v: f64, y: i32, block: Block| {
        let x = (cx + (u + u_mid) * ux + (v + v_mid) * vx).round() as i32;
        let z = (cz + (u + u_mid) * uz + (v + v_mid) * vz).round() as i32;
        if cells.contains(&(x, z)) {
            editor.set_block(block, x, y, z, None, None);
        }
    };
    match kind {
        PitchKind::Soccer if a >= 10.0 && b >= 5.0 => {
            for end in [-1.0, 1.0] {
                for dv in -2i32..=2 {
                    place_uv(editor, end * (a - 0.5), dv as f64, 2, IRON_BARS);
                    if dv.abs() == 2 {
                        place_uv(editor, end * (a - 0.5), dv as f64, 1, IRON_BARS);
                    }
                }
            }
        }
        PitchKind::Tennis if b >= 4.0 => {
            let net_hw = (b * 0.8) as i32;
            for dv in -net_hw..=net_hw {
                place_uv(editor, 0.0, dv as f64, 1, IRON_BARS);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::osm_parser::ProcessedNode;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn test_editor(xzbbox: &XZBBox) -> WorldEditor<'_> {
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        WorldEditor::new(PathBuf::from("/dev/null/unused"), xzbbox, llbbox)
    }

    fn rect_pitch(x0: i32, z0: i32, x1: i32, z1: i32, sport: Option<&str>) -> ProcessedWay {
        let mut tags = HashMap::new();
        tags.insert("leisure".to_string(), "pitch".to_string());
        if let Some(s) = sport {
            tags.insert("sport".to_string(), s.to_string());
        }
        let corners = [(x0, z0), (x1, z0), (x1, z1), (x0, z1), (x0, z0)];
        ProcessedWay {
            id: 1,
            nodes: corners
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: i as u64,
                    tags: HashMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags,
        }
    }

    fn rect_fill(x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<(i32, i32)> {
        let mut v = Vec::new();
        for x in x0..=x1 {
            for z in z0..=z1 {
                v.push((x, z));
            }
        }
        v
    }

    #[test]
    fn soccer_pitch_gets_lines_and_goals() {
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let way = rect_pitch(10, 10, 49, 29, Some("soccer"));
        let fill = rect_fill(10, 10, 49, 29);
        draw_pitch_markings(&mut editor, &way, &fill, GREEN_STAINED_HARDENED_CLAY);

        assert!(
            editor.check_for_block(29, 0, 15, Some(&[WHITE_CONCRETE])),
            "halfway line"
        );
        assert!(
            editor.check_for_block(10, 0, 10, Some(&[WHITE_CONCRETE])),
            "perimeter line"
        );
        assert!(
            !editor.check_for_block(20, 0, 19, Some(&[WHITE_CONCRETE])),
            "open play area stays unmarked"
        );
        assert!(
            editor.check_for_block(49, 2, 20, Some(&[IRON_BARS])),
            "goal crossbar"
        );
    }

    #[test]
    fn tiny_pitch_gets_no_markings() {
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let way = rect_pitch(10, 10, 15, 13, Some("soccer"));
        let fill = rect_fill(10, 10, 15, 13);
        draw_pitch_markings(&mut editor, &way, &fill, GREEN_STAINED_HARDENED_CLAY);

        for &(x, z) in &fill {
            assert!(!editor.check_for_block(x, 0, z, Some(&[WHITE_CONCRETE])));
        }
    }

    #[test]
    fn untagged_and_unknown_sports_get_no_markings() {
        let xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let fill = rect_fill(10, 10, 49, 29);
        for sport in [None, Some("golf"), Some("equestrian")] {
            let mut editor = test_editor(&xzbbox);
            let way = rect_pitch(10, 10, 49, 29, sport);
            draw_pitch_markings(&mut editor, &way, &fill, GREEN_STAINED_HARDENED_CLAY);
            assert!(!editor.check_for_block(29, 0, 15, Some(&[WHITE_CONCRETE])));
        }
    }

    #[test]
    fn rotated_tennis_court_gets_clipped_markings() {
        let xzbbox = XZBBox::rect_from_xz_lengths(120.0, 120.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        // 45-degree court: cells within |u|<=15, |v|<=7 around (50,50).
        let inv = 1.0 / 2.0_f64.sqrt();
        let mut fill = Vec::new();
        for x in 20..80 {
            for z in 20..80 {
                let (dx, dz) = ((x - 50) as f64, (z - 50) as f64);
                let (u, v) = (dx * inv + dz * inv, -dx * inv + dz * inv);
                if u.abs() <= 15.0 && v.abs() <= 7.0 {
                    fill.push((x, z));
                }
            }
        }
        let mut way = rect_pitch(0, 0, 0, 0, Some("tennis"));
        way.nodes = [(39, 39), (61, 61), (66, 56), (44, 34), (39, 39)]
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64,
                tags: HashMap::new(),
                x,
                z,
            })
            .collect();
        draw_pitch_markings(&mut editor, &way, &fill, GREEN_STAINED_HARDENED_CLAY);

        // Net line crosses the center regardless of rotation.
        assert!(
            editor.check_for_block(50, 0, 50, Some(&[WHITE_CONCRETE])),
            "center net line"
        );
        // Every marking stays inside the filled shape.
        let cells: HashSet<(i32, i32)> = fill.iter().copied().collect();
        for x in 20..80 {
            for z in 20..80 {
                if !cells.contains(&(x, z)) {
                    assert!(!editor.check_for_block(x, 0, z, Some(&[WHITE_CONCRETE])));
                }
            }
        }
    }
}
