//! Roof shapes the laser saw, for buildings OpenStreetMap gives no `roof:shape`.
//!
//! The outline is first laid onto the roof it covers: OSM traces a building a
//! metre or two off at times, and nodes and survey cells are taken half a block
//! apart. Then every roof cell under it gets the plane through its neighbours.
//! Where that plane is clean and pitched (not the step of a set-back storey or
//! a parapet), the cell slopes, and falls one way. Together the slopes give the
//! shape:
//!
//! * hardly any, and its middle no higher than its rim: flat (a gently
//!   pitched roof under a flat top has its slopes too shallow to count);
//! * along every wall falling towards it: hipped;
//! * along some walls running sideways, falling towards the others: gabled,
//!   its ridge across the way most of its slopes fall.
//!
//! Slopes nearly all falling one way are left to the generator: they are as
//! often rows of solar panels on a flat roof as a mono-pitch one. So is
//! anything else, or too little seen, and small outlines. Each cell's plane
//! carries a dormer, a chimney or a solar panel only where they stand. A roof
//! reaching the highest height the survey records (51 m) is seen only below
//! it. Heights are above the scan's ground, so the
//! terrain's slope under the outline is added back first: a flat roof on a
//! hillside must not read as a mono-pitch.
//!
//! The result is a hint the generator builds as one of its own roofs, not a
//! mapped tag: turned outlines, roofs under a mapped height and the massing of
//! towers follow the rules of an untagged building, and only the shape and the
//! ridge's direction come from the laser.

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use rayon::prelude::*;

/// The measured shape, read by the generator after a mapped `roof:shape`.
pub const SHAPE_HINT: &str = "arnis:roof:shape";
/// The bearing the measured roof's slope faces, as `roof:direction` gives it.
pub const DIRECTION_HINT: &str = "arnis:roof:direction";

/// Blocks the outline may be moved, either way, to lie on its roof.
const REGISTER: i32 = 2;
/// Share of an outline's blocks that must be roof once it lies on it.
const MIN_COVER: f64 = 0.6;
/// Outlines smaller than this are left to the generator, in square metres.
const MIN_AREA_M2: f64 = 30.0;
/// Outlines larger than this many blocks are halls or blocks, not roofs to read.
const MAX_BLOCKS: usize = 40_000;
/// The highest height the survey records (255 fifths of a metre): a roof cell
/// there may stand any height above it, so it is not read.
const SATURATED_M: f32 = 51.0;
/// Most a cell's neighbours may stand off the plane through them, in metres:
/// past it they straddle a step, not a slope.
const PLANE_RMS_M: f64 = 0.3;
/// Pitches a cell slopes at, as rise a metre: about 14 to 60 degrees.
const MIN_PITCH: f64 = 0.25;
const MAX_PITCH: f64 = 1.8;
/// A roof with fewer of its cells sloping than this is flat; one with fewer
/// than the second share, or than [`MIN_SLOPED`] cells, is too mixed to tell.
const FLAT_SHARE: f64 = 0.15;
/// A flat roof's middle (over [`MIDDLE_M`] in from its walls) stands at most
/// this much above its rim (within [`RIM_M`] of them), in metres; an outline
/// too narrow to have a middle spans at most [`FLAT_SPAN_M`] from its lower
/// tenth to its upper.
const FLAT_RISE_M: f64 = 0.5;
const RIM_M: f64 = 1.5;
const MIDDLE_M: f64 = 3.0;
const FLAT_SPAN_M: f64 = 1.5;
const PITCHED_SHARE: f64 = 0.35;
const MIN_SLOPED: usize = 12;
/// How alike the ways the cells fall may be: past it they are a mono-pitch roof
/// or the solar panels on a flat one, and which is not told.
const ONE_WAY: f64 = 0.8;
/// How far a slope may turn from a wall's normal and still fall towards it (or
/// from its line and still run along it), in degrees.
const TURN_DEG: f64 = 35.0;
/// A wall is read from the cells this far inside it, along its middle half,
/// when it is at least this long and has this many of them sloping; a share of
/// them falling towards it or running along it makes it an eave or a gable.
const BAND_M: f64 = 2.0;
const WALL_MIN_M: f64 = 3.0;
const WALL_CELLS: usize = 3;
const WALL_SHARE: f64 = 0.6;
/// A hipped roof is read along most of its outline, and eaves along most of
/// what was read.
const HIP_READ: f64 = 0.85;
const HIP_EAVES: f64 = 0.8;
/// Most of the slopes that may fall towards the middle (a valley or a court).
const MAX_INWARD: f64 = 0.15;
/// How alike the axes the cells fall along must be for a gabled roof's ridge.
const ONE_AXIS: f64 = 0.6;

