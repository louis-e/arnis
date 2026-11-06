//! Sports venues: raked stands, planned before the tile loop and placed post-merge, sized in metres.

use crate::args::{Args, GenerationMode};
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::sport_pitches;
use crate::land_cover::coord_hash;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;
use colored::Colorize;
use std::collections::{HashMap, HashSet};

type Key = (&'static str, u64);

/// Rows rise one block for every two of depth, about 27 degrees.
const RAKE: f32 = 0.5;
/// Chamfer distance units per block (5-7 metric).
const UNIT: f32 = 5.0;
/// Deepest stand built, in metres; anything mapped beyond is concourse or forecourt.
const MAX_DEPTH_M: f64 = 55.0;
/// A synthesized ring narrower than this stays open ground.
const MIN_DEPTH_M: f64 = 5.0;
const LIP_M: f64 = 1.5;
const TIER_M: f64 = 20.0;
const TIER_JUMP_M: f64 = 3.5;
const WALKWAY_M: f64 = 2.0;
const SEATS_CAP_M: f64 = 48.0;
const ROOF_CLEARANCE_M: f64 = 5.0;
const MAST_ABOVE_M: f64 = 18.0;
/// Pitches and tracks further than this from a lone grandstand are not what it faces.
const GRANDSTAND_REACH_M: f64 = 60.0;
/// Venue grids above this many cells are whole sports parks mistagged as one stadium.
const MAX_GRID_CELLS: usize = 1_500_000;
/// Seat-colour sections are about this many blocks wide at the front row.
const AISLE_SPACING: f64 = 22.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sport {
    Soccer,
    AmericanFootball,
    Rugby,
    Baseball,
    Cricket,
    Athletics,
    /// Australian rules and Gaelic games, played on large ovals.
    Oval,
    Indoor,
}

fn sport_of(tags: &HashMap<String, String>) -> Option<Sport> {
    let sport = tags.get("sport")?;
    // Bare "football" means whichever code the country plays, so a named one wins.
    let named = sport.split(';').find_map(|s| match s.trim() {
        "soccer" | "association_football" => Some(Sport::Soccer),
        "american_football" | "canadian_football" => Some(Sport::AmericanFootball),
        "rugby" | "rugby_union" | "rugby_league" => Some(Sport::Rugby),
        "baseball" | "softball" => Some(Sport::Baseball),
        "cricket" => Some(Sport::Cricket),
        "athletics" | "running" => Some(Sport::Athletics),
        "australian_football" | "gaelic_games" | "gaelic_football" | "hurling" => Some(Sport::Oval),
        "basketball" | "ice_hockey" | "handball" | "volleyball" | "boxing" | "futsal"
        | "gymnastics" | "badminton" | "table_tennis" | "wrestling" | "martial_arts"
        | "ice_skating" | "swimming" | "fencing" | "judo" | "squash" | "bowling" | "10pin"
        | "netball" | "esports" | "darts" | "billiards" => Some(Sport::Indoor),
        _ => None,
    });
    named.or_else(|| {
        sport
            .split(';')
            .any(|s| s.trim() == "football")
            .then_some(Sport::Soccer)
    })
}

impl Sport {
    /// Space between the field's lines and the first row, in metres.
    fn apron_m(self) -> f64 {
        match self {
            Sport::Soccer | Sport::Rugby => 6.0,
            Sport::AmericanFootball => 8.0,
            Sport::Baseball | Sport::Athletics => 2.0,
            Sport::Cricket | Sport::Oval => 4.0,
            Sport::Indoor => 4.0,
        }
    }

    /// Field size where none is mapped; baseball needs its mapped fan, so it is never invented.
    fn field_m(self) -> Option<(f64, f64)> {
        match self {
            Sport::Soccer | Sport::Athletics => Some((105.0, 68.0)),
            Sport::AmericanFootball => Some((110.0, 49.0)),
            Sport::Rugby => Some((120.0, 70.0)),
            Sport::Baseball | Sport::Cricket | Sport::Oval | Sport::Indoor => None,
        }
    }

    fn pitch_tag(self) -> &'static str {
        match self {
            Sport::AmericanFootball => "american_football",
            Sport::Rugby => "rugby",
            _ => "soccer",
        }
    }

    /// Ballparks, gridiron bowls and athletics grounds are open unless mapped with a roof.
    fn usually_roofed(self) -> bool {
        matches!(self, Sport::Soccer | Sport::Rugby)
    }
}

fn tag<'a>(tags: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    tags.get(key).map(String::as_str)
}

fn is_venue(tags: &HashMap<String, String>) -> bool {
    tag(tags, "leisure") == Some("stadium")
}

fn is_stand(tags: &HashMap<String, String>) -> bool {
    matches!(
        tag(tags, "building"),
        Some("grandstand" | "stadium" | "bleachers" | "tribune")
    ) || matches!(
        tag(tags, "building:part"),
        Some("grandstand" | "stadium" | "bleachers" | "tribune" | "stand")
    ) || tag(tags, "leisure") == Some("bleachers")
}

fn is_indoor(tags: &HashMap<String, String>) -> bool {
    tag(tags, "indoor") == Some("yes")
        || tag(tags, "covered") == Some("yes")
        || tag(tags, "location") == Some("indoor")
}

fn is_field(tags: &HashMap<String, String>) -> bool {
    matches!(tag(tags, "leisure"), Some("pitch" | "track")) && !is_indoor(tags)
}

fn is_building(tags: &HashMap<String, String>) -> bool {
    tags.get("building").is_some_and(|v| v != "no")
        || tags.get("building:part").is_some_and(|v| v != "no")
}

/// A roof, or a building part thin and high enough to be one.
fn is_roofish(tags: &HashMap<String, String>) -> bool {
    if tag(tags, "building") == Some("roof") || tag(tags, "building:part") == Some("roof") {
        return true;
    }
    let (Some(min), Some(top)) = (metres(tags, "min_height"), metres(tags, "height")) else {
        return false;
    };
    tags.contains_key("building:part") && min >= 4.0 && top - min <= 3.0
}

fn metres(tags: &HashMap<String, String>, key: &str) -> Option<f64> {
    let raw = tags.get(key)?;
    raw.trim()
        .trim_end_matches('m')
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bbox {
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
}

impl Bbox {
    fn union(self, o: Bbox) -> Bbox {
        Bbox {
            min_x: self.min_x.min(o.min_x),
            min_z: self.min_z.min(o.min_z),
            max_x: self.max_x.max(o.max_x),
            max_z: self.max_z.max(o.max_z),
        }
    }

    fn pad(self, p: i32) -> Bbox {
        Bbox {
            min_x: self.min_x - p,
            min_z: self.min_z - p,
            max_x: self.max_x + p,
            max_z: self.max_z + p,
        }
    }

    fn contains(&self, x: i32, z: i32) -> bool {
        x >= self.min_x && x <= self.max_x && z >= self.min_z && z <= self.max_z
    }
}

/// An area's rings, filled even-odd; multipolygon members stay open pieces that close together.
#[derive(Clone, Debug)]
struct Shape {
    rings: Vec<Vec<(i32, i32)>>,
    /// Which of `rings` are outer rings.
    outer: Vec<bool>,
    bbox: Bbox,
    centroid: (i32, i32),
    area: f64,
    has_hole: bool,
}

fn shape_of(element: &ProcessedElement) -> Option<Shape> {
    let mut rings: Vec<(Vec<(i32, i32)>, bool)> = Vec::new();
    match element {
        ProcessedElement::Way(w) => {
            let mut ring: Vec<(i32, i32)> = w.nodes.iter().map(|n| (n.x, n.z)).collect();
            if ring.len() >= 3 && ring.first() != ring.last() {
                ring.push(ring[0]);
            }
            rings.push((ring, true));
        }
        ProcessedElement::Relation(r) => {
            for m in &r.members {
                let outer = match m.role {
                    ProcessedMemberRole::Outer => true,
                    ProcessedMemberRole::Inner => false,
                    ProcessedMemberRole::Part => continue,
                };
                rings.push((m.way.nodes.iter().map(|n| (n.x, n.z)).collect(), outer));
            }
        }
        ProcessedElement::Node(_) => return None,
    }
    rings.retain(|(r, _)| r.len() >= 2);
    if rings.is_empty() {
        return None;
    }
    let pts = rings.iter().filter(|(_, o)| *o).flat_map(|(r, _)| r.iter());
    let (mut sx, mut sz, mut n) = (0i64, 0i64, 0i64);
    let mut bbox: Option<Bbox> = None;
    for &(x, z) in pts {
        sx += x as i64;
        sz += z as i64;
        n += 1;
        let b = Bbox {
            min_x: x,
            min_z: z,
            max_x: x,
            max_z: z,
        };
        bbox = Some(bbox.map_or(b, |a| a.union(b)));
    }
    let bbox = bbox?;
    let mut shape = Shape {
        has_hole: rings.iter().any(|(_, o)| !o),
        outer: rings.iter().map(|(_, o)| *o).collect(),
        rings: rings.into_iter().map(|(r, _)| r).collect(),
        bbox,
        centroid: ((sx / n) as i32, (sz / n) as i32),
        area: 0.0,
    };
    shape.area = if shape.rings.iter().all(|r| r.first() == r.last()) {
        (0..shape.rings.len())
            .map(|i| ring_area(&shape.rings[i]) * if shape.outer[i] { 1.0 } else { -1.0 })
            .sum::<f64>()
            .abs()
    } else {
        // Pieces of rings have no area of their own; count the cells they enclose.
        let grid = Grid::of(bbox);
        if grid.len() > MAX_GRID_CELLS {
            return None;
        }
        let mut mask = vec![false; grid.len()];
        rasterize(&grid, &shape, &mut mask);
        mask.iter().filter(|&&m| m).count() as f64
    };
    Some(shape)
}

impl Shape {
    /// The outline with its holes filled, so a pitch mapped as an inner ring stays inside.
    fn filled(&self) -> Shape {
        let keep = |i: &usize| self.outer[*i];
        Shape {
            rings: (0..self.rings.len())
                .filter(keep)
                .map(|i| self.rings[i].clone())
                .collect(),
            outer: vec![true; self.outer.iter().filter(|&&o| o).count()],
            has_hole: false,
            ..self.clone()
        }
    }

    /// Every mapped edge, closing edges of ways included.
    fn edges(&self) -> impl Iterator<Item = ((i32, i32), (i32, i32))> + '_ {
        self.rings
            .iter()
            .flat_map(|r| r.windows(2).map(|w| (w[0], w[1])))
    }
}

