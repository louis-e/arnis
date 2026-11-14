use crate::block_definitions::*;
use crate::land_cover::coord_hash;
use crate::osm_parser::ProcessedWay;
use crate::world_editor::WorldEditor;
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PitchKind {
    Soccer,
    AmericanFootball,
    Rugby,
    Baseball,
    /// Australian rules and Gaelic football on a grass oval.
    Oval,
    Cricket,
    Basketball,
    Tennis,
    Generic,
}

impl PitchKind {
    /// Played on a long lawn that groundsmen mow in stripes.
    fn mown(self) -> bool {
        matches!(
            self,
            PitchKind::Soccer
                | PitchKind::AmericanFootball
                | PitchKind::Rugby
                | PitchKind::Baseball
                | PitchKind::Oval
                | PitchKind::Cricket
        )
    }
}

// First recognized sport wins; unknown/untagged pitches get no markings.
fn pitch_kind(way: &ProcessedWay) -> Option<PitchKind> {
    let sport = way.tags.get("sport")?;
    let codes: Vec<&str> = sport.split(';').map(str::trim).collect();
    // Bare "football" is whichever code the country plays, so a named code beside it wins.
    if codes.contains(&"football") {
        if codes
            .iter()
            .any(|c| matches!(*c, "american_football" | "canadian_football"))
        {
            return Some(PitchKind::AmericanFootball);
        }
        if codes
            .iter()
            .any(|c| matches!(*c, "rugby" | "rugby_union" | "rugby_league"))
        {
            return Some(PitchKind::Rugby);
        }
    }
    for s in codes {
        match s {
            "soccer" | "football" | "association_football" => return Some(PitchKind::Soccer),
            "american_football" | "canadian_football" => return Some(PitchKind::AmericanFootball),
            "rugby" | "rugby_union" | "rugby_league" => return Some(PitchKind::Rugby),
            "baseball" | "softball" => return Some(PitchKind::Baseball),
            "australian_football" | "gaelic_games" | "gaelic_football" | "hurling" => {
                return Some(PitchKind::Oval)
            }
            "cricket" => return Some(PitchKind::Cricket),
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

/// Ground a pitch of this sport is laid with when the map gives no surface.
pub(crate) fn default_surface(tags: &std::collections::HashMap<String, String>) -> Option<Block> {
    let sport = tags.get("sport")?;
    sport.split(';').find_map(|s| match s.trim() {
        "beachvolleyball" | "beach_volleyball" | "beachhandball" | "beach_soccer" => Some(SAND),
        "equestrian" | "horse_racing" => Some(COARSE_DIRT),
        "athletics" | "running" => Some(RED_TERRACOTTA),
        "boules" | "petanque" | "bocce" => Some(GRAVEL),
        "skateboard" => Some(SMOOTH_STONE),
        _ => None,
    })
}

// Unit direction of the way's longest edge, or None for degenerate geometry.
pub(crate) fn longest_edge_dir(way: &ProcessedWay) -> Option<(f64, f64)> {
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

/// A darker band every few blocks across a lawn, as a mower leaves it.
fn stripe_block(surface: Block) -> Option<Block> {
    matches!(surface, GRASS_BLOCK | GREEN_STAINED_HARDENED_CLAY).then_some(MOSS_BLOCK)
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
    if kind == PitchKind::Baseball {
        draw_diamond(editor, way, filled_area, surface);
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
    let stripes = stripe_block(surface).filter(|_| kind.mown() && a >= 20.0);
    let band = (a / 9.0).round().max(3.0);
    // Gridiron end zones take a team colour.
    let end_zone = [BLUE_CONCRETE, RED_CONCRETE, GREEN_CONCRETE]
        [(coord_hash(way.id as i32, (way.id >> 32) as i32) % 3) as usize];

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
                PitchKind::AmericanFootball => {
                    // End zones are 10 of the 120 yards; a line every 5 yards between.
                    let goal = a * 5.0 / 6.0;
                    let step = goal / 10.0;
                    let k = (u.abs() / step).round();
                    u.abs() <= goal + 0.6 && line(u.abs() - k * step)
                }
                PitchKind::Rugby => {
                    // Try lines 50 m out, 22 m lines, dashed 10 m lines.
                    let try_line = a * 5.0 / 6.0;
                    line(u)
                        || line(u.abs() - try_line)
                        || line(u.abs() - try_line * 28.0 / 50.0)
                        || (line(u.abs() - try_line * 10.0 / 50.0)
                            && (v.round() as i32).rem_euclid(4) < 2)
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
                PitchKind::Oval => {
                    // A 50 m centre square, the centre circle and 50 m arcs from the goals.
                    let k = a / 80.0;
                    let r = (u * u + v * v).sqrt();
                    let from_goal = ((a - u.abs()).powi(2) + v * v).sqrt();
                    (line(u.abs() - 25.0 * k) && v.abs() <= 25.0 * k)
                        || (line(v.abs() - 25.0 * k) && u.abs() <= 25.0 * k)
                        || line(r - 5.0 * k)
                        || line(from_goal - 50.0 * k)
                }
                PitchKind::Cricket => {
                    // Popping creases across the strip.
                    let k = (a / 70.0).max(0.3);
                    line(u.abs() - 8.8 * k) && v.abs() <= 2.0 * k.max(1.0)
                }
                PitchKind::Baseball => false,
            };

        let strip = kind == PitchKind::Cricket && {
            let k = (a / 70.0).max(0.3);
            u.abs() <= 10.0 * k && v.abs() <= 1.5 * k.max(1.0)
        };
        let paint = if marked {
            Some(WHITE_CONCRETE)
        } else if strip {
            Some(DIRT_PATH)
        } else if kind == PitchKind::AmericanFootball && u.abs() > a * 5.0 / 6.0 + 0.6 {
            Some(end_zone)
        } else if stripes.is_some() && ((u + a) / band).floor() as i64 % 2 == 1 {
            stripes
        } else {
            None
        };
        if let Some(block) = paint {
            editor.set_block(block, x, 0, z, Some(&[surface]), None);
        }
    }

    // Goals and posts above the surface.
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
        PitchKind::AmericanFootball if a >= 20.0 && b >= 10.0 => {
            // 18.5 ft apart, crossbar at 10 ft, uprights 30 ft above it.
            let half = (b * 0.115).round().max(2.0) as i32;
            let bar = (b * 0.125).round().max(2.0) as i32;
            let top = bar + (b * 0.37).round().max(4.0) as i32;
            for end in [-1.0, 1.0] {
                let u = end * (a - 0.5);
                for y in 1..bar {
                    place_uv(editor, u, 0.0, y, YELLOW_CONCRETE);
                }
                for dv in -half..=half {
                    place_uv(editor, u, dv as f64, bar, YELLOW_CONCRETE);
                }
                for y in bar + 1..=top {
                    place_uv(editor, u, -half as f64, y, YELLOW_CONCRETE);
                    place_uv(editor, u, half as f64, y, YELLOW_CONCRETE);
                }
            }
        }
        PitchKind::Rugby if a >= 20.0 && b >= 10.0 => {
            // 5.6 m apart, crossbar at 3 m, uprights about 16 m tall.
            let half = (b * 0.08).round().max(2.0) as i32;
            let bar = (b * 0.086).round().max(2.0) as i32;
            let top = (b * 0.45).round().max(bar as f64 + 4.0) as i32;
            let try_line = a * 5.0 / 6.0;
            for end in [-1.0, 1.0] {
                let u = end * try_line;
                for dv in -half..=half {
                    place_uv(editor, u, dv as f64, bar, WHITE_CONCRETE);
                }
                for y in 1..=top {
                    place_uv(editor, u, -half as f64, y, WHITE_CONCRETE);
                    place_uv(editor, u, half as f64, y, WHITE_CONCRETE);
                }
            }
        }
        PitchKind::Oval if a >= 30.0 => {
            // Two goal posts 6.4 m apart, a shorter behind post 6.4 m out from each.
            let k = a / 80.0;
            let gap = (3.2 * k).round().max(2.0) as i32;
            let tall = (b * 0.1).round().max(5.0) as i32;
            for end in [-1.0, 1.0] {
                let u = end * (a - 0.5);
                for side in [-1, 1] {
                    for y in 1..=tall {
                        place_uv(editor, u, (side * gap) as f64, y, WHITE_CONCRETE);
                    }
                    for y in 1..=tall * 2 / 3 {
                        place_uv(editor, u, (side * gap * 3) as f64, y, WHITE_CONCRETE);
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

/// Home plate and the line to centre field: where the two longest straight runs of the
/// outline meet at about a right angle, as the foul lines of a fan-shaped field do.
fn home_plate(way: &ProcessedWay, filled_area: &[(i32, i32)]) -> Option<((f64, f64), (f64, f64))> {
    let pts: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
    let hull = hull_ccw(&pts);
    if hull.len() < 3 {
        return None;
    }
    // Runs of hull edges that keep within a few degrees of one heading.
    // (heading, start, length)
    type Run = ((f64, f64), (f64, f64), f64);
    let mut runs: Vec<Run> = Vec::new();
    for i in 0..hull.len() {
        let (p, q) = (hull[i], hull[(i + 1) % hull.len()]);
        let len = (q.0 - p.0).hypot(q.1 - p.1);
        if len < 1e-6 {
            continue;
        }
        let dir = ((q.0 - p.0) / len, (q.1 - p.1) / len);
        if let Some(last) = runs.last_mut() {
            let (d0, _, l0) = *last;
            if d0.0 * dir.0 + d0.1 * dir.1 > 0.985 {
                let total = l0 + len;
                let blended = (
                    (d0.0 * l0 + dir.0 * len) / total,
                    (d0.1 * l0 + dir.1 * len) / total,
                );
                let norm = blended.0.hypot(blended.1);
                *last = ((blended.0 / norm, blended.1 / norm), last.1, total);
                continue;
            }
        }
        runs.push((dir, p, len));
    }
    // The hull starts at its leftmost point, which can fall midway along a foul line.
    if runs.len() > 2 {
        let (d0, _, l0) = runs[0];
        let (dl, pl, ll) = runs[runs.len() - 1];
        if d0.0 * dl.0 + d0.1 * dl.1 > 0.985 {
            let total = l0 + ll;
            let blended = (
                (d0.0 * l0 + dl.0 * ll) / total,
                (d0.1 * l0 + dl.1 * ll) / total,
            );
            let norm = blended.0.hypot(blended.1);
            runs[0] = ((blended.0 / norm, blended.1 / norm), pl, total);
            runs.pop();
        }
    }
    let n = filled_area.len() as f64;
    let centroid = (
        filled_area.iter().map(|&(x, _)| x as f64).sum::<f64>() / n,
        filled_area.iter().map(|&(_, z)| z as f64).sum::<f64>() / n,
    );
    let longest = runs.iter().map(|r| r.2).fold(0.0, f64::max);
    let mut best: Option<(f64, (f64, f64))> = None;
    for i in 0..runs.len() {
        for step in 1..=2 {
            let j = (i + step) % runs.len();
            if i == j {
                continue;
            }
            let (da, pa, la) = runs[i];
            let (db, pb, lb) = runs[j];
            if la < longest * 0.4 || lb < longest * 0.4 {
                continue;
            }
            let cos = (da.0 * db.0 + da.1 * db.1).abs();
            if cos > 0.35 {
                continue;
            }
            // Where the two foul lines cross.
            let det = da.0 * db.1 - da.1 * db.0;
            if det.abs() < 1e-9 {
                continue;
            }
            let t = ((pb.0 - pa.0) * db.1 - (pb.1 - pa.1) * db.0) / det;
            let apex = (pa.0 + da.0 * t, pa.1 + da.1 * t);
            let score = la.min(lb);
            if best.is_none_or(|(s, _)| score > s) {
                best = Some((score, apex));
            }
        }
    }
    let apex = match best {
        Some((_, apex)) => apex,
        // No fan: a ball field in a corner of a multi-use lawn.
        None => {
            let corner = hull[(way.id as usize) % hull.len()];
            (corner.0, corner.1)
        }
    };
    let (dx, dz) = (centroid.0 - apex.0, centroid.1 - apex.1);
    let len = dx.hypot(dz);
    if len < 5.0 {
        return None;
    }
    Some((apex, (dx / len, dz / len)))
}

/// Convex hull, counter-clockwise without the closing point (Andrew's monotone chain).
pub(crate) fn hull_ccw(points: &[(i32, i32)]) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = points.iter().map(|&(x, z)| (x as f64, z as f64)).collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut hull: Vec<(f64, f64)> = Vec::with_capacity(pts.len() + 1);
    for &p in &pts {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    // The upper chain may not pop back into the finished lower one.
    let lower = hull.len() + 1;
    for &p in pts.iter().rev().skip(1) {
        while hull.len() >= lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    hull.pop();
    hull
}

/// A ball diamond: clay infield and base paths round a grass square, mound, bases,
/// foul lines, a warning track and, on big fields, the outfield wall.
fn draw_diamond(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    filled_area: &[(i32, i32)],
    surface: Block,
) {
    let Some(((hx, hz), (ux, uz))) = home_plate(way, filled_area) else {
        return;
    };
    let (wx, wz) = (-uz, ux);
    let cells: HashSet<(i32, i32)> = filled_area.iter().copied().collect();
    let reach = filled_area
        .iter()
        .map(|&(x, z)| (x as f64 - hx).hypot(z as f64 - hz))
        .fold(0.0, f64::max);
    // Sizes in metres, measured on the world's scale.
    let scale = editor.scale();
    if reach < 30.0 * scale {
        return;
    }
    // 90 ft bases on a 400 ft park, 60 ft on a little league or softball field.
    let base = (reach * 0.27).clamp(12.0 * scale, 27.4 * scale);
    let s2 = std::f64::consts::FRAC_1_SQRT_2;
    let mound = base * 0.672;
    let clay = TERRACOTTA;
    let stripes = stripe_block(surface);
    let band = (base / 5.0).round().max(3.0);
    let near_edge = |x: i32, z: i32, r: i32| {
        (-r..=r).any(|dz| {
            (-r..=r).any(|dx| dx * dx + dz * dz <= r * r && !cells.contains(&(x + dx, z + dz)))
        })
    };
    let track = (base * 0.16).round().max(2.0) as i32;

    for &(x, z) in filled_area {
        let (px, pz) = (x as f64 - hx, z as f64 - hz);
        let along = px * ux + pz * uz;
        let across = px * wx + pz * wz;
        // Diamond square: s toward first base, t toward third.
        let s = (along - across) * s2;
        let t = (along + across) * s2;
        let to_mound = (along - mound).hypot(across);
        let home_r = along.hypot(across);
        let fair = s >= -0.5 && t >= -0.5;

        let foul_line = fair && (s.abs() <= 0.5 || t.abs() <= 0.5);
        let inner_grass = s > 1.5 && t > 1.5 && s < base - 1.5 && t < base - 1.5;
        let skin = fair && to_mound <= base * 1.055 && along > 0.0;
        let path = (s.abs() <= 1.5 || t.abs() <= 1.5)
            && s <= base + 1.5
            && t <= base + 1.5
            && s > -2.0
            && t > -2.0;
        let dirt =
            home_r <= base * 0.145 || to_mound <= base * 0.1 || (skin && !inner_grass) || path;
        let warning = fair && home_r > base * 1.6 && near_edge(x, z, track);

        let block = if foul_line && home_r > base * 0.145 {
            Some(WHITE_CONCRETE)
        } else if dirt || warning {
            Some(clay)
        } else if stripes.is_some() && (along / band).floor() as i64 % 2 == 1 {
            stripes
        } else {
            None
        };
        if let Some(b) = block {
            editor.set_block(b, x, 0, z, Some(&[surface]), None);
        }
    }

    let at = |a: f64, c: f64| {
        (
            (hx + a * ux + c * wx).round() as i32,
            (hz + a * uz + c * wz).round() as i32,
        )
    };
    let bases = [
        at(0.0, 0.0),
        at(base * s2, -base * s2),
        at(base * 2.0 * s2, 0.0),
        at(base * s2, base * s2),
        at(mound, 0.0),
    ];
    for (x, z) in bases {
        if cells.contains(&(x, z)) {
            editor.set_block(WHITE_CONCRETE, x, 0, z, None, Some(&[]));
        }
    }

    // A padded wall round the outfield of a full-size park.
    if reach >= 85.0 * scale {
        for &(x, z) in filled_area {
            let (px, pz) = (x as f64 - hx, z as f64 - hz);
            let along = px * ux + pz * uz;
            let across = px * wx + pz * wz;
            let fair = along - across.abs() >= -0.5;
            if fair && along.hypot(across) > base * 2.0 && near_edge(x, z, 1) {
                for y in 1..=3 {
                    editor.set_block(GREEN_CONCRETE, x, y, z, None, None);
                }
            }
        }
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

    #[test]
    fn gridiron_gets_yard_lines_end_zones_and_goalposts() {
        let xzbbox = XZBBox::rect_from_xz_lengths(160.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        // 120 x 54: 10 blocks of end zone at each end.
        let way = rect_pitch(10, 10, 129, 63, Some("american_football"));
        let fill = rect_fill(10, 10, 129, 63);
        draw_pitch_markings(&mut editor, &way, &fill, GRASS_BLOCK);

        let midfield = 69;
        assert!(editor.check_for_block(midfield, 0, 30, Some(&[WHITE_CONCRETE])));
        let end_zone = [BLUE_CONCRETE, RED_CONCRETE, GREEN_CONCRETE];
        assert!(
            editor.check_for_block(14, 0, 30, Some(&end_zone)),
            "end zone"
        );
        // Centred on z 36.5, so the crossbar spans z 34 to 40 with an upright at each end.
        assert!(
            editor.check_for_block(129, 3, 37, Some(&[YELLOW_CONCRETE])),
            "goalpost crossbar"
        );
        assert!(editor.check_for_block(129, 10, 34, Some(&[YELLOW_CONCRETE])));
        assert!(!editor.check_for_block(129, 10, 37, Some(&[YELLOW_CONCRETE])));
    }

    #[test]
    fn bare_football_beside_a_named_code_follows_the_code() {
        let way = rect_pitch(0, 0, 10, 10, Some("football;american_football"));
        assert_eq!(pitch_kind(&way), Some(PitchKind::AmericanFootball));
        let way = rect_pitch(0, 0, 10, 10, Some("football;tennis"));
        assert_eq!(pitch_kind(&way), Some(PitchKind::Soccer));
    }

    #[test]
    fn rugby_posts_stand_on_the_try_lines() {
        let xzbbox = XZBBox::rect_from_xz_lengths(160.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let way = rect_pitch(10, 10, 129, 79, Some("rugby_union"));
        let fill = rect_fill(10, 10, 129, 79);
        draw_pitch_markings(&mut editor, &way, &fill, GRASS_BLOCK);
        // Try line 50 of 60 half-lengths out from the middle.
        let found = (115..=121)
            .any(|x| (40..=50).any(|z| editor.check_for_block(x, 12, z, Some(&[WHITE_CONCRETE]))));
        assert!(found, "an upright above the crossbar");
    }

    fn fan(r: f64) -> (ProcessedWay, Vec<(i32, i32)>) {
        // Home plate at (10, 10), foul lines along +x and +z, outfield arc of radius r.
        let mut pts = vec![(10, 10)];
        for i in 0..=12 {
            let a = std::f64::consts::FRAC_PI_2 * i as f64 / 12.0;
            pts.push((10 + (r * a.cos()) as i32, 10 + (r * a.sin()) as i32));
        }
        pts.push((10, 10));
        let mut way = rect_pitch(0, 0, 0, 0, Some("baseball"));
        way.nodes = pts
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64,
                tags: HashMap::new(),
                x,
                z,
            })
            .collect();
        let mut fill = Vec::new();
        for x in 10..=10 + r as i32 {
            for z in 10..=10 + r as i32 {
                let (dx, dz) = ((x - 10) as f64, (z - 10) as f64);
                if dx.hypot(dz) <= r {
                    fill.push((x, z));
                }
            }
        }
        (way, fill)
    }

    #[test]
    fn diamond_finds_home_plate_at_the_corner_of_the_fan() {
        let (way, fill) = fan(110.0);
        let ((hx, hz), (ux, uz)) = home_plate(&way, &fill).expect("a home plate");
        assert!(
            (hx - 10.0).abs() < 2.0 && (hz - 10.0).abs() < 2.0,
            "{hx},{hz}"
        );
        let s2 = std::f64::consts::FRAC_1_SQRT_2;
        assert!(
            (ux - s2).abs() < 0.15 && (uz - s2).abs() < 0.15,
            "{ux},{uz}"
        );
    }

    #[test]
    fn home_plate_joins_a_foul_line_split_where_the_hull_starts() {
        // The left foul line bends out at (9, 60), the hull's first point, halving it.
        let mut pts = vec![(10, 10), (160, 10)];
        for i in 1..12 {
            let a = std::f64::consts::FRAC_PI_2 * i as f64 / 12.0;
            pts.push((10 + (150.0 * a.cos()) as i32, 10 + (100.0 * a.sin()) as i32));
        }
        pts.extend([(10, 110), (9, 60), (10, 10)]);
        let (mut way, _) = fan(10.0);
        // The fallback corner would then be the hull's first point, not home plate.
        way.id = 0;
        way.nodes = pts
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64,
                tags: HashMap::new(),
                x,
                z,
            })
            .collect();
        let fill: Vec<(i32, i32)> = (10..150)
            .flat_map(|x| (10..100).map(move |z| (x, z)))
            .collect();
        let ((hx, hz), _) = home_plate(&way, &fill).expect("a home plate");
        assert!(
            (hx - 10.0).abs() < 2.0 && (hz - 10.0).abs() < 2.0,
            "{hx},{hz}"
        );
    }

    #[test]
    fn ballpark_gets_clay_bases_foul_lines_and_a_wall() {
        let xzbbox = XZBBox::rect_from_xz_lengths(140.0, 140.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let (way, fill) = fan(110.0);
        draw_pitch_markings(&mut editor, &way, &fill, GRASS_BLOCK);
        // 27.4 block bases: first at (37, 10), second at (37, 37), the mound between.
        assert!(
            editor.check_for_block(37, 0, 37, Some(&[WHITE_CONCRETE])),
            "second base"
        );
        assert!(
            editor.check_for_block(37, 0, 22, Some(&[TERRACOTTA])),
            "base path"
        );
        assert!(
            !editor.check_for_block(25, 0, 18, Some(&[TERRACOTTA])),
            "infield grass"
        );
        assert!(
            editor.check_for_block(70, 0, 10, Some(&[WHITE_CONCRETE])),
            "foul line"
        );
        let wall = (85..=90)
            .any(|x| (85..=90).any(|z| editor.check_for_block(x, 1, z, Some(&[GREEN_CONCRETE]))));
        assert!(wall, "outfield wall");
    }

    #[test]
    fn sports_lay_their_own_ground_without_a_surface_tag() {
        let tags = |sport: &str| -> HashMap<String, String> {
            [("sport".to_string(), sport.to_string())]
                .into_iter()
                .collect()
        };
        assert_eq!(default_surface(&tags("beachvolleyball")), Some(SAND));
        assert_eq!(default_surface(&tags("athletics")), Some(RED_TERRACOTTA));
        assert_eq!(default_surface(&tags("soccer")), None);
    }

    #[test]
    fn hull_keeps_every_corner_of_a_rectangle() {
        let hull = hull_ccw(&[(0, 0), (10, 0), (10, 5), (0, 5), (4, 2)]);
        assert_eq!(hull.len(), 4, "{hull:?}");
        for corner in [(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)] {
            assert!(hull.contains(&corner), "{corner:?} missing from {hull:?}");
        }
    }
}