/// Fills the measured roof hints in place; returns how many buildings got one.
pub fn apply(
    elements: &mut [ProcessedElement],
    ground: &Ground,
    xzbbox: &XZBBox,
    scale: f64,
) -> usize {
    if !ground.has_laser() || !ground.laser_roofs_wanted() {
        return 0;
    }
    elements
        .par_iter_mut()
        .filter_map(|el| match el {
            ProcessedElement::Way(way) if wants_roof(way) => Some(way),
            _ => None,
        })
        .map(|way| match measure(way, ground, xzbbox, scale) {
            Some(roof) => {
                way.tags
                    .insert(SHAPE_HINT.to_string(), roof.shape.to_string());
                if let Some(deg) = roof.direction {
                    way.tags
                        .insert(DIRECTION_HINT.to_string(), format!("{deg:.0}"));
                }
                1
            }
            None => 0,
        })
        .sum()
}

/// A building outline whose roof OpenStreetMap leaves open.
fn wants_roof(way: &ProcessedWay) -> bool {
    let t = &way.tags;
    t.get("building").is_some_and(|b| b != "no")
        && !t.contains_key("building:part")
        && !t.contains_key("roof:shape")
        && !t.contains_key(SHAPE_HINT)
}

/// The measured roof: one of the generator's shapes, and for a gabled roof the
/// bearing its slopes face.
#[derive(Debug, PartialEq)]
struct Roof {
    shape: &'static str,
    direction: Option<f64>,
}

/// One sloping cell: the way it falls (a unit vector in blocks), and the wall
/// nearest it with how far along that wall (0 to 1) and how far off, in blocks.
struct Slope {
    falls: (f64, f64),
    wall: usize,
    along: f64,
    off: f64,
}