/// Shoelace area of a closed ring (first node repeated last).
fn ring_area(r: &[(i32, i32)]) -> f64 {
    let s: f64 = r
        .windows(2)
        .map(|w| w[0].0 as f64 * w[1].1 as f64 - w[1].0 as f64 * w[0].1 as f64)
        .sum();
    (s / 2.0).abs()
}

#[derive(Clone, Copy, Debug)]
struct Grid {
    x0: i32,
    z0: i32,
    w: usize,
    h: usize,
}

impl Grid {
    fn of(b: Bbox) -> Grid {
        Grid {
            x0: b.min_x,
            z0: b.min_z,
            w: (b.max_x - b.min_x + 1).max(1) as usize,
            h: (b.max_z - b.min_z + 1).max(1) as usize,
        }
    }

    fn len(&self) -> usize {
        self.w * self.h
    }

    fn idx(&self, x: i32, z: i32) -> Option<usize> {
        let (dx, dz) = (x - self.x0, z - self.z0);
        (dx >= 0 && dz >= 0 && (dx as usize) < self.w && (dz as usize) < self.h)
            .then(|| dz as usize * self.w + dx as usize)
    }

    fn xz(&self, i: usize) -> (i32, i32) {
        (self.x0 + (i % self.w) as i32, self.z0 + (i / self.w) as i32)
    }

    fn bbox(&self) -> Bbox {
        Bbox {
            min_x: self.x0,
            min_z: self.z0,
            max_x: self.x0 + self.w as i32 - 1,
            max_z: self.z0 + self.h as i32 - 1,
        }
    }
}

/// Even-odd fill of the rings plus their outlines, ORed into `out`.
fn rasterize(grid: &Grid, shape: &Shape, out: &mut [bool]) {
    let b = shape.bbox;
    for z in b.min_z.max(grid.z0)..=b.max_z.min(grid.z0 + grid.h as i32 - 1) {
        let zs = z as f64 + 1e-4;
        let mut xs: Vec<f64> = Vec::new();
        for (a, c) in shape.edges() {
            let (za, zc) = (a.1 as f64, c.1 as f64);
            if (za <= zs) != (zc <= zs) {
                xs.push(a.0 as f64 + (zs - za) * (c.0 - a.0) as f64 / (zc - za));
            }
        }
        xs.sort_by(|p, q| p.total_cmp(q));
        for k in 0..xs.len() / 2 {
            for x in xs[2 * k].ceil() as i32..=xs[2 * k + 1].floor() as i32 {
                if let Some(i) = grid.idx(x, z) {
                    out[i] = true;
                }
            }
        }
    }
    for (a, c) in shape.edges() {
        for (x, _, z) in bresenham_line(a.0, 0, a.1, c.0, 0, c.1) {
            if let Some(i) = grid.idx(x, z) {
                out[i] = true;
            }
        }
    }
}

/// 5-7 chamfer distance from the seed cells, in fifths of a block.
fn chamfer(grid: &Grid, seed: impl Fn(usize) -> bool) -> Vec<u16> {
    let (w, h) = (grid.w, grid.h);
    let mut d: Vec<u16> = (0..grid.len())
        .map(|i| if seed(i) { 0 } else { u16::MAX })
        .collect();
    let relax = |d: &mut Vec<u16>, i: usize, j: usize, c: u16| {
        let v = d[j].saturating_add(c);
        if v < d[i] {
            d[i] = v;
        }
    };
    for z in 0..h {
        for x in 0..w {
            let i = z * w + x;
            if x > 0 {
                relax(&mut d, i, i - 1, 5);
            }
            if z > 0 {
                relax(&mut d, i, i - w, 5);
                if x > 0 {
                    relax(&mut d, i, i - w - 1, 7);
                }
                if x + 1 < w {
                    relax(&mut d, i, i - w + 1, 7);
                }
            }
        }
    }
    for z in (0..h).rev() {
        for x in (0..w).rev() {
            let i = z * w + x;
            if x + 1 < w {
                relax(&mut d, i, i + 1, 5);
            }
            if z + 1 < h {
                relax(&mut d, i, i + w, 5);
                if x + 1 < w {
                    relax(&mut d, i, i + w + 1, 7);
                }
                if x > 0 {
                    relax(&mut d, i, i + w - 1, 7);
                }
            }
        }
    }
    d
}

/// Cells not reachable from the grid border without crossing `mask`.
fn enclosed(grid: &Grid, mask: &[bool]) -> Vec<bool> {
    let filled = crate::landmarks::fill_enclosed(mask, grid.w, grid.h);
    filled.iter().zip(mask).map(|(&f, &m)| f && !m).collect()
}

/// Centre, unit long axis and half extents of a set of cells.
#[derive(Clone, Copy, Debug)]
struct Frame {
    cx: f64,
    cz: f64,
    ux: f64,
    uz: f64,
    half_len: f64,
    half_wid: f64,
}

fn frame_of(cells: impl Iterator<Item = (i32, i32)> + Clone) -> Option<Frame> {
    let pts: Vec<(i32, i32)> = cells.collect();
    let (cx, cz, long, short, theta) = min_area_rect(&pts)?;
    Some(Frame {
        cx,
        cz,
        ux: theta.cos(),
        uz: theta.sin(),
        half_len: long / 2.0,
        half_wid: short / 2.0,
    })
}

/// Minimum-area enclosing rectangle: (centre x, centre z, long, short, long-axis angle).
fn min_area_rect(points: &[(i32, i32)]) -> Option<(f64, f64, f64, f64, f64)> {
    let hull = sport_pitches::hull_ccw(points);
    if hull.len() < 3 {
        return None;
    }
    let mut best: Option<(f64, f64, f64, f64, f64, f64)> = None;
    for i in 0..hull.len() {
        let (x0, z0) = hull[i];
        let (x1, z1) = hull[(i + 1) % hull.len()];
        let len = (x1 - x0).hypot(z1 - z0);
        if len < 1e-9 {
            continue;
        }
        let (ux, uz) = ((x1 - x0) / len, (z1 - z0) / len);
        let (vx, vz) = (-uz, ux);
        let (mut a0, mut a1, mut b0, mut b1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for &(px, pz) in &hull {
            let (a, b) = (px * ux + pz * uz, px * vx + pz * vz);
            a0 = a0.min(a);
            a1 = a1.max(a);
            b0 = b0.min(b);
            b1 = b1.max(b);
        }
        let area = (a1 - a0) * (b1 - b0);
        if best.is_none_or(|(ba, ..)| area < ba) {
            let (ca, cb) = ((a0 + a1) / 2.0, (b0 + b1) / 2.0);
            best = Some((
                area,
                ca * ux + cb * vx,
                ca * uz + cb * vz,
                a1 - a0,
                b1 - b0,
                uz.atan2(ux),
            ));
        }
    }
    let (_, cx, cz, ea, eb, theta) = best?;
    Some(if ea >= eb {
        (cx, cz, ea, eb, theta)
    } else {
        (cx, cz, eb, ea, theta + std::f64::consts::FRAC_PI_2)
    })
}

#[derive(Clone, Copy, Debug)]
struct Palette {
    seat: Block,
    accent: Block,
    wall: Block,
    rib: Block,
}

const SEATS: [(Block, Block); 6] = [
    (RED_CONCRETE, WHITE_CONCRETE),
    (BLUE_CONCRETE, WHITE_CONCRETE),
    (LIGHT_BLUE_CONCRETE, WHITE_CONCRETE),
    (YELLOW_CONCRETE, BLACK_CONCRETE),
    (CYAN_CONCRETE, WHITE_CONCRETE),
    (ORANGE_CONCRETE, BLACK_CONCRETE),
];

const FACADES: [(Block, Block); 3] = [
    (LIGHT_GRAY_CONCRETE, WHITE_CONCRETE),
    (WHITE_CONCRETE, LIGHT_GRAY_CONCRETE),
    (POLISHED_ANDESITE, LIGHT_GRAY_CONCRETE),
];

fn palette_for(id: u64, tags: &HashMap<String, String>) -> Palette {
    let h = coord_hash(id as i32, (id >> 32) as i32);
    let (seat, accent) = SEATS[(h % SEATS.len() as u64) as usize];
    let brick = matches!(tag(tags, "building:material"), Some("brick" | "bricks"));
    let (wall, rib) = if brick {
        (BRICK, LIGHT_GRAY_CONCRETE)
    } else {
        FACADES[((h >> 8) % FACADES.len() as u64) as usize]
    };
    Palette {
        seat,
        accent,
        wall,
        rib,
    }
}

/// A field painted where the venue has none mapped.
#[derive(Clone, Debug)]
struct SynthField {
    way: ProcessedWay,
    cells: Vec<(i32, i32)>,
}

#[derive(Clone, Copy, Debug)]
struct Profile {
    lip: i32,
    tier_len: f32,
    walkway: f32,
    jump: i32,
    tiers: u8,
    cap: i32,
}

impl Profile {
    /// Seat height, tier and walkway flag at `f` blocks from the front of the stand.
    fn at(&self, f: f32) -> (i32, u8, bool) {
        let mut base = self.lip;
        let mut rest = f;
        for tier in 0..self.tiers {
            let last = tier + 1 == self.tiers;
            if last || rest < self.tier_len {
                let h = base + (rest * RAKE).floor() as i32;
                return (h.min(self.cap), tier, false);
            }
            base += (self.tier_len * RAKE).floor() as i32;
            rest -= self.tier_len;
            if rest < self.walkway {
                return (base.min(self.cap), tier, true);
            }
            rest -= self.walkway;
            base += self.jump;
        }
        (base.min(self.cap), self.tiers.saturating_sub(1), false)
    }
}

#[derive(Clone, Debug)]
struct Venue {
    name: Option<String>,
    grid: Grid,
    stand: Vec<bool>,
    field: Vec<bool>,
    /// Chamfer distances from the stand's front row and from its back wall.
    front: Vec<u16>,
    back: Vec<u16>,
    centre: (f64, f64),
    field_frame: Option<Frame>,
    synth_field: Option<SynthField>,
    profile: Profile,
    /// Share of each stand's depth the roof covers, from the back; zero for none.
    roof_cover: f32,
    /// Height of the roof above the field, when the outline sets one taller than the seats.
    roof_top: Option<i32>,
    /// Stands shallower than this (blocks) stay open; a roof there would need a tall wall.
    roof_min_depth: f32,
    /// Blocks between the top row and the roof, and from the top row to a mast's lamps.
    roof_clearance: i32,
    mast_above: i32,
    palette: Palette,
    front_radius: f64,
}

impl Venue {
    fn stand_cells(&self) -> impl Iterator<Item = usize> + '_ {
        self.stand
            .iter()
            .enumerate()
            .filter(|(_, &s)| s)
            .map(|(i, _)| i)
    }
}

pub struct StadiumPlan {
    venues: Vec<Venue>,
    suppressed: HashSet<Key>,
}

impl StadiumPlan {
    pub fn empty() -> Self {
        Self {
            venues: Vec::new(),
            suppressed: HashSet::new(),
        }
    }

    pub fn suppressed(&self) -> &HashSet<Key> {
        &self.suppressed
    }

    pub fn placement_count(&self) -> usize {
        self.venues.len()
    }

    /// Regions each venue covers, widened by the tree sweep's reach.
    pub fn placement_regions(&self) -> Vec<Vec<(i32, i32)>> {
        self.venues
            .iter()
            .map(|v| {
                let b = v.grid.bbox().pad(crate::models_3d::TREE_SWEEP_REACH);
                ((b.min_x >> 9)..=(b.max_x >> 9))
                    .flat_map(|rx| ((b.min_z >> 9)..=(b.max_z >> 9)).map(move |rz| (rx, rz)))
                    .collect()
            })
            .collect()
    }
}

/// A planned venue, what it draws instead, and what it only takes (outline, fields).
struct Planned {
    venue: Venue,
    suppress: Vec<Key>,
    taken: Vec<Key>,
}

impl StadiumPlan {
    fn take(&mut self, p: Planned, claimed: &mut HashSet<Key>) {
        claimed.extend(p.suppress.iter().chain(&p.taken).copied());
        self.suppressed.extend(p.suppress);
        self.venues.push(p.venue);
    }
}

/// An element the plan reads, with its shape worked out once.
struct Candidate<'a> {
    element: &'a ProcessedElement,
    shape: Shape,
}

impl Candidate<'_> {
    fn key(&self) -> Key {
        (self.element.kind(), self.element.id())
    }

    fn tags(&self) -> &HashMap<String, String> {
        self.element.tags()
    }
}

/// Plans every venue in the area. `already` holds elements a landmark or 3D model claims.
pub fn prescan(elements: &[ProcessedElement], args: &Args, already: &HashSet<Key>) -> StadiumPlan {
    if !matches!(
        args.mode,
        GenerationMode::GeoTerrain | GenerationMode::GeoOnly
    ) {
        return StadiumPlan::empty();
    }
    let has_any = elements.iter().any(|e| {
        let t = e.tags();
        is_venue(t) || is_stand(t)
    });
    if !has_any {
        return StadiumPlan::empty();
    }

    let mut venues_in: Vec<Candidate> = Vec::new();
    let mut stands: Vec<Candidate> = Vec::new();
    for e in elements {
        let t = e.tags();
        let (venue, stand) = (is_venue(t), is_stand(t));
        if !(venue || stand) {
            continue;
        }
        let Some(c) = candidate(e, already) else {
            continue;
        };
        if venue && stand {
            // A stadium ring tagged as the venue too: a venue, and a stand of any outline round it.
            venues_in.push(Candidate {
                element: c.element,
                shape: c.shape.clone(),
            });
            stands.push(c);
        } else if venue {
            venues_in.push(c);
        } else {
            stands.push(c);
        }
    }

    // Only pitches and buildings near a venue or stand get shaped; large areas have millions.
    let reach = (GRANDSTAND_REACH_M * args.scale).ceil() as i32 + 8;
    let near: HashSet<(i32, i32)> = venues_in
        .iter()
        .chain(&stands)
        .flat_map(|c| region_cells(c.shape.bbox.pad(reach)))
        .collect();
    let mut fields: Vec<Candidate> = Vec::new();
    let mut others: Vec<Candidate> = Vec::new();
    for e in elements {
        let t = e.tags();
        if is_venue(t) || is_stand(t) {
            continue;
        }
        let (field, building) = (is_field(t), is_building(t));
        if !(field || building) {
            continue;
        }
        if !quick_bbox(e).is_some_and(|b| region_cells(b).any(|c| near.contains(&c))) {
            continue;
        }
        let Some(c) = candidate(e, already) else {
            continue;
        };
        if field {
            fields.push(c);
        } else {
            others.push(c);
        }
    }

    let scale = args.scale;
    let mut plan = StadiumPlan::empty();
    let mut claimed: HashSet<Key> = HashSet::new();

    // Big venues first, so a stand inside two outlines goes to the stadium, not a forecourt.
    venues_in.sort_by(|a, b| b.shape.area.total_cmp(&a.shape.area));
    for v in &venues_in {
        if claimed.contains(&v.key()) {
            continue;
        }
        if let Some(p) = plan_venue(v, &stands, &fields, &others, &claimed, scale) {
            plan.take(p, &mut claimed);
        }
    }

    // A stadium building ringing a field is a venue; a solid one is an arena building.
    let tried: HashSet<Key> = venues_in.iter().map(|v| v.key()).collect();
    let outlines: Vec<usize> = (0..stands.len())
        .filter(|&i| tag(stands[i].tags(), "building") == Some("stadium"))
        .filter(|&i| !tried.contains(&stands[i].key()))
        .collect();
    for i in outlines {
        let s = &stands[i];
        if claimed.contains(&s.key()) {
            continue;
        }
        if let Some(p) = plan_venue(s, &stands, &fields, &others, &claimed, scale) {
            plan.take(p, &mut claimed);
        }
    }

    // Lone grandstands face the nearest pitch or track.
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    let reach = GRANDSTAND_REACH_M * scale;
    for (i, s) in stands.iter().enumerate() {
        if claimed.contains(&s.key()) || tag(s.tags(), "building") == Some("stadium") {
            continue;
        }
        // Rooftop seats over the road from a ballpark stay on their building.
        if !is_building(s.tags()) && on_a_building(s, &others) {
            continue;
        }
        let (sx, sz) = s.shape.centroid;
        let nearest = fields
            .iter()
            .enumerate()
            .filter(|(_, f)| !claimed.contains(&f.key()))
            .map(|(j, f)| (j, bbox_distance(f.shape.bbox, sx, sz)))
            .filter(|&(_, d)| d <= reach)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((j, _)) = nearest {
            groups.entry(j).or_default().push(i);
        }
    }
    let mut group_list: Vec<(usize, Vec<usize>)> = groups.into_iter().collect();
    group_list.sort();
    for (field_idx, members) in group_list {
        let members: Vec<&Candidate> = members.iter().map(|&i| &stands[i]).collect();
        if let Some(p) = plan_grandstands(&fields[field_idx], &members, &others, &claimed, scale) {
            plan.take(p, &mut claimed);
        }
    }
    plan
}