fn measure(way: &ProcessedWay, ground: &Ground, xzbbox: &XZBBox, scale: f64) -> Option<Roof> {
    let mut ring: Vec<(f64, f64)> = way
        .nodes
        .iter()
        .map(|n| (f64::from(n.x), f64::from(n.z)))
        .collect();
    if ring.len() >= 2 && ring.first() == ring.last() {
        ring.pop();
    }
    if ring.len() < 3 {
        return None;
    }
    // Nodes are rounded down to their block and the survey samples a block at
    // its corner: block (x, z) holds the roof at (x - 0.5, z - 0.5) here.
    let at = |x: i32, z: i32| (f64::from(x) - 0.5, f64::from(z) - 0.5);
    let (x0, x1) = minmax(ring.iter().map(|p| p.0));
    let (z0, z1) = minmax(ring.iter().map(|p| p.1));
    let margin = REGISTER + 2;
    let (gx0, gz0) = (x0.floor() as i32 - margin, z0.floor() as i32 - margin);
    let w = (x1.ceil() as i32 + margin - gx0 + 1).max(1) as usize;
    let h = (z1.ceil() as i32 + margin - gz0 + 1).max(1) as usize;
    if w * h > 4 * MAX_BLOCKS {
        return None;
    }
    let mut inside_cells = Vec::new();
    for gz in 0..h {
        for gx in 0..w {
            let (px, pz) = at(gx0 + gx as i32, gz0 + gz as i32);
            if inside(&ring, px, pz) {
                inside_cells.push((gx as i64, gz as i64));
            }
        }
    }
    let blocks = inside_cells.len();
    if blocks > MAX_BLOCKS || (blocks as f64) / (scale * scale) < MIN_AREA_M2 {
        return None;
    }

    // The roof's height in metres over the terrain at the window's middle;
    // NaN off the roof.
    let to_world = |gx: usize, gz: usize| {
        XZPoint::new(
            gx0 + gx as i32 - xzbbox.min_x(),
            gz0 + gz as i32 - xzbbox.min_z(),
        )
    };
    let bpm = ground.blocks_per_meter();
    let terrain_ref = ground.level_exact(to_world(w / 2, h / 2));
    let mut roof = vec![f64::NAN; w * h];
    for gz in 0..h {
        for gx in 0..w {
            let coord = to_world(gx, gz);
            if let Some(c) = ground.laser_cell(coord) {
                if c.kind == super::Kind::Building && c.height > 1.5 && c.height < SATURATED_M {
                    let terrain = (ground.level_exact(coord) - terrain_ref) / bpm;
                    roof[gz * w + gx] = f64::from(c.height) + terrain;
                }
            }
        }
    }
    let roof_at = |gx: i64, gz: i64| -> f64 {
        if gx < 0 || gz < 0 || gx as usize >= w || gz as usize >= h {
            f64::NAN
        } else {
            roof[gz as usize * w + gx as usize]
        }
    };

    // Lay the outline on the roof: the shift covering most of it, the
    // smallest of those.
    let mut best = (0usize, i64::MAX, 0i64, 0i64);
    for dz in -REGISTER..=REGISTER {
        for dx in -REGISTER..=REGISTER {
            let (dx, dz) = (i64::from(dx), i64::from(dz));
            let covered = inside_cells
                .iter()
                .filter(|&&(gx, gz)| !roof_at(gx + dx, gz + dz).is_nan())
                .count();
            let moved = dx.abs() + dz.abs();
            if covered > best.0 || (covered == best.0 && moved < best.1) {
                best = (covered, moved, dx, dz);
            }
        }
    }
    let (covered, _, sx, sz) = best;
    if (covered as f64) < MIN_COVER * blocks as f64 {
        return None;
    }
    let laid = |gx: i64, gz: i64| roof_at(gx + sx, gz + sz);

    let walls: Vec<Wall> = (0..ring.len())
        .filter_map(|i| Wall::new(&ring, i))
        .collect();
    let mut cells = 0usize;
    let mut slopes = Vec::new();
    let (mut rim, mut middle, mut all) = (Vec::new(), Vec::new(), Vec::new());
    for &(gx, gz) in &inside_cells {
        let here = laid(gx, gz);
        if here.is_nan() {
            continue;
        }
        let (px, pz) = at(gx0 + gx as i32, gz0 + gz as i32);
        let nearest = nearest_wall(&walls, px, pz);
        all.push(here);
        if let Some((_, _, off)) = nearest {
            if off / scale <= RIM_M {
                rim.push(here);
            } else if off / scale >= MIDDLE_M {
                middle.push(here);
            }
        }
        let mut pts = [(0.0, 0.0, 0.0); 9];
        let mut n = 0;
        for oz in -1..=1 {
            for ox in -1..=1 {
                let v = laid(gx + ox, gz + oz);
                if !v.is_nan() {
                    pts[n] = (ox as f64, oz as f64, v);
                    n += 1;
                }
            }
        }
        if n < 6 {
            continue;
        }
        cells += 1;
        let Some((bx, bz)) = clean_plane(&pts[..n]) else {
            continue;
        };
        // Rise a metre: a block is 1 / scale metres.
        let (rx, rz) = (bx * scale, bz * scale);
        let pitch = rx.hypot(rz);
        if !(MIN_PITCH..=MAX_PITCH).contains(&pitch) {
            continue;
        }
        if let Some((wall, along, off)) = nearest {
            slopes.push(Slope {
                falls: (-rx / pitch, -rz / pitch),
                wall,
                along,
                off,
            });
        }
    }
    let levelled = match (median(&mut rim), median(&mut middle)) {
        (Some(r), Some(m)) => m - r <= FLAT_RISE_M,
        _ => spread(&mut all) <= FLAT_SPAN_M,
    };
    decide(&walls, &slopes, cells, levelled, scale)
}