fn candidate<'a>(e: &'a ProcessedElement, already: &HashSet<Key>) -> Option<Candidate<'a>> {
    if matches!(e, ProcessedElement::Node(_)) || already.contains(&(e.kind(), e.id())) {
        return None;
    }
    Some(Candidate {
        element: e,
        shape: shape_of(e)?,
    })
}

/// Coarse cells a bbox touches, for the reach test.
fn region_cells(b: Bbox) -> impl Iterator<Item = (i32, i32)> {
    const CELL: i32 = 7;
    ((b.min_x >> CELL)..=(b.max_x >> CELL))
        .flat_map(move |x| ((b.min_z >> CELL)..=(b.max_z >> CELL)).map(move |z| (x, z)))
}

/// An element's extent without copying its rings.
fn quick_bbox(e: &ProcessedElement) -> Option<Bbox> {
    let mut bbox: Option<Bbox> = None;
    let mut add = |x: i32, z: i32| {
        let p = Bbox {
            min_x: x,
            min_z: z,
            max_x: x,
            max_z: z,
        };
        bbox = Some(bbox.map_or(p, |b| b.union(p)));
    };
    match e {
        ProcessedElement::Way(w) => w.nodes.iter().for_each(|n| add(n.x, n.z)),
        ProcessedElement::Relation(r) => r
            .members
            .iter()
            .flat_map(|m| m.way.nodes.iter())
            .for_each(|n| add(n.x, n.z)),
        ProcessedElement::Node(_) => {}
    }
    bbox
}

fn on_a_building(stand: &Candidate, others: &[Candidate]) -> bool {
    let (x, z) = stand.shape.centroid;
    others
        .iter()
        .filter(|o| tags_are_plain_building(o.tags()) && o.shape.bbox.contains(x, z))
        .any(|o| {
            let local = Grid::of(o.shape.bbox);
            if local.len() > MAX_GRID_CELLS {
                return false;
            }
            let mut mask = vec![false; local.len()];
            rasterize(&local, &o.shape, &mut mask);
            local.idx(x, z).is_some_and(|i| mask[i])
        })
}

fn bbox_distance(b: Bbox, x: i32, z: i32) -> f64 {
    let dx = (b.min_x - x).max(0).max(x - b.max_x) as f64;
    let dz = (b.min_z - z).max(0).max(z - b.max_z) as f64;
    dx.hypot(dz)
}

fn m(scale: f64, metres: f64) -> f64 {
    metres * scale
}

/// The venue an outline makes, and the elements it replaces.
fn plan_venue(
    outline: &Candidate,
    stands: &[Candidate],
    fields: &[Candidate],
    others: &[Candidate],
    claimed: &HashSet<Key>,
    scale: f64,
) -> Option<Planned> {
    let tags = outline.tags();
    let outline_building = tags.get("building").filter(|b| b.as_str() != "no");
    let outline_is_stand =
        outline_building.is_some_and(|b| matches!(b.as_str(), "stadium" | "grandstand"));
    // An arena: the outline itself is a hall, which the building generator draws.
    if outline_building.is_some() && !outline_is_stand {
        return None;
    }
    // Roof and storey tags on a solid outline map a hall too, a dome with the tag left off.
    let hall_tags =
        tags.get("roof:shape").is_some_and(|r| r != "no") || tags.contains_key("building:levels");
    if hall_tags && !outline.shape.has_hole {
        return None;
    }

    let probe = Grid::of(outline.shape.bbox.pad(2));
    if probe.len() > MAX_GRID_CELLS {
        return None;
    }
    let mut inside = vec![false; probe.len()];
    rasterize(&probe, &outline.shape.filled(), &mut inside);
    let within = |c: &Candidate| {
        let (x, z) = c.shape.centroid;
        probe.idx(x, z).is_some_and(|i| inside[i])
    };

    // A second outline round a field another venue already seats would seat it again.
    if fields
        .iter()
        .any(|f| claimed.contains(&f.key()) && within(f))
    {
        return None;
    }
    let inner_fields: Vec<&Candidate> = fields.iter().filter(|f| within(f)).collect();
    let inner_stands: Vec<&Candidate> = stands
        .iter()
        .filter(|s| s.key() != outline.key() && !claimed.contains(&s.key()) && within(s))
        .collect();

    let sport =
        sport_of(tags).or_else(|| main_field(&inner_fields).and_then(|f| sport_of(f.tags())));
    let has_field = !inner_fields.is_empty();
    if outline_is_stand && !has_field && !outline.shape.has_hole {
        return None;
    }
    if sport == Some(Sport::Indoor) && !has_field {
        return None;
    }
    if is_indoor(tags) && !has_field {
        return None;
    }

    let field_set = field_group(&inner_fields);
    let mut stand_elems: Vec<&Candidate> = inner_stands;
    if outline_is_stand {
        stand_elems.push(outline);
    }

    let mut bbox = outline.shape.bbox;
    for c in stand_elems.iter().chain(field_set.iter()) {
        bbox = bbox.union(c.shape.bbox);
    }
    let mast_pad = 4;
    let grid = Grid::of(bbox.pad(mast_pad));
    if grid.len() > MAX_GRID_CELLS {
        return None;
    }
    let mut venue_mask = vec![false; grid.len()];
    rasterize(&grid, &outline.shape.filled(), &mut venue_mask);

    let mut field = vec![false; grid.len()];
    for f in &field_set {
        rasterize(&grid, &f.shape, &mut field);
    }
    let mut stand = vec![false; grid.len()];
    for s in &stand_elems {
        rasterize(&grid, &s.shape, &mut stand);
    }
    let mapped_stands = stand.iter().any(|&s| s);

    // Non-stand buildings; a venue mostly under one without mapped seating is an arena.
    let mut keep_out = vec![false; grid.len()];
    for o in others
        .iter()
        .filter(|o| within(o) && !claimed.contains(&o.key()))
    {
        if tags_are_plain_building(o.tags()) {
            rasterize(&grid, &o.shape, &mut keep_out);
        }
    }
    let venue_cells = venue_mask.iter().filter(|&&v| v).count();
    let roofed_over = (0..grid.len())
        .filter(|&i| venue_mask[i] && keep_out[i])
        .count();
    if roofed_over * 2 >= venue_cells && stand_elems.is_empty() {
        return None;
    }

    let mut synth_field = None;
    if !field.iter().any(|&f| f) {
        let hole = if mapped_stands {
            enclosed(&grid, &stand)
        } else {
            Vec::new()
        };
        let hole_cells = hole.iter().filter(|&&h| h).count() as f64;
        if hole_cells >= m(scale, 30.0) * m(scale, 20.0) {
            field = hole;
        } else if let Some(sf) = synthesize_field(&grid, &venue_mask, sport, outline.key().1, scale)
        {
            let idx: Vec<usize> = sf
                .cells
                .iter()
                .filter_map(|&(x, z)| grid.idx(x, z))
                .collect();
            // A building standing where the pitch would go means this is no bowl.
            let built = idx.iter().filter(|&&i| keep_out[i]).count();
            if built * 50 <= idx.len() {
                for i in idx {
                    field[i] = true;
                }
                synth_field = Some(sf);
            }
        }
    }
    if !field.iter().any(|&f| f) {
        return None;
    }

    let sport_or = sport.unwrap_or(Sport::Soccer);
    let field_dist = chamfer(&grid, |i| field[i]);
    let apron = (m(scale, sport_or.apron_m()) * UNIT as f64) as u32;
    if !mapped_stands {
        // No stands mapped: seat the ring between field and outline, around buildings.
        let max_d = (m(scale, MAX_DEPTH_M) * UNIT as f64) as u32 + apron;
        for i in 0..grid.len() {
            let d = field_dist[i] as u32;
            stand[i] = venue_mask[i] && !field[i] && !keep_out[i] && d >= apron && d <= max_d;
        }
        thin_out(
            &grid,
            &mut stand,
            (m(scale, MIN_DEPTH_M) * UNIT as f64) as u16,
        );
    } else {
        close_gaps(&grid, &mut stand, &venue_mask, m(scale, 4.0));
        let min_gap = UNIT as u16 * 2;
        for i in 0..grid.len() {
            if field[i] || field_dist[i] < min_gap {
                stand[i] = false;
            }
        }
    }

    let profile_input = ProfileInput {
        tags,
        stand_elems: &stand_elems,
        sport,
        scale,
    };
    let venue = build_venue(
        outline.key(),
        tags,
        grid,
        stand,
        field,
        &field_dist,
        synth_field,
        others,
        claimed,
        &profile_input,
    )?;

    let mut keys: Vec<Key> = stand_elems.iter().map(|s| s.key()).collect();
    if outline_is_stand {
        keys.push(outline.key());
    }
    keys.extend(covered_buildings(&venue, others, claimed));
    keys.sort();
    keys.dedup();
    let mut taken: Vec<Key> = field_set.iter().map(|f| f.key()).collect();
    taken.push(outline.key());
    Some(Planned {
        venue,
        suppress: keys,
        taken,
    })
}