fn median(v: &mut [f64]) -> Option<f64> {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied()
}

/// From the lower tenth of `v` to its upper.
fn spread(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    at(0.9) - at(0.1)
}

/// One side of the outline: where it starts, its direction and length in
/// blocks, and the normal pointing out of the building.
struct Wall {
    start: (f64, f64),
    dir: (f64, f64),
    len: f64,
    out: (f64, f64),
}

impl Wall {
    fn new(ring: &[(f64, f64)], i: usize) -> Option<Wall> {
        let a = ring[i];
        let b = ring[(i + 1) % ring.len()];
        let (dx, dz) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dz);
        if len < 1e-9 {
            return None;
        }
        let dir = (dx / len, dz / len);
        let mid = (a.0 + dx / 2.0, a.1 + dz / 2.0);
        let probe = 0.3_f64.min(len / 4.0);
        let n = (dir.1, -dir.0);
        let out = if inside(ring, mid.0 + n.0 * probe, mid.1 + n.1 * probe) {
            (-n.0, -n.1)
        } else {
            n
        };
        Some(Wall {
            start: a,
            dir,
            len,
            out,
        })
    }
}

/// The wall nearest (x, z): its index, how far along it (0 to 1) and how far
/// off it, in blocks.
fn nearest_wall(walls: &[Wall], x: f64, z: f64) -> Option<(usize, f64, f64)> {
    let mut best: Option<(usize, f64, f64)> = None;
    for (i, w) in walls.iter().enumerate() {
        let (rx, rz) = (x - w.start.0, z - w.start.1);
        let t = ((rx * w.dir.0 + rz * w.dir.1) / w.len).clamp(0.0, 1.0);
        let (qx, qz) = (
            w.start.0 + w.dir.0 * w.len * t,
            w.start.1 + w.dir.1 * w.len * t,
        );
        let d = (x - qx).hypot(z - qz);
        if best.is_none_or(|b| d < b.2) {
            best = Some((i, t, d));
        }
    }
    best
}

/// The gradient (a block, in x and z) of the plane through `pts` (offset x,
/// offset z, height), if they lie on one.
fn clean_plane(pts: &[(f64, f64, f64)]) -> Option<(f64, f64)> {
    let n = pts.len() as f64;
    let (mut su, mut sw, mut suu, mut sww, mut suw) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut sv, mut suv, mut swv) = (0.0, 0.0, 0.0);
    for &(u, w, v) in pts {
        su += u;
        sw += w;
        suu += u * u;
        sww += w * w;
        suw += u * w;
        sv += v;
        suv += u * v;
        swv += w * v;
    }
    // Normal equations of v = a + b u + c w, by Cramer's rule.
    let m = [[n, su, sw], [su, suu, suw], [sw, suw, sww]];
    let det = det3(m);
    if det.abs() < 1e-9 {
        return None;
    }
    let rhs = [sv, suv, swv];
    let solve = |col: usize| {
        let mut mm = m;
        for (row, r) in mm.iter_mut().zip(rhs) {
            row[col] = r;
        }
        det3(mm) / det
    };
    let (a, b, c) = (solve(0), solve(1), solve(2));
    let mse = pts
        .iter()
        .map(|&(u, w, v)| (v - (a + b * u + c * w)).powi(2))
        .sum::<f64>()
        / n;
    (mse.sqrt() <= PLANE_RMS_M).then_some((b, c))
}