/// Grandstands without a stadium outline, seated toward the pitch or track they line.
fn plan_grandstands(
    field_elem: &Candidate,
    members: &[&Candidate],
    others: &[Candidate],
    claimed: &HashSet<Key>,
    scale: f64,
) -> Option<Planned> {
    let mut bbox = field_elem.shape.bbox;
    for s in members {
        bbox = bbox.union(s.shape.bbox);
    }
    let grid = Grid::of(bbox.pad(4));
    if grid.len() > MAX_GRID_CELLS {
        return None;
    }
    let mut field = vec![false; grid.len()];
    rasterize(&grid, &field_elem.shape, &mut field);
    let mut stand = vec![false; grid.len()];
    for s in members {
        rasterize(&grid, &s.shape, &mut stand);
    }
    let field_dist = chamfer(&grid, |i| field[i]);
    for i in 0..grid.len() {
        if field[i] {
            stand[i] = false;
        }
    }
    let tags = members[0].tags();
    let stand_elems: Vec<&Candidate> = members.to_vec();
    let profile_input = ProfileInput {
        tags,
        stand_elems: &stand_elems,
        sport: sport_of(field_elem.tags()),
        scale,
    };
    let venue = build_venue(
        members[0].key(),
        tags,
        grid,
        stand,
        field,
        &field_dist,
        None,
        others,
        claimed,
        &profile_input,
    )?;
    let mut keys: Vec<Key> = members.iter().map(|s| s.key()).collect();
    keys.extend(covered_buildings(&venue, others, claimed));
    keys.sort();
    keys.dedup();
    Some(Planned {
        venue,
        suppress: keys,
        taken: vec![field_elem.key()],
    })
}

/// A building of its own rather than a slice of the stands.
fn tags_are_plain_building(tags: &HashMap<String, String>) -> bool {
    tags.get("building").is_some_and(|b| {
        !matches!(
            b.as_str(),
            "no" | "roof" | "stadium" | "grandstand" | "bleachers" | "tribune"
        )
    }) && !tags.contains_key("building:part")
}

/// The largest pitch, or failing one the largest track.
fn main_field<'a>(fields: &[&'a Candidate<'a>]) -> Option<&'a Candidate<'a>> {
    let largest = |want: &str| {
        fields
            .iter()
            .filter(|f| tag(f.tags(), "leisure") == Some(want))
            .max_by(|a, b| a.shape.area.total_cmp(&b.shape.area))
            .copied()
    };
    largest("pitch").or_else(|| largest("track"))
}

/// The main field and what is played around it: the track ringing it, pitches on it.
fn field_group<'a>(fields: &[&'a Candidate<'a>]) -> Vec<&'a Candidate<'a>> {
    let Some(main) = main_field(fields) else {
        return Vec::new();
    };
    let (mx, mz) = main.shape.centroid;
    fields
        .iter()
        .filter(|f| {
            f.key() == main.key()
                || f.shape.bbox.contains(mx, mz)
                || main
                    .shape
                    .bbox
                    .contains(f.shape.centroid.0, f.shape.centroid.1)
        })
        .copied()
        .collect()
}

/// Closes slivers between stand sections and ramp notches, up to `r` blocks.
fn close_gaps(grid: &Grid, stand: &mut [bool], within: &[bool], r: f64) {
    let r = (r * UNIT as f64) as u16;
    if r == 0 {
        return;
    }
    let to_stand = chamfer(grid, |i| stand[i]);
    let grown: Vec<bool> = to_stand.iter().map(|&d| d <= r).collect();
    let to_outside = chamfer(grid, |i| !grown[i]);
    for i in 0..grid.len() {
        if !stand[i] && within[i] && grown[i] && to_outside[i] > r {
            stand[i] = true;
        }
    }
}

/// Drops synthesized stand cells where the ring is too thin to hold a few rows.
fn thin_out(grid: &Grid, stand: &mut [bool], min_depth: u16) {
    let outside = chamfer(grid, |i| !stand[i]);
    // A cell survives if a cell at least `min_depth` deep lies within that reach.
    let deep = chamfer(grid, |i| stand[i] && outside[i] >= min_depth / 2);
    for i in 0..grid.len() {
        if stand[i] && deep[i] > min_depth / 2 + UNIT as u16 {
            stand[i] = false;
        }
    }
}

fn synthesize_field(
    grid: &Grid,
    venue: &[bool],
    sport: Option<Sport>,
    id: u64,
    scale: f64,
) -> Option<SynthField> {
    let sport = sport.unwrap_or(Sport::Soccer);
    let (len_m, wid_m) = sport.field_m()?;
    let cells: Vec<(i32, i32)> = (0..grid.len())
        .filter(|&i| venue[i])
        .map(|i| grid.xz(i))
        .collect();
    let fr = frame_of(cells.iter().copied())?;
    let apron = m(scale, sport.apron_m() + MIN_DEPTH_M);
    let half_len = (m(scale, len_m) / 2.0).min(fr.half_len - apron);
    let half_wid = (m(scale, wid_m) / 2.0).min(fr.half_wid - apron);
    // A venue far smaller than its field is a mapping slip, not a pitch to shrink.
    if half_len < m(scale, len_m) * 0.3 || half_wid < m(scale, wid_m) * 0.3 {
        return None;
    }
    // Far larger, and it is a sports park whose stadium is not mapped.
    if fr.half_len > m(scale, len_m) * 2.0 || fr.half_wid > m(scale, wid_m) * 2.5 {
        return None;
    }
    let (vx, vz) = (-fr.uz, fr.ux);
    let corner = |a: f64, b: f64| {
        (
            (fr.cx + a * fr.ux + b * vx).round() as i32,
            (fr.cz + a * fr.uz + b * vz).round() as i32,
        )
    };
    let corners = [
        corner(-half_len, -half_wid),
        corner(half_len, -half_wid),
        corner(half_len, half_wid),
        corner(-half_len, half_wid),
        corner(-half_len, -half_wid),
    ];
    let mut tags = HashMap::new();
    tags.insert("leisure".to_string(), "pitch".to_string());
    tags.insert("sport".to_string(), sport.pitch_tag().to_string());
    let way = ProcessedWay {
        id,
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
    };
    let shape = Shape {
        rings: vec![corners.to_vec()],
        outer: vec![true],
        bbox: Bbox {
            min_x: corners.iter().map(|c| c.0).min()?,
            min_z: corners.iter().map(|c| c.1).min()?,
            max_x: corners.iter().map(|c| c.0).max()?,
            max_z: corners.iter().map(|c| c.1).max()?,
        },
        centroid: (fr.cx as i32, fr.cz as i32),
        area: 4.0 * half_len * half_wid,
        has_hole: false,
    };
    let mut mask = vec![false; grid.len()];
    rasterize(grid, &shape, &mut mask);
    let cells = (0..grid.len())
        .filter(|&i| mask[i])
        .map(|i| grid.xz(i))
        .collect();
    Some(SynthField { way, cells })
}

struct ProfileInput<'a> {
    tags: &'a HashMap<String, String>,
    stand_elems: &'a [&'a Candidate<'a>],
    sport: Option<Sport>,
    scale: f64,
}