fn det3(m: [[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// The roof the slopes describe over `cells` read cells, if they describe one;
/// `levelled` says whether its middle stands no higher than its rim.
fn decide(
    walls: &[Wall],
    slopes: &[Slope],
    cells: usize,
    levelled: bool,
    scale: f64,
) -> Option<Roof> {
    if cells < 10 {
        return None;
    }
    let sloping = slopes.len() as f64 / cells as f64;
    if sloping < FLAT_SHARE {
        return levelled.then_some(Roof {
            shape: "flat",
            direction: None,
        });
    }
    if sloping < PITCHED_SHARE || slopes.len() < MIN_SLOPED {
        return None;
    }
    let n = slopes.len() as f64;
    let (mx, mz) = slopes
        .iter()
        .fold((0.0, 0.0), |(x, z), s| (x + s.falls.0, z + s.falls.1));
    if (mx / n).hypot(mz / n) >= ONE_WAY {
        return None;
    }

    let (cos, sin) = (TURN_DEG.to_radians().cos(), TURN_DEG.to_radians().sin());
    let towards = |s: &Slope| dot(s.falls, walls[s.wall].out);
    let inward = slopes.iter().filter(|s| towards(s) <= -cos).count() as f64 / n;
    if inward > MAX_INWARD {
        return None;
    }
    // Per wall, its band's sloping cells: how many, falling towards it, along it.
    let mut bands = vec![(0usize, 0usize, 0usize); walls.len()];
    for s in slopes {
        if s.off / scale <= BAND_M && (0.25..=0.75).contains(&s.along) {
            let t = towards(s);
            let band = &mut bands[s.wall];
            band.0 += 1;
            if t >= cos {
                band.1 += 1;
            } else if t.abs() <= sin {
                band.2 += 1;
            }
        }
    }
    let (mut long_m, mut read_m, mut eaves_m, mut gables_m) = (0.0, 0.0, 0.0, 0.0);
    for (wall, &(n, falls_to, runs_along)) in walls.iter().zip(&bands) {
        let len_m = wall.len / scale;
        if len_m < WALL_MIN_M {
            continue;
        }
        long_m += len_m;
        if n < WALL_CELLS {
            continue;
        }
        read_m += len_m;
        if falls_to as f64 >= WALL_SHARE * n as f64 {
            eaves_m += len_m;
        } else if runs_along as f64 >= WALL_SHARE * n as f64 {
            gables_m += len_m;
        }
    }
    if read_m > 0.0
        && read_m >= HIP_READ * long_m
        && gables_m == 0.0
        && eaves_m >= HIP_EAVES * read_m
    {
        return Some(Roof {
            shape: "hipped",
            direction: None,
        });
    }
    if gables_m > 0.0 && eaves_m > 0.0 {
        // The axis most slopes fall along (angles doubled, so both sides of
        // the ridge count alike); an L's two wings give none.
        let (c2, s2) = slopes.iter().fold((0.0, 0.0), |(c, s), sl| {
            let a = 2.0 * sl.falls.1.atan2(sl.falls.0);
            (c + a.cos(), s + a.sin())
        });
        let (c2, s2) = (c2 / n, s2 / n);
        let axis = 0.5 * s2.atan2(c2);
        let direction = (c2.hypot(s2) >= ONE_AXIS).then(|| bearing((axis.cos(), axis.sin())));
        return Some(Roof {
            shape: "gabled",
            direction,
        });
    }
    None
}

fn dot(a: (f64, f64), b: (f64, f64)) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

/// Compass bearing of a direction in blocks (north is -z), as `roof:direction`
/// takes it.
fn bearing(v: (f64, f64)) -> f64 {
    v.0.atan2(-v.1).to_degrees().rem_euclid(360.0)
}

fn minmax(v: impl Iterator<Item = f64>) -> (f64, f64) {
    v.fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(x), b.max(x)))
}

/// Even-odd point in polygon.
fn inside(ring: &[(f64, f64)], x: f64, z: f64) -> bool {
    let mut odd = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (xi, zi) = ring[i];
        let (xj, zj) = ring[j];
        if (zi > z) != (zj > z) && x < (xj - xi) * (z - zi) / (zj - zi) + xi {
            odd = !odd;
        }
        j = i;
    }
    odd
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::ProcessedNode;

    const W: usize = 40;

    /// What `apply` makes of an outline round `corners` with `tags`, over a
    /// survey whose roof stands `h(x, z)` metres at each point of the
    /// outline's plane (None: no roof), with the outline laid `shift` blocks
    /// off it.
    fn read_roof(
        corners: &[(i32, i32)],
        tags: &[(&str, &str)],
        shift: (i32, i32),
        h: impl Fn(f64, f64) -> Option<f64>,
    ) -> (Option<String>, Option<String>) {
        let mut cells = vec![[0u8; 5]; W * W];
        for z in 0..W {
            for x in 0..W {
                // Block (x, z) holds the roof at (x - 0.5, z - 0.5).
                let (px, pz) = (
                    x as f64 - 0.5 - f64::from(shift.0),
                    z as f64 - 0.5 - f64::from(shift.1),
                );
                if let Some(m) = h(px, pz) {
                    let q = (m * 5.0).round().clamp(0.0, 255.0) as u8;
                    cells[z * W + x] = [super::super::classify::BUILDING, q, 0, 0, 0];
                }
            }
        }
        let features = super::super::Features {
            garden: false,
            trees: false,
            heights: false,
            roofs: true,
        };
        let mut ground = Ground::new_flat(0);
        ground.set_world_dims(W, W);
        ground.set_survey(Some(super::super::SurveyData::from_cells(
            cells, W, W, features,
        )));
        let mut nodes: Vec<ProcessedNode> = corners
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64 + 1,
                tags: Default::default(),
                x,
                z,
            })
            .collect();
        nodes.push(nodes[0].clone());
        let way = ProcessedWay {
            id: 1,
            nodes,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let xzbbox = XZBBox::rect_from_xz_lengths((W - 1) as f64, (W - 1) as f64).unwrap();
        let mut els = vec![ProcessedElement::Way(way)];
        apply(&mut els, &ground, &xzbbox, 1.0);
        let t = els[0].tags();
        (t.get(SHAPE_HINT).cloned(), t.get(DIRECTION_HINT).cloned())
    }

    /// A 16 x 10 house over x 5..21, z 5..15, its roof reaching half a metre
    /// past its walls.
    const HOUSE: [(i32, i32); 4] = [(5, 5), (21, 5), (21, 15), (5, 15)];
    fn over_house(x: f64, z: f64) -> bool {
        (4.5..=21.5).contains(&x) && (4.5..=15.5).contains(&z)
    }
    fn house(h: impl Fn(f64, f64) -> f64) -> (Option<String>, Option<String>) {
        read_roof(&HOUSE, &[("building", "house")], (0, 0), move |x, z| {
            over_house(x, z).then(|| h(x, z))
        })
    }
    /// Distance in from the long walls (z = 5, 15) and the short ones.
    fn from_long(z: f64) -> f64 {
        (z - 5.0).min(15.0 - z)
    }
    fn from_short(x: f64) -> f64 {
        (x - 5.0).min(21.0 - x)
    }
    fn direction(d: Option<String>) -> f64 {
        d.expect("a direction").parse().unwrap()
    }

    #[test]
    fn a_gabled_roof_and_its_ridge() {
        // Eaves on the long walls, 40 degrees up to a ridge along x: the slopes
        // face north and south.
        let (shape, dir) = house(|_, z| 6.0 + 0.84 * from_long(z));
        assert_eq!(shape.as_deref(), Some("gabled"));
        let d = direction(dir);
        assert!(d == 0.0 || d == 180.0, "{d}");
        // Turned the other way, the ridge runs along z, across the long side.
        let (shape, dir) = house(|x, _| 6.0 + 0.84 * from_short(x));
        assert_eq!(shape.as_deref(), Some("gabled"));
        let d = direction(dir);
        assert!(d == 90.0 || d == 270.0, "{d}");
    }

    #[test]
    fn hipped_and_flat_but_not_one_way() {
        let hip = |x: f64, z: f64| 6.0 + 0.7 * from_long(z).min(from_short(x));
        assert_eq!(house(hip), (Some("hipped".into()), None));
        // Falling to the south wall (+z): a mono-pitch, or solar panels.
        let mono = |_: f64, z: f64| 4.0 + 0.4 * (15.0 - z);
        assert_eq!(house(mono), (None, None));
        assert_eq!(house(|_, _| 7.0), (Some("flat".into()), None));
        // A parapet never makes a roof pitched (a small one is left to the
        // generator), and a storey set back from the walls is a step, not a
        // slope: the roof is left to the generator, its middle being higher.
        let parapet = |x: f64, z: f64| {
            if from_long(z).min(from_short(x)) < 0.8 {
                8.2
            } else {
                7.0
            }
        };
        assert!(matches!(house(parapet).0.as_deref(), None | Some("flat")));
        // A shallow hip under a flat top: too gentle to slope, but its middle
        // stands well over its rim.
        let flat_top = |x: f64, z: f64| {
            15.0 + (0.2 * from_long(z).min(from_short(x))).min(1.0)
                + if from_long(z).min(from_short(x)) > 3.0 {
                    2.0
                } else {
                    0.0
                }
        };
        assert_eq!(house(flat_top).0, None);
        let set_back = |x: f64, z: f64| {
            if from_long(z).min(from_short(x)) < 2.0 {
                17.0
            } else {
                20.0
            }
        };
        assert_eq!(house(set_back), (None, None));
    }

    #[test]
    fn chimneys_and_dormers_do_not_change_the_shape() {
        let gable = |x: f64, z: f64| {
            let roof = 6.0 + 0.84 * from_long(z);
            if (9.0..11.0).contains(&x) && (9.0..11.0).contains(&z) {
                roof + 1.5
            } else if (14.0..17.0).contains(&x) && (12.0..14.0).contains(&z) {
                roof + 1.0
            } else {
                roof
            }
        };
        assert_eq!(house(gable).0.as_deref(), Some("gabled"));
    }

    #[test]
    fn an_outline_mapped_off_its_roof_is_laid_on_it() {
        let gable = |x: f64, z: f64| over_house(x, z).then(|| 6.0 + 0.84 * from_long(z));
        let (shape, dir) = read_roof(&HOUSE, &[("building", "house")], (2, -1), gable);
        assert_eq!(shape.as_deref(), Some("gabled"));
        let d = direction(dir);
        assert!(d == 0.0 || d == 180.0, "{d}");
    }

    #[test]
    fn an_l_keeps_its_shape_but_not_one_ridge() {
        // A 20 x 8 bar along x and an 8 x 20 arm down z, gabled each.
        let corners = [(5, 5), (25, 5), (25, 13), (13, 13), (13, 25), (5, 25)];
        let (shape, dir) = read_roof(&corners, &[("building", "house")], (0, 0), |x, z| {
            let in_bar = (4.5..=25.5).contains(&x) && (4.5..=13.5).contains(&z);
            let in_arm = (4.5..=13.5).contains(&x) && (4.5..=25.5).contains(&z);
            let bar = 5.0 + 0.8 * (z - 5.0).min(13.0 - z);
            let arm = 5.0 + 0.8 * (x - 5.0).min(13.0 - x);
            match (in_bar, in_arm) {
                (true, true) => Some(bar.max(arm)),
                (true, false) => Some(bar),
                (false, true) => Some(arm),
                _ => None,
            }
        });
        assert_eq!(shape.as_deref(), Some("gabled"));
        assert_eq!(dir, None);
    }

    #[test]
    fn mapped_roofs_parts_garages_and_unseen_roofs_are_left_alone() {
        let gable = |x: f64, z: f64| over_house(x, z).then(|| 6.0 + 0.84 * from_long(z));
        let mapped = [("building", "house"), ("roof:shape", "hipped")];
        assert_eq!(read_roof(&HOUSE, &mapped, (0, 0), gable), (None, None));
        let part = [("building:part", "yes"), ("building", "yes")];
        assert_eq!(read_roof(&HOUSE, &part, (0, 0), gable), (None, None));
        // A 4 x 6 m garage.
        let garage = [(5, 5), (9, 5), (9, 11), (5, 11)];
        let flat =
            |x: f64, z: f64| ((4.5..=9.5).contains(&x) && (4.5..=11.5).contains(&z)).then_some(3.0);
        assert_eq!(
            read_roof(&garage, &[("building", "garage")], (0, 0), flat),
            (None, None)
        );
        // The laser saw a third of the roof.
        let third = |x: f64, z: f64| gable(x, z).filter(|_| x < 10.0);
        assert_eq!(
            read_roof(&HOUSE, &[("building", "house")], (0, 0), third),
            (None, None)
        );
    }

    #[test]
    fn a_roof_over_the_highest_height_recorded_is_not_read() {
        // A tower's top, all of it at the survey's 51 m ceiling: no slope can
        // show there, and it must not read as flat.
        assert_eq!(house(|_, _| 60.0), (None, None));
        // A gable whose ridge alone rises past it is read from its lower slopes.
        let tall_gable = |_: f64, z: f64| 48.0 + 0.84 * from_long(z);
        let (shape, dir) = house(tall_gable);
        assert_eq!(shape.as_deref(), Some("gabled"));
        let d = direction(dir);
        assert!(d == 0.0 || d == 180.0, "{d}");
    }

    #[test]
    fn nothing_is_read_unless_measured_roofs_are_wanted() {
        let mut cells = vec![[0u8; 5]; W * W];
        for z in 5..16 {
            for x in 5..22 {
                cells[z * W + x] = [super::super::classify::BUILDING, 35, 0, 0, 0];
            }
        }
        let features = super::super::Features {
            garden: true,
            trees: true,
            heights: true,
            roofs: false,
        };
        let mut ground = Ground::new_flat(0);
        ground.set_world_dims(W, W);
        ground.set_survey(Some(super::super::SurveyData::from_cells(
            cells, W, W, features,
        )));
        let nodes = HOUSE
            .iter()
            .chain(HOUSE.first())
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64,
                tags: Default::default(),
                x,
                z,
            })
            .collect();
        let way = ProcessedWay {
            id: 1,
            nodes,
            tags: [("building".to_string(), "house".to_string())].into(),
        };
        let xzbbox = XZBBox::rect_from_xz_lengths((W - 1) as f64, (W - 1) as f64).unwrap();
        let mut els = vec![ProcessedElement::Way(way)];
        assert_eq!(apply(&mut els, &ground, &xzbbox, 1.0), 0);
        assert!(!els[0].tags().contains_key(SHAPE_HINT));
    }

    #[test]
    fn bearings_follow_roof_direction() {
        assert_eq!(bearing((0.0, -1.0)), 0.0);
        assert_eq!(bearing((1.0, 0.0)), 90.0);
        assert_eq!(bearing((0.0, 1.0)), 180.0);
        assert_eq!(bearing((-1.0, 0.0)), 270.0);
    }
}