#[allow(clippy::too_many_arguments)]
fn build_venue(
    key: Key,
    tags: &HashMap<String, String>,
    grid: Grid,
    mut stand: Vec<bool>,
    field: Vec<bool>,
    field_dist: &[u16],
    synth_field: Option<SynthField>,
    others: &[Candidate],
    claimed: &HashSet<Key>,
    input: &ProfileInput,
) -> Option<Venue> {
    let scale = input.scale;
    let (front_seed, _) = edges(&grid, &stand, &field, field_dist);
    let front = chamfer(&grid, |i| front_seed[i]);
    let max_front = (m(scale, MAX_DEPTH_M) * UNIT as f64) as u16;
    for i in 0..grid.len() {
        if stand[i] && front[i] > max_front {
            stand[i] = false;
        }
    }
    let (_, back_seed) = edges(&grid, &stand, &field, field_dist);
    let back = chamfer(&grid, |i| back_seed[i]);
    let stand_count = stand.iter().filter(|&&s| s).count();
    if stand_count < 24 && synth_field.is_none() {
        return None;
    }

    let field_cells = (0..grid.len()).filter(|&i| field[i]).map(|i| grid.xz(i));
    let field_frame = frame_of(field_cells);
    let centre = field_frame.map_or_else(
        || {
            let b = grid.bbox();
            (
                (b.min_x + b.max_x) as f64 / 2.0,
                (b.min_z + b.max_z) as f64 / 2.0,
            )
        },
        |f| (f.cx, f.cz),
    );

    let mut depths: Vec<u32> = (0..grid.len())
        .filter(|&i| stand[i])
        .map(|i| front[i] as u32 + back[i] as u32)
        .collect();
    depths.sort_unstable();
    let median_depth = depths
        .get(depths.len() / 2)
        .map_or(0.0, |&d| d as f64 / UNIT as f64);
    let tier_len = m(scale, TIER_M).max(6.0);
    let tiers = if median_depth < tier_len * 1.5 {
        1
    } else if median_depth < tier_len * 2.6 {
        2
    } else {
        3
    };
    let blocks = |metres: f64| (m(scale, metres).round() as i32).max(1);

    // Heights mapped on the outline or the stands, when they fit what the depth holds.
    let tag_top = std::iter::once(input.tags)
        .chain(input.stand_elems.iter().map(|s| s.tags()))
        .filter_map(|t| metres(t, "height"))
        .map(|h| m(scale, h))
        .filter(|&h| h >= m(scale, 4.0) && h >= median_depth * 0.3)
        .fold(None, |a: Option<f64>, h| Some(a.map_or(h, |a| a.max(h))));
    let mut cap = blocks(SEATS_CAP_M);
    if let Some(t) = tag_top {
        cap = cap.min(t.round() as i32);
    }
    let profile = Profile {
        lip: blocks(LIP_M),
        tier_len: tier_len as f32,
        walkway: (m(scale, WALKWAY_M) as f32).max(1.0),
        jump: blocks(TIER_JUMP_M).max(2),
        tiers,
        cap: cap.max(2),
    };
    let seats_top = depths
        .last()
        .map_or(profile.lip, |&d| profile.at(d as f32 / UNIT).0);

    // Roof from mapped roof parts, covered=yes, a tall outline, or a big football stand.
    let mut roof_mask = vec![false; grid.len()];
    for o in others
        .iter()
        .filter(|o| !claimed.contains(&o.key()) && is_roofish(o.tags()))
    {
        if grid.bbox().contains(o.shape.centroid.0, o.shape.centroid.1) {
            rasterize(&grid, &o.shape, &mut roof_mask);
        }
    }
    let roofed_cells = (0..grid.len())
        .filter(|&i| stand[i] && roof_mask[i])
        .count();
    let mapped_cover = roofed_cells as f32 / stand_count.max(1) as f32;
    let covered = |t: &HashMap<String, String>| {
        tag(t, "covered") == Some("yes")
            || tag(t, "roof") == Some("yes")
            || t.get("roof:shape")
                .is_some_and(|s| s != "flat" && s != "no")
    };
    let roof_top = tag_top
        .map(|t| t.round() as i32)
        .filter(|&t| t >= seats_top + blocks(6.0));
    let roof_cover = if roof_top.is_some() {
        0.95
    } else if mapped_cover >= 0.08 {
        (mapped_cover * 1.2).min(0.95)
    } else if covered(input.tags) || input.stand_elems.iter().any(|s| covered(s.tags())) {
        0.75
    } else if input.sport.is_none_or(Sport::usually_roofed) && seats_top >= blocks(14.0) {
        0.65
    } else {
        0.0
    };

    let roof_min_depth = depths
        .get(depths.len() * 3 / 4)
        .map_or(0.0, |&d| d as f32 / UNIT * 0.5)
        .max(m(scale, 12.0) as f32);
    let front_radius = {
        let (cx, cz) = centre;
        let r: Vec<f64> = (0..grid.len())
            .filter(|&i| front_seed[i] && stand[i])
            .map(|i| {
                let (x, z) = grid.xz(i);
                (x as f64 - cx).hypot(z as f64 - cz)
            })
            .collect();
        if r.is_empty() {
            1.0
        } else {
            r.iter().sum::<f64>() / r.len() as f64
        }
    };

    Some(Venue {
        name: tags.get("name").cloned(),
        grid,
        stand,
        field,
        front,
        back,
        centre,
        field_frame,
        synth_field,
        profile,
        roof_cover,
        roof_top,
        roof_min_depth,
        roof_clearance: blocks(ROOF_CLEARANCE_M).max(3),
        mast_above: blocks(MAST_ABOVE_M).max(8),
        palette: palette_for(key.1, tags),
        front_radius: front_radius.max(1.0),
    })
}

/// Front-row and back-wall cells, by whether a neighbour is nearer or further from the field.
fn edges(
    grid: &Grid,
    stand: &[bool],
    field: &[bool],
    field_dist: &[u16],
) -> (Vec<bool>, Vec<bool>) {
    let mut front = vec![false; grid.len()];
    let mut back = vec![false; grid.len()];
    for i in 0..grid.len() {
        if !stand[i] {
            continue;
        }
        let (x, z) = grid.xz(i);
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            match grid.idx(x + dx, z + dz) {
                None => back[i] = true,
                Some(n) if stand[n] => {}
                Some(n) if field[n] || field_dist[n].saturating_add(3) <= field_dist[i] => {
                    front[i] = true
                }
                Some(n) if field_dist[n] >= field_dist[i].saturating_add(3) => back[i] = true,
                Some(_) => {}
            }
        }
    }
    (front, back)
}

/// Buildings standing mostly on the venue's stands or field, which it draws over.
fn covered_buildings(venue: &Venue, others: &[Candidate], claimed: &HashSet<Key>) -> Vec<Key> {
    let area = venue.grid.bbox();
    others
        .iter()
        .filter(|o| !claimed.contains(&o.key()))
        .filter(|o| {
            let b = o.shape.bbox;
            b.max_x >= area.min_x
                && b.min_x <= area.max_x
                && b.max_z >= area.min_z
                && b.min_z <= area.max_z
        })
        .filter(|o| venue_share(venue, &o.shape) >= 0.3)
        .map(|o| o.key())
        .collect()
}

/// How much of a footprint lies on the venue's stands or field.
fn venue_share(venue: &Venue, shape: &Shape) -> f64 {
    let local = Grid::of(shape.bbox);
    if local.len() > MAX_GRID_CELLS {
        return 0.0;
    }
    let mut mask = vec![false; local.len()];
    rasterize(&local, shape, &mut mask);
    let (mut on, mut all) = (0usize, 0usize);
    for (i, _) in mask.iter().enumerate().filter(|(_, &m)| m) {
        all += 1;
        let (x, z) = local.xz(i);
        if let Some(j) = venue.grid.idx(x, z) {
            on += (venue.stand[j] || venue.field[j]) as usize;
        }
    }
    if all == 0 {
        0.0
    } else {
        on as f64 / all as f64
    }
}

/// Places `indices` (into the plan's venues, ascending).
pub fn place_stadiums(editor: &mut WorldEditor, plan: &StadiumPlan, indices: &[usize]) {
    if indices.is_empty() {
        return;
    }
    println!(
        "{} Building {} sports venue{}...",
        "  [+]".bold(),
        indices.len(),
        if indices.len() == 1 { "" } else { "s" }
    );
    for venue in indices.iter().filter_map(|&i| plan.venues.get(i)) {
        place_venue(editor, venue);
    }
}

fn place_venue(editor: &mut WorldEditor, v: &Venue) {
    let g = &v.grid;
    let base = field_level(editor, v);

    let top_guess = base + v.profile.cap + 30;
    // A synthesized field is unsealed ground, so land-cover trees may stand on it.
    let synth = v.synth_field.is_some();
    let cleared: Vec<bool> = v
        .stand
        .iter()
        .zip(&v.field)
        .map(|(&s, &f)| s || (synth && f))
        .collect();
    crate::models_3d::clear_trees_in_mask(editor, g.x0, g.z0, g.w, g.h, &cleared, top_guess);

    if let Some(sf) = &v.synth_field {
        paint_field(editor, sf);
    }

    let (cx, cz) = v.centre;
    let p = &v.profile;
    let roof_y = v.roof_top.map(|t| base + t).unwrap_or_else(|| {
        let top = v
            .stand_cells()
            .map(|i| p.at(v.front[i] as f32 / UNIT).0)
            .max()
            .unwrap_or(p.lip);
        base + top + v.roof_clearance
    });
    let sections = (std::f64::consts::TAU * v.front_radius / AISLE_SPACING)
        .round()
        .max(4.0);

    for i in v.stand_cells() {
        let (x, z) = g.xz(i);
        let f = v.front[i] as f32 / UNIT;
        let b = v.back[i] as f32 / UNIT;
        let (h, tier, walkway) = p.at(f);
        let ground = editor.get_ground_level(x, z);
        let seat_y = base + h;
        let (dx, dz) = (x as f64 - cx, z as f64 - cz);
        let angle = dz.atan2(dx);
        let turn = (angle / std::f64::consts::TAU).rem_euclid(1.0) * sections;
        let aisle = turn.fract() < 1.0 / AISLE_SPACING;
        let roof_from = (1.0 - v.roof_cover) * (f + b);
        let roofed = v.roof_cover > 0.0 && f >= roof_from && f + b >= v.roof_min_depth;

        let mut put = |y: i32, block: Block| {
            editor.set_block_absolute(block, x, y, z, None, Some(&[]));
        };
        // Rows on a hillside are cut into it, as bowls built into slopes are.
        for y in seat_y + 1..=ground + 2 {
            put(y, AIR);
        }
        if b < 1.0 {
            // Back wall up to the roof, a parapet, or the hillside it holds back.
            let top = if roofed { roof_y } else { seat_y + 1 }.max(ground);
            let along = (angle * dx.hypot(dz)).round() as i64;
            for y in ground.min(seat_y) + 1..=top {
                if y == seat_y {
                    continue;
                }
                put(y, facade_block(v, along, y - ground.min(seat_y)));
            }
        } else {
            // The face of an upper tier over the walkway below is a band of boxes.
            let boxes = !walkway && tier > 0 && p.at((f - 1.0).max(0.0)).2;
            let walk_top = base + p.at((f - 1.0).max(0.0)).0;
            let fill = if f < 1.0 {
                v.palette.accent
            } else {
                v.palette.wall
            };
            for y in ground + 1..seat_y {
                let block = if boxes && y > walk_top {
                    if (y - walk_top) % 3 == 0 {
                        LIGHT_GRAY_CONCRETE
                    } else {
                        GRAY_STAINED_GLASS
                    }
                } else {
                    fill
                };
                put(y, block);
            }
        }
        put(
            seat_y,
            if walkway || aisle {
                LIGHT_GRAY_CONCRETE
            } else {
                v.palette.seat
            },
        );

        if roofed {
            let block = if f < roof_from + 1.5 {
                LIGHT_GRAY_CONCRETE
            } else if f < roof_from + 6.0 {
                WHITE_STAINED_GLASS
            } else {
                WHITE_CONCRETE
            };
            put(roof_y, block);
            if f < roof_from + 1.5 && coord_hash(x, z).is_multiple_of(5) {
                put(roof_y - 1, SEA_LANTERN);
            }
        }
    }

    if v.roof_cover < 0.3 {
        place_masts(editor, v, base);
    }
    let blocks = v.stand_cells().count();
    println!(
        "  {} ({} stand columns{})",
        v.name.as_deref().unwrap_or("Sports venue"),
        blocks,
        if v.roof_cover > 0.0 { ", roofed" } else { "" }
    );
}

fn facade_block(v: &Venue, along: i64, rel: i32) -> Block {
    if along.rem_euclid(6) == 0 {
        v.palette.rib
    } else if rel > 2 && rel % 6 == 0 {
        GRAY_STAINED_GLASS
    } else {
        v.palette.wall
    }
}

/// The field's typical ground height, so the stands rise from one level all round.
fn field_level(editor: &WorldEditor, v: &Venue) -> i32 {
    let g = &v.grid;
    let step = (g.len() / 4000).max(1);
    let mut levels: Vec<i32> = (0..g.len())
        .step_by(step)
        .filter(|&i| v.field[i])
        .map(|i| {
            let (x, z) = g.xz(i);
            editor.get_ground_level(x, z)
        })
        .collect();
    if levels.is_empty() {
        let (x, z) = (v.centre.0 as i32, v.centre.1 as i32);
        return editor.get_ground_level(x, z);
    }
    levels.sort_unstable();
    levels[levels.len() / 2]
}

fn paint_field(editor: &mut WorldEditor, sf: &SynthField) {
    for &(x, z) in &sf.cells {
        editor.set_block(GRASS_BLOCK, x, 0, z, None, Some(&[]));
        editor.set_block(
            AIR,
            x,
            1,
            z,
            Some(crate::ground_decoration::LOOSE_PLANTS),
            None,
        );
    }
    sport_pitches::draw_pitch_markings(editor, &sf.way, &sf.cells, GRASS_BLOCK);
}

/// Four light masts behind the field's corners, standing on the back of the stands.
fn place_masts(editor: &mut WorldEditor, v: &Venue, base: i32) {
    let Some(fr) = v.field_frame else {
        return;
    };
    let g = &v.grid;
    let (vx, vz) = (-fr.uz, fr.ux);
    let p = &v.profile;
    let top_seat = v
        .stand_cells()
        .map(|i| p.at(v.front[i] as f32 / UNIT).0)
        .max()
        .unwrap_or(0);
    let mast_top = base + top_seat + v.mast_above;
    for (sa, sb) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let (tx, tz) = (
            fr.cx + sa * fr.half_len * fr.ux + sb * fr.half_wid * vx,
            fr.cz + sa * fr.half_len * fr.uz + sb * fr.half_wid * vz,
        );
        let (dx, dz) = (tx - fr.cx, tz - fr.cz);
        let len = dx.hypot(dz).max(1.0);
        let (dx, dz) = (dx / len, dz / len);
        // The last stand cell along the diagonal, else open ground past the corner.
        let mut spot: Option<(i32, i32)> = None;
        let mut t = len;
        // Out to the edge of the grid, which holds every stand.
        let far = len + (g.w + g.h) as f64;
        while t < far {
            let (x, z) = (
                (fr.cx + dx * t).round() as i32,
                (fr.cz + dz * t).round() as i32,
            );
            match g.idx(x, z) {
                Some(i) if v.stand[i] => spot = Some((x, z)),
                Some(_) => {}
                None => break,
            }
            t += 1.0;
        }
        let (x, z) = spot.unwrap_or((
            (tx + dx * 8.0).round() as i32,
            (tz + dz * 8.0).round() as i32,
        ));
        if g.idx(x, z).is_none() {
            continue;
        }
        let from = editor.get_ground_level(x, z) + 1;
        for y in from..mast_top {
            if editor.get_block_absolute(x, y, z).is_some_and(|b| b != AIR) {
                continue;
            }
            editor.set_block_absolute(LIGHT_GRAY_CONCRETE, x, y, z, None, Some(&[]));
        }
        // A lamp bank facing the field, across the line of sight.
        let (px, pz) = (-dz, dx);
        for k in -2..=2 {
            for dy in 0..3 {
                let (lx, lz) = (
                    (x as f64 + px * k as f64 - dx).round() as i32,
                    (z as f64 + pz * k as f64 - dz).round() as i32,
                );
                let block = if dy == 1 || k == 0 {
                    SEA_LANTERN
                } else {
                    LIGHT_GRAY_CONCRETE
                };
                editor.set_block_absolute(block, lx, mast_top + dy, lz, None, Some(&[]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_shape(x0: i32, z0: i32, x1: i32, z1: i32) -> Shape {
        Shape {
            rings: vec![vec![(x0, z0), (x1, z0), (x1, z1), (x0, z1), (x0, z0)]],
            outer: vec![true],
            bbox: Bbox {
                min_x: x0,
                min_z: z0,
                max_x: x1,
                max_z: z1,
            },
            centroid: ((x0 + x1) / 2, (z0 + z1) / 2),
            area: ((x1 - x0) * (z1 - z0)) as f64,
            has_hole: false,
        }
    }

    #[test]
    fn rasterize_fills_rectangle_with_its_edges() {
        let grid = Grid::of(Bbox {
            min_x: 0,
            min_z: 0,
            max_x: 20,
            max_z: 20,
        });
        let mut out = vec![false; grid.len()];
        rasterize(&grid, &rect_shape(5, 5, 10, 8), &mut out);
        let count = out.iter().filter(|&&b| b).count();
        assert_eq!(count, 6 * 4);
        assert!(out[grid.idx(5, 5).unwrap()]);
        assert!(out[grid.idx(10, 8).unwrap()]);
        assert!(!out[grid.idx(11, 8).unwrap()]);
    }

    #[test]
    fn ring_with_hole_leaves_the_hole_open() {
        let grid = Grid::of(Bbox {
            min_x: 0,
            min_z: 0,
            max_x: 40,
            max_z: 40,
        });
        let mut shape = rect_shape(0, 0, 40, 40);
        shape
            .rings
            .push(vec![(10, 10), (30, 10), (30, 30), (10, 30), (10, 10)]);
        let mut out = vec![false; grid.len()];
        rasterize(&grid, &shape, &mut out);
        assert!(out[grid.idx(5, 20).unwrap()]);
        assert!(!out[grid.idx(20, 20).unwrap()]);
        assert!(out[grid.idx(10, 20).unwrap()], "inner outline is wall");
    }

    #[test]
    fn profile_rises_half_a_block_per_block_then_steps_up_a_tier() {
        let p = Profile {
            lip: 2,
            tier_len: 20.0,
            walkway: 2.0,
            jump: 3,
            tiers: 2,
            cap: 100,
        };
        assert_eq!(p.at(0.0), (2, 0, false));
        assert_eq!(p.at(4.0), (4, 0, false));
        assert_eq!(p.at(20.5), (12, 0, true));
        let (h, tier, walk) = p.at(22.0);
        assert_eq!((h, tier, walk), (15, 1, false));
        assert!(p.at(40.0).0 > p.at(30.0).0);
    }

    #[test]
    fn profile_never_exceeds_its_cap() {
        let p = Profile {
            lip: 1,
            tier_len: 20.0,
            walkway: 2.0,
            jump: 3,
            tiers: 3,
            cap: 18,
        };
        for f in 0..200 {
            assert!(p.at(f as f32).0 <= 18);
        }
    }

    #[test]
    fn stand_fronts_face_the_field() {
        // Field in the middle, a stand ring around it.
        let grid = Grid::of(Bbox {
            min_x: 0,
            min_z: 0,
            max_x: 60,
            max_z: 60,
        });
        let mut field = vec![false; grid.len()];
        rasterize(&grid, &rect_shape(20, 20, 40, 40), &mut field);
        let mut stand = vec![false; grid.len()];
        let mut ring = rect_shape(5, 5, 55, 55);
        ring.rings
            .push(vec![(15, 15), (45, 15), (45, 45), (15, 45), (15, 15)]);
        rasterize(&grid, &ring, &mut stand);
        let dist = chamfer(&grid, |i| field[i]);
        let (front, back) = edges(&grid, &stand, &field, &dist);
        assert!(front[grid.idx(15, 30).unwrap()]);
        assert!(back[grid.idx(5, 30).unwrap()]);
        assert!(!front[grid.idx(5, 30).unwrap()]);
        let f = chamfer(&grid, |i| front[i]);
        assert!(f[grid.idx(8, 30).unwrap()] > f[grid.idx(12, 30).unwrap()]);
    }

    fn way(id: u64, pts: &[(i32, i32)], tags: &[(&str, &str)]) -> ProcessedElement {
        let mut nodes: Vec<ProcessedNode> = pts
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: id * 100 + i as u64,
                tags: HashMap::new(),
                x,
                z,
            })
            .collect();
        nodes.push(nodes[0].clone());
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    fn rect(x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<(i32, i32)> {
        vec![(x0, z0), (x1, z0), (x1, z1), (x0, z1)]
    }

    fn args() -> Args {
        use clap::Parser;
        Args::parse_from(["arnis", "--bbox", "1,2,3,4"])
    }

    fn plan(elements: &[ProcessedElement]) -> StadiumPlan {
        prescan(elements, &args(), &HashSet::new())
    }

    #[test]
    fn arena_outline_stays_a_building() {
        let els = [way(
            1,
            &rect(0, 0, 160, 120),
            &[
                ("leisure", "stadium"),
                ("building", "yes"),
                ("sport", "basketball"),
            ],
        )];
        let p = plan(&els);
        assert_eq!(p.placement_count(), 0);
        assert!(p.suppressed().is_empty());
    }

    #[test]
    fn venue_under_an_arena_building_is_left_to_it() {
        let els = [
            way(
                1,
                &rect(0, 0, 160, 140),
                &[("leisure", "stadium"), ("sport", "baseball")],
            ),
            way(
                2,
                &rect(30, 30, 130, 110),
                &[("leisure", "pitch"), ("sport", "baseball")],
            ),
            way(3, &rect(2, 2, 158, 138), &[("building", "yes")]),
        ];
        assert_eq!(plan(&els).placement_count(), 0);
        let dome = [way(
            1,
            &rect(0, 0, 160, 140),
            &[
                ("leisure", "stadium"),
                ("roof:shape", "dome"),
                ("height", "56"),
            ],
        )];
        assert_eq!(plan(&dome).placement_count(), 0);
    }

    #[test]
    fn indoor_sport_venue_without_a_field_is_left_alone() {
        let els = [way(
            1,
            &rect(0, 0, 160, 120),
            &[("leisure", "stadium"), ("sport", "ice_hockey")],
        )];
        assert_eq!(plan(&els).placement_count(), 0);
    }

    #[test]
    fn open_venue_seats_the_ring_round_its_pitch_and_takes_its_parts() {
        let els = [
            way(
                1,
                &rect(0, 0, 180, 140),
                &[("leisure", "stadium"), ("sport", "soccer")],
            ),
            way(
                2,
                &rect(38, 36, 142, 104),
                &[("leisure", "pitch"), ("sport", "soccer")],
            ),
            way(
                3,
                &rect(4, 4, 30, 136),
                &[("building:part", "yes"), ("height", "20")],
            ),
            // A clubhouse in the ring keeps its own building.
            way(4, &rect(150, 60, 170, 80), &[("building", "yes")]),
        ];
        let p = plan(&els);
        assert_eq!(p.placement_count(), 1);
        let v = &p.venues[0];
        let at = |x: i32, z: i32| v.grid.idx(x, z).is_some_and(|i| v.stand[i]);
        assert!(at(20, 70), "stand west of the pitch");
        assert!(!at(90, 70), "pitch stays open");
        assert!(!at(160, 70), "clubhouse is not seated over");
        assert!(p.suppressed().contains(&("way", 3)));
        assert!(!p.suppressed().contains(&("way", 4)));
        assert!(
            !p.suppressed().contains(&("way", 2)),
            "the pitch draws itself"
        );
        // Rows climb away from the field.
        let h = |x: i32| {
            v.profile
                .at(v.front[v.grid.idx(x, 70).unwrap()] as f32 / UNIT)
                .0
        };
        assert!(h(10) > h(28));
    }

    #[test]
    fn nested_outline_round_the_same_pitch_builds_one_venue() {
        let els = [
            way(1, &rect(0, 0, 220, 180), &[("leisure", "stadium")]),
            way(2, &rect(20, 20, 200, 160), &[("leisure", "stadium")]),
            way(
                3,
                &rect(58, 56, 162, 124),
                &[("leisure", "pitch"), ("sport", "soccer")],
            ),
        ];
        assert_eq!(plan(&els).placement_count(), 1);
    }

    #[test]
    fn stadium_ring_with_a_hole_is_its_own_venue() {
        let outer = rect(0, 0, 200, 160);
        let inner = rect(40, 40, 160, 120);
        let ring = ProcessedElement::Relation(crate::osm_parser::ProcessedRelation {
            id: 9,
            tags: [("building", "stadium"), ("type", "multipolygon")]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            members: [
                (outer, ProcessedMemberRole::Outer),
                (inner, ProcessedMemberRole::Inner),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (pts, role))| {
                let ProcessedElement::Way(w) = way(10 + i as u64, &pts, &[]) else {
                    unreachable!()
                };
                crate::osm_parser::ProcessedMember {
                    role,
                    way: std::sync::Arc::new(w),
                }
            })
            .collect(),
        });
        let p = plan(&[ring]);
        assert_eq!(p.placement_count(), 1);
        assert!(p.suppressed().contains(&("relation", 9)));
    }

    #[test]
    fn lone_grandstand_faces_the_pitch_beside_it() {
        let els = [
            way(
                1,
                &rect(0, 0, 100, 60),
                &[("leisure", "pitch"), ("sport", "soccer")],
            ),
            way(2, &rect(0, 66, 100, 80), &[("building", "grandstand")]),
        ];
        let p = plan(&els);
        assert_eq!(p.placement_count(), 1);
        assert!(p.suppressed().contains(&("way", 2)));
        let v = &p.venues[0];
        let f = |z: i32| v.front[v.grid.idx(50, z).unwrap()];
        assert!(f(78) > f(68), "rows rise away from the pitch");
    }

    #[test]
    fn rooftop_bleachers_stay_on_their_building() {
        let els = [
            way(
                1,
                &rect(0, 0, 100, 60),
                &[("leisure", "pitch"), ("sport", "baseball")],
            ),
            way(2, &rect(0, 70, 40, 90), &[("building", "apartments")]),
            way(
                3,
                &rect(5, 72, 35, 80),
                &[("leisure", "bleachers"), ("layer", "2")],
            ),
        ];
        assert_eq!(plan(&els).placement_count(), 0);
    }

    #[test]
    fn grandstand_with_nothing_to_watch_stays_a_building() {
        let els = [way(2, &rect(0, 0, 100, 14), &[("building", "grandstand")])];
        assert_eq!(plan(&els).placement_count(), 0);
    }

    #[test]
    fn sports_parse_from_lists() {
        let mut t = HashMap::new();
        t.insert("sport".to_string(), "multi;baseball".to_string());
        assert_eq!(sport_of(&t), Some(Sport::Baseball));
        t.insert("sport".to_string(), "ice_hockey".to_string());
        assert_eq!(sport_of(&t), Some(Sport::Indoor));
        t.insert("sport".to_string(), "multi".to_string());
        assert_eq!(sport_of(&t), None);
        t.insert(
            "sport".to_string(),
            "football;american_football".to_string(),
        );
        assert_eq!(sport_of(&t), Some(Sport::AmericanFootball));
        t.insert("sport".to_string(), "football".to_string());
        assert_eq!(sport_of(&t), Some(Sport::Soccer));
    }
}
