//! Cemeteries: graves in rows along each grave field's outline, lawn, paths, trees and a wall.

use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::decals::region::SignRegion;
use crate::element_processing::area_layout::{
    bounds, cell_hash, inside, interior_point, point_in_rings, AreaMask, Frame,
};
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::bush::{place_bush, BushKind};
use crate::element_processing::connected_blocks::{connected_wall, four_connected_line};
use crate::element_processing::landuse::ground_is_protected;
use crate::element_processing::tree::Tree;
use crate::floodfill_cache::{
    in_hole, BuildingFootprintBitmap, FloodFillCache, Holes, RoadMaskBitmap,
};
use crate::land_cover::coord_hash;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use crate::structures::{tombstone, Prop};
use crate::world_editor::WorldEditor;
use std::collections::{HashMap, HashSet};

const STYLE_TAG: &str = "arnis:cemetery";
const SECTORS_TAG: &str = "arnis:sectors";

/// Grave fields: whole cemeteries, churchyards and the sectors mapped inside them.
pub fn claims(tags: &HashMap<String, String>) -> bool {
    is_cemetery(tags) || is_sector(tags)
}

fn is_cemetery(tags: &HashMap<String, String>) -> bool {
    tags.get("landuse").map(String::as_str) == Some("cemetery")
        || tags.get("amenity").map(String::as_str) == Some("grave_yard")
}

fn is_sector(tags: &HashMap<String, String>) -> bool {
    tags.get("cemetery").map(String::as_str) == Some("sector")
}

fn is_war(tags: &HashMap<String, String>) -> bool {
    matches!(
        tags.get("cemetery").map(String::as_str),
        Some("war_cemetery" | "military")
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Style {
    /// Uniform white headstones in tight rows on a lawn.
    War,
    /// Headstones on a lawn, as in North America, Britain and Australia.
    Lawn,
    /// Planted and gravelled grave beds with paths between, as in continental Europe.
    Beds,
}

impl Style {
    fn of(tags: &HashMap<String, String>) -> Self {
        match tags.get(STYLE_TAG).map(String::as_str) {
            Some("war") => Style::War,
            Some("lawn") => Style::Lawn,
            Some("beds") => Style::Beds,
            _ if is_war(tags) => Style::War,
            _ => Style::Beds,
        }
    }

    /// Spacing along a row, spacing of rows, columns a grave takes, percent of places used.
    fn lattice(self) -> (f64, f64, i32, u64) {
        match self {
            Style::War => (2.0, 3.0, 1, 94),
            Style::Lawn => (3.0, 4.0, 2, 72),
            Style::Beds => unreachable!("bed graves are laid out by `Beds`"),
        }
    }
}

/// Tags grave fields with their style, and cemeteries with mapped sectors, which then lay no graves.
pub fn prepare(elements: &mut [ProcessedElement], region: SignRegion) {
    let regional = match region {
        SignRegion::NorthAmerica
        | SignRegion::Canada
        | SignRegion::Oceania
        | SignRegion::UkIreland => "lawn",
        _ => "beds",
    };
    let mut sectors: Vec<(usize, (f64, f64))> = Vec::new();
    for (i, e) in elements.iter().enumerate() {
        if let ProcessedElement::Way(w) = e {
            if is_sector(&w.tags) && !is_cemetery(&w.tags) && w.nodes.len() >= 3 {
                if let Some(p) = interior_point(&w.nodes) {
                    sectors.push((i, p));
                }
            }
        }
    }
    let mut containers: Vec<usize> = Vec::new();
    let mut war_sectors: Vec<usize> = Vec::new();
    if !sectors.is_empty() {
        for (i, e) in elements.iter().enumerate() {
            let (tags, rings): (_, Vec<&[ProcessedNode]>) = match e {
                ProcessedElement::Way(w) if is_cemetery(&w.tags) => {
                    (&w.tags, vec![w.nodes.as_slice()])
                }
                ProcessedElement::Relation(r) if is_cemetery(&r.tags) => (
                    &r.tags,
                    r.members
                        .iter()
                        .filter(|m| m.role == ProcessedMemberRole::Outer)
                        .map(|m| m.way.nodes.as_slice())
                        .collect(),
                ),
                _ => continue,
            };
            let boxes: Vec<_> = rings.iter().filter_map(|r| bounds(r)).collect();
            let inside: Vec<usize> = sectors
                .iter()
                .filter(|&&(j, c)| {
                    j != i && boxes.iter().any(|&b| inside(b, c)) && point_in_rings(&rings, c)
                })
                .map(|&(j, _)| j)
                .collect();
            if !inside.is_empty() {
                containers.push(i);
                if is_war(tags) {
                    war_sectors.extend(inside);
                }
            }
        }
    }
    for e in elements.iter_mut() {
        let tags = match e {
            ProcessedElement::Way(w) => &mut w.tags,
            ProcessedElement::Relation(r) => &mut r.tags,
            ProcessedElement::Node(_) => continue,
        };
        if claims(tags) {
            let style = if is_war(tags) { "war" } else { regional };
            tags.insert(STYLE_TAG.to_string(), style.to_string());
        }
    }
    let mut mark = |i: usize, key: &str, value: &str| match &mut elements[i] {
        ProcessedElement::Way(w) => {
            w.tags.insert(key.to_string(), value.to_string());
        }
        ProcessedElement::Relation(r) => {
            r.tags.insert(key.to_string(), value.to_string());
        }
        ProcessedElement::Node(_) => {}
    };
    for i in containers {
        mark(i, SECTORS_TAG, "yes");
    }
    for i in war_sectors {
        mark(i, STYLE_TAG, "war");
    }
}

pub fn generate_cemetery(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    road_mask: &RoadMaskBitmap,
    bridge_surface: &BridgeSurfaceMap,
) {
    cemetery_area(
        editor,
        way,
        args,
        flood_fill_cache,
        building_footprints,
        road_mask,
        bridge_surface,
        None,
    );
}

/// One grave field, or one outer ring of a cemetery multipolygon less its holes.
#[allow(clippy::too_many_arguments)]
pub fn cemetery_area(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    road_mask: &RoadMaskBitmap,
    bridge_surface: &BridgeSurfaceMap,
    holes: Option<&Holes>,
) {
    let fill = flood_fill_cache.get_or_compute(way, args.timeout.as_ref());
    if fill.is_empty() {
        return;
    }
    let style = Style::of(&way.tags);
    let has_sectors = way.tags.get(SECTORS_TAG).is_some_and(|v| v == "yes");
    let plan = Frame::of(&way.nodes).filter(|_| !has_sectors).map(|frame| {
        let mut paths = false;
        let mask = AreaMask::new(&fill, |x, z| {
            paths |= road_mask.contains(x, z);
            !in_hole(holes, x, z)
        });
        if style == Style::Beds {
            Plan::Beds(Beds::new(way.id, &frame, mask, !paths))
        } else {
            Plan::Rows(Rows::new(way.id, style, frame, mask))
        }
    });

    for &(x, z) in fill.iter() {
        if in_hole(holes, x, z) || !editor.in_bounds(x, z) || ground_is_protected(editor, x, z) {
            continue;
        }
        let path = matches!(&plan, Some(Plan::Beds(b)) if b.on_path(x, z));
        let ground = if path { GRAVEL } else { GRASS_BLOCK };
        editor.set_block(ground, x, 0, z, None, None);
    }

    match &plan {
        Some(Plan::Rows(rows)) => rows.place(editor, road_mask, building_footprints),
        Some(Plan::Beds(beds)) => beds.place(editor, road_mask, building_footprints),
        None => {}
    }

    // Trees, shrubs and flowers between the graves; positional, so every tile agrees.
    for &(x, z) in fill.iter() {
        if in_hole(holes, x, z) || !editor.owns(x, z) || editor.is_lc_water(x, z) {
            continue;
        }
        if style != Style::War && !has_sectors {
            tombstone::maybe_place_crypt(editor, x, z, road_mask);
        }
        if !editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK]))
            || editor.block_exists_absolute(x, editor.get_absolute_y(x, 1, z), z)
        {
            continue;
        }
        let roll = coord_hash(x ^ 0x6CE7, z) % 1000;
        let on_grid = x.rem_euclid(3) == 0 && z.rem_euclid(3) == 0;
        if on_grid && roll < 30 {
            if editor.land_cover_backs_trees(x, z) && clear_around(editor, x, z) {
                Tree::create(
                    editor,
                    (x, 1, z),
                    Some(building_footprints),
                    Some(bridge_surface),
                );
            }
        } else if style != Style::War && (30..34).contains(&roll) && clear_around(editor, x, z) {
            place_bush(editor, x, z, BushKind::Garden);
        } else if style != Style::War && (34..60).contains(&roll) {
            editor.set_block(GRASS, x, 1, z, None, None);
        } else if style == Style::Lawn && (60..64).contains(&roll) {
            crate::ground_decoration::place_scattered_flower(
                editor,
                x,
                z,
                crate::ground_decoration::FlowerSetting::Meadow,
            );
        }
    }

    if is_cemetery(&way.tags) {
        generate_wall(editor, way, road_mask, building_footprints);
    }
}

/// No grave or other plant beside the cell, so a tree or shrub keeps clear of the stones.
fn clear_around(editor: &WorldEditor, x: i32, z: i32) -> bool {
    [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().all(|&(dx, dz)| {
        let y = editor.get_absolute_y(x + dx, 1, z + dz);
        !editor.block_exists_absolute(x + dx, y, z + dz)
    })
}

/// A stone-brick wall with a slab cap along the outline, open where a road or path enters and
/// stopping at a church or other building on the line.
fn generate_wall(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    road_mask: &RoadMaskBitmap,
    footprints: &BuildingFootprintBitmap,
) {
    let line: Vec<(i32, i32)> = way
        .nodes
        .windows(2)
        .flat_map(|w| bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z))
        .map(|(x, _, z)| (x, z))
        .collect();
    let wall: HashSet<(i32, i32)> = four_connected_line(&line)
        .into_iter()
        .filter(|&(x, z)| !road_mask.contains(x, z) && !footprints.contains(x, z))
        .collect();
    for &(x, z) in &wall {
        if !editor.in_bounds(x, z) {
            continue;
        }
        let ground = editor.get_ground_level(x, z);
        let joins = |dx: i32, dz: i32| {
            wall.contains(&(x + dx, z + dz)) && editor.get_ground_level(x + dx, z + dz) == ground
        };
        let block = connected_wall(
            STONE_BRICK_WALL,
            joins(0, -1),
            joins(0, 1),
            joins(1, 0),
            joins(-1, 0),
        );
        let y = editor.get_absolute_y(x, 1, z);
        if !editor.block_exists_absolute(x, y, z) {
            editor.set_block_with_properties_absolute(block, x, y, z, None, None);
        }
        editor.set_block(STONE_BRICK_SLAB, x, 2, z, None, None);
    }
}

enum Plan {
    Rows(Rows),
    Beds(Beds),
}

/// Whether a grave may take column (x, z).
fn free(
    editor: &WorldEditor,
    mask: &AreaMask,
    road_mask: &RoadMaskBitmap,
    footprints: &BuildingFootprintBitmap,
    (x, z): (i32, i32),
    crypts: bool,
) -> bool {
    mask.contains(x, z)
        && !road_mask.contains(x, z)
        && !footprints.contains(x, z)
        && !editor.surface_is_sealed(x, z)
        && !editor.is_lc_water(x, z)
        && !(crypts && editor.place_prop(Prop::Tombstone) && tombstone::near_crypt(x, z))
}

/// Grave models in straight rows on the world axes, each field keeping its own row phase.
/// A row is pairs of 3-long graves, or one 4x3 cross grave in place of a pair.
struct Beds {
    id: u64,
    mask: AreaMask,
    origin: (i32, i32),
    /// Unit steps along a row and from a grave's head to its foot.
    along: (i32, i32),
    dir: (i32, i32),
    /// Gravel paths cut through bigger fields where none are mapped.
    sections: bool,
}

/// Width of a pair of graves, and the distance between rows.
const PAIR: i32 = 4;
const ROW: i32 = 5;
/// Rows between paths, and pairs between cross paths.
const ROWS_PER_SECTION: i32 = 8;
const PAIRS_PER_SECTION: i32 = 7;

impl Beds {
    fn new(id: u64, frame: &Frame, mask: AreaMask, unmapped: bool) -> Self {
        let a = frame.along_axis();
        let along = (a.0.abs(), a.1.abs());
        let dir = (along.1, along.0);
        Self {
            id,
            origin: mask.min(),
            sections: unmapped && frame.length().min(frame.width()) >= 48.0,
            mask,
            along,
            dir,
        }
    }

    fn at(&self, u: i32, v: i32) -> (i32, i32) {
        (
            self.origin.0 + u * self.along.0 + v * self.dir.0,
            self.origin.1 + u * self.along.1 + v * self.dir.1,
        )
    }

    fn uv(&self, x: i32, z: i32) -> (i32, i32) {
        let (dx, dz) = (x - self.origin.0, z - self.origin.1);
        (
            dx * self.along.0 + dz * self.along.1,
            dx * self.dir.0 + dz * self.dir.1,
        )
    }

    fn path_row(&self, r: i32) -> bool {
        self.sections && (r + 1).rem_euclid(ROWS_PER_SECTION) == 0
    }

    fn path_pair(&self, k: i32) -> bool {
        self.sections && (k + 1).rem_euclid(PAIRS_PER_SECTION) == 0
    }

    fn on_path(&self, x: i32, z: i32) -> bool {
        let (u, v) = self.uv(x, z);
        let (k, r) = (u.div_euclid(PAIR), v.div_euclid(ROW));
        (self.path_row(r) && (1..4).contains(&v.rem_euclid(ROW)))
            || (self.path_pair(k) && (1..3).contains(&u.rem_euclid(PAIR)))
    }

    /// Whether the cells u0..=u1 by v0..=v1 are all free and on one level.
    fn fits(
        &self,
        editor: &WorldEditor,
        road_mask: &RoadMaskBitmap,
        footprints: &BuildingFootprintBitmap,
        (u0, u1): (i32, i32),
        (v0, v1): (i32, i32),
    ) -> Option<bool> {
        let ground = editor.get_ground_level(self.at(u0, v0).0, self.at(u0, v0).1);
        let mut level = true;
        for u in u0..=u1 {
            for v in v0..=v1 {
                let c = self.at(u, v);
                if !free(editor, &self.mask, road_mask, footprints, c, true) {
                    return None;
                }
                level &= editor.get_ground_level(c.0, c.1) == ground;
            }
        }
        // Room to stand at the head.
        let front = self.at(u0, v0 - 1);
        (self.mask.contains(front.0, front.1) && !road_mask.contains(front.0, front.1))
            .then_some(level)
    }

    fn place(
        &self,
        editor: &mut WorldEditor,
        road_mask: &RoadMaskBitmap,
        footprints: &BuildingFootprintBitmap,
    ) {
        let (bx0, bz0, bx1, bz1) = editor.bounds();
        let (u_a, v_a) = self.uv(bx0, bz0);
        let (u_b, v_b) = self.uv(bx1, bz1);
        let (k0, k1) = (
            u_a.min(u_b).div_euclid(PAIR) - 1,
            u_a.max(u_b).div_euclid(PAIR) + 1,
        );
        let (r0, r1) = (
            v_a.min(v_b).div_euclid(ROW) - 1,
            v_a.max(v_b).div_euclid(ROW) + 1,
        );
        let (mu, mv) = self.uv(self.mask.max().0, self.mask.max().1);
        for r in r0.max(0)..=r1.min(mv.div_euclid(ROW) + 1) {
            if self.path_row(r) {
                continue;
            }
            let v0 = r * ROW;
            for k in k0.max(0)..=k1.min(mu.div_euclid(PAIR) + 1) {
                if self.path_pair(k) {
                    continue;
                }
                let u0 = k * PAIR;
                let h = cell_hash(self.id, k as i64, r as i64);
                // A cross grave over the whole pair, its anchor at the model's centre.
                if h % 100 < 16 {
                    if let Some(true) =
                        self.fits(editor, road_mask, footprints, (u0, u0 + 2), (v0, v0 + 3))
                    {
                        let anchor = self.at(u0 + 1, v0 + 2);
                        if editor.owns(anchor.0, anchor.1)
                            && tombstone::place_model(editor, true, anchor, self.dir, h >> 8)
                        {
                            continue;
                        }
                        if editor.place_prop(Prop::Tombstone) {
                            continue;
                        }
                    }
                }
                for (i, u) in [u0, u0 + 2].into_iter().enumerate() {
                    let g = cell_hash(self.id ^ 0x6E4E, (2 * k + i as i32) as i64, r as i64);
                    if g % 100 >= 86 {
                        continue;
                    }
                    let Some(level) =
                        self.fits(editor, road_mask, footprints, (u, u), (v0, v0 + 2))
                    else {
                        continue;
                    };
                    let anchor = self.at(u, v0 + 1);
                    if level && editor.place_prop(Prop::Tombstone) {
                        if editor.owns(anchor.0, anchor.1) {
                            tombstone::place_model(editor, false, anchor, self.dir, g >> 8);
                        }
                    } else {
                        self.bed(editor, self.at(u, v0), g >> 8);
                    }
                }
            }
        }
    }

    /// A drawn grave where no model fits: a headstone and a planted or gravelled bed.
    fn bed(&self, editor: &mut WorldEditor, head: (i32, i32), h: u64) {
        headstone(editor, head, self.dir, h, false);
        for k in 1..3 {
            let (x, z) = (head.0 + k * self.dir.0, head.1 + k * self.dir.1);
            if !editor.owns(x, z) {
                continue;
            }
            let over = Some(&[GRASS_BLOCK][..]);
            if (h >> 12).is_multiple_of(2) {
                editor.set_block(GRAVEL, x, 0, z, over, None);
            } else {
                editor.set_block(PODZOL, x, 0, z, over, None);
                editor.set_block(flower(h >> (k * 4)), x, 1, z, None, None);
            }
        }
    }
}

/// The lattice of graves over one grave field, laid along its outline.
struct Rows {
    id: u64,
    style: Style,
    frame: Frame,
    mask: AreaMask,
    slot: f64,
    row: f64,
    len: i32,
    occupancy: u64,
    dir: (i32, i32),
}

#[derive(Clone, Copy)]
struct Grave {
    head: (i32, i32),
    hash: u64,
}

impl Rows {
    fn new(id: u64, style: Style, frame: Frame, mask: AreaMask) -> Self {
        let (slot, row, len, occupancy) = style.lattice();
        Self {
            id,
            style,
            dir: frame.across_axis(),
            frame,
            mask,
            slot,
            row,
            len,
            occupancy,
        }
    }

    fn cells(&self, g: Grave) -> impl Iterator<Item = (i32, i32)> + '_ {
        (0..self.len).map(move |k| (g.head.0 + k * self.dir.0, g.head.1 + k * self.dir.1))
    }

    /// The grave at lattice point (s, r) if its place is used and all of it fits.
    fn grave(
        &self,
        editor: &WorldEditor,
        road_mask: &RoadMaskBitmap,
        footprints: &BuildingFootprintBitmap,
        s: i64,
        r: i64,
    ) -> Option<Grave> {
        let hash = cell_hash(self.id, s, r);
        if hash % 100 >= self.occupancy {
            return None;
        }
        let head = self
            .frame
            .column((s as f64 + 0.5) * self.slot, r as f64 * self.row + 0.5);
        let g = Grave { head, hash };
        let fits = self
            .cells(g)
            .all(|cell| self.free(editor, road_mask, footprints, cell));
        // A path or the wall in front of the headstone leaves no room to stand.
        let (bx, bz) = (head.0 - self.dir.0, head.1 - self.dir.1);
        (fits && self.mask.contains(bx, bz) && !road_mask.contains(bx, bz)).then_some(g)
    }

    /// The grave at (s, r) unless an earlier neighbour already holds one of its columns.
    fn kept(
        &self,
        editor: &WorldEditor,
        road_mask: &RoadMaskBitmap,
        footprints: &BuildingFootprintBitmap,
        s: i64,
        r: i64,
    ) -> Option<Grave> {
        let g = self.grave(editor, road_mask, footprints, s, r)?;
        for (ds, dr) in [(-1, 0), (-1, -1), (0, -1), (1, -1)] {
            if let Some(n) = self.grave(editor, road_mask, footprints, s + ds, r + dr) {
                if self.cells(n).any(|c| self.cells(g).any(|d| d == c)) {
                    return None;
                }
            }
        }
        Some(g)
    }

    fn place(
        &self,
        editor: &mut WorldEditor,
        road_mask: &RoadMaskBitmap,
        footprints: &BuildingFootprintBitmap,
    ) {
        let [along, across] = self.frame.local_span(editor.bounds());
        let (a0, a1) = (along.0.max(0.0), along.1.min(self.frame.length()));
        let (c0, c1) = (across.0.max(0.0), across.1.min(self.frame.width()));
        if a0 > a1 || c0 > c1 {
            return;
        }
        let s_range = (a0 / self.slot).floor() as i64 - 1..=(a1 / self.slot).ceil() as i64;
        let r_range = (c0 / self.row).floor() as i64 - 1..=(c1 / self.row).ceil() as i64;
        for r in r_range {
            for s in s_range.clone() {
                if let Some(g) = self.kept(editor, road_mask, footprints, s, r) {
                    self.draw(editor, g);
                }
            }
        }
    }

    fn draw(&self, editor: &mut WorldEditor, g: Grave) {
        let h = g.hash >> 8;
        match self.style {
            Style::War => {
                if editor.owns(g.head.0, g.head.1) {
                    editor.set_block(SMOOTH_QUARTZ, g.head.0, 1, g.head.1, None, None);
                }
            }
            Style::Lawn => {
                self.headstone(editor, g.head, h, true);
                let (fx, fz) = (g.head.0 + self.dir.0, g.head.1 + self.dir.1);
                if h % 7 < 2 && editor.owns(fx, fz) {
                    editor.set_block(flower(h >> 4), fx, 1, fz, None, None);
                }
            }
            Style::Beds => {}
        }
    }

    /// Whether a grave may take column (x, z).
    fn free(
        &self,
        editor: &WorldEditor,
        road_mask: &RoadMaskBitmap,
        footprints: &BuildingFootprintBitmap,
        (x, z): (i32, i32),
    ) -> bool {
        free(
            editor,
            &self.mask,
            road_mask,
            footprints,
            (x, z),
            self.style != Style::War,
        )
    }

    fn headstone(&self, editor: &mut WorldEditor, head: (i32, i32), h: u64, low: bool) {
        headstone(editor, head, self.dir, h, low);
    }
}

/// An upright stone facing along its grave, or a low marker on a lawn.
fn headstone(editor: &mut WorldEditor, (x, z): (i32, i32), dir: (i32, i32), h: u64, low: bool) {
    if !editor.owns(x, z) {
        return;
    }
    let facing = match dir {
        (1, _) => StairFacing::East,
        (-1, _) => StairFacing::West,
        (0, 1) => StairFacing::South,
        _ => StairFacing::North,
    };
    let y = editor.get_absolute_y(x, 1, z);
    match h % 20 {
        0..=8 => {
            const TABLETS: [Block; 5] = [
                STONE_BRICK_STAIRS,
                POLISHED_ANDESITE_STAIRS,
                DIORITE_STAIRS,
                GRANITE_STAIRS,
                MOSSY_COBBLESTONE_STAIRS,
            ];
            let stair = TABLETS[(h >> 5) as usize % TABLETS.len()];
            let block = create_stair_with_properties(stair, facing, StairShape::Straight);
            if !editor.block_exists_absolute(x, y, z) {
                editor.set_block_with_properties_absolute(block, x, y, z, None, None);
            }
        }
        9..=14 => {
            const STONES: [Block; 4] = [
                POLISHED_ANDESITE,
                POLISHED_GRANITE,
                POLISHED_DIORITE,
                STONE_BRICKS,
            ];
            editor.set_block(
                STONES[(h >> 5) as usize % STONES.len()],
                x,
                1,
                z,
                None,
                None,
            );
        }
        15..=16 => {
            editor.set_block(POLISHED_ANDESITE, x, 1, z, None, None);
            editor.set_block(ANDESITE_WALL, x, 2, z, None, None);
        }
        _ if low => editor.set_block(SMOOTH_STONE_SLAB, x, 1, z, None, None),
        _ => editor.set_block(POLISHED_DIORITE, x, 1, z, None, None),
    }
}

pub(crate) fn flower(h: u64) -> Block {
    const FLOWERS: [Block; 8] = [
        RED_TULIP,
        PINK_TULIP,
        WHITE_TULIP,
        ORANGE_TULIP,
        RED_FLOWER,
        ALLIUM,
        CORNFLOWER,
        LILY_OF_THE_VALLEY,
    ];
    FLOWERS[h as usize % FLOWERS.len()]
}

/// A grave mapped as a node of its own: a headstone on open ground, not a crypt or one below.
pub fn is_mapped_grave(tags: &HashMap<String, String>) -> bool {
    let below = ["level", "layer"].iter().any(|k| {
        tags.get(*k)
            .and_then(|v| v.parse::<f64>().ok())
            .is_some_and(|v| v < 0.0)
    });
    let headstone = tags.get("tomb").is_none_or(|t| {
        matches!(
            t.as_str(),
            "gravestone" | "headstone" | "grave" | "war_grave"
        )
    });
    !below
        && headstone
        && (tags.get("cemetery").map(String::as_str) == Some("grave")
            || tags.get("historic").map(String::as_str) == Some("tomb"))
}

/// A grave mapped on its own, such as a notable person's.
pub fn generate_mapped_grave(
    editor: &mut WorldEditor,
    node: &ProcessedNode,
    footprints: &BuildingFootprintBitmap,
) {
    let (x, z) = (node.x, node.z);
    if !editor.owns(x, z) || footprints.contains(x, z) || editor.surface_is_sealed(x, z) {
        return;
    }
    // A named tomb gets a tall cross, a plain grave a headstone with its bed.
    if node.tags.contains_key("historic") || node.tags.contains_key("name") {
        tombstone::place_cross(editor, x, z);
    } else {
        let h = coord_hash(x, z);
        tombstone::place_grave(editor, (x, z), (0, 1), h >> 8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::*;

    fn draw(tags: &[(&str, &str)], roads: &RoadMaskBitmap) -> WorldEditor<'static> {
        let xzbbox = Box::leak(Box::new(XZBBox::rect_from_xz_lengths(90.0, 90.0).unwrap()));
        let mut editor = test_editor(xzbbox);
        let bridges = no_bridges(&editor);
        let way = rect_way(11, 5, 5, 84, 84, tags);
        generate_cemetery(
            &mut editor,
            &way,
            &flat_ground_args(),
            &FloodFillCache::new(),
            &BuildingFootprintBitmap::new_empty(),
            roads,
            &bridges,
        );
        editor
    }

    #[test]
    fn sectors_mark_their_cemetery_and_inherit_a_war_style() {
        let mut elements = vec![
            ProcessedElement::Way(rect_way(
                1,
                0,
                0,
                100,
                100,
                &[("landuse", "cemetery"), ("cemetery", "war_cemetery")],
            )),
            ProcessedElement::Way(rect_way(2, 10, 10, 40, 40, &[("cemetery", "sector")])),
            ProcessedElement::Way(rect_way(3, 200, 0, 300, 100, &[("landuse", "cemetery")])),
        ];
        prepare(&mut elements, SignRegion::Europe);
        let tags = |i: usize| match &elements[i] {
            ProcessedElement::Way(w) => w.tags.clone(),
            _ => unreachable!(),
        };
        assert_eq!(tags(0).get(SECTORS_TAG).map(String::as_str), Some("yes"));
        assert_eq!(
            Style::of(&tags(1)),
            Style::War,
            "the sector takes the war style"
        );
        assert!(!tags(2).contains_key(SECTORS_TAG));
        assert_eq!(Style::of(&tags(2)), Style::Beds);
    }

    #[test]
    fn crypts_and_graves_below_ground_get_no_headstone() {
        let t = tag_map;
        assert!(is_mapped_grave(&t(&[("cemetery", "grave")])));
        assert!(is_mapped_grave(&t(&[
            ("historic", "tomb"),
            ("tomb", "gravestone")
        ])));
        assert!(!is_mapped_grave(&t(&[
            ("historic", "tomb"),
            ("tomb", "crypt")
        ])));
        assert!(!is_mapped_grave(&t(&[
            ("historic", "tomb"),
            ("level", "-1")
        ])));
    }

    #[test]
    fn war_graves_stand_in_rows_of_single_stones() {
        let editor = draw(
            &[("landuse", "cemetery"), ("cemetery", "war_cemetery")],
            &RoadMaskBitmap::new_empty(),
        );
        let stones = count_blocks(&editor, 6..84, 6..84, 1, &[SMOOTH_QUARTZ]);
        // A stone every 2 x 3 blocks, most places used.
        assert!(stones > 700 && stones < 1100, "{stones} stones");
        for x in 6..83 {
            for z in 6..84 {
                let here = editor.check_for_block(x, 1, z, Some(&[SMOOTH_QUARTZ]));
                assert!(
                    !(here && editor.check_for_block(x + 1, 1, z, Some(&[SMOOTH_QUARTZ]))),
                    "stones touch at ({x}, {z})"
                );
            }
        }
    }

    #[test]
    fn bed_graves_stand_in_straight_rows() {
        let xzbbox = XZBBox::rect_from_xz_lengths(90.0, 60.0).unwrap();
        let mut editor = test_editor(&xzbbox);
        let bridges = no_bridges(&editor);
        let way = rect_way(12, 5, 5, 84, 54, &[("landuse", "cemetery")]);
        generate_cemetery(
            &mut editor,
            &way,
            &flat_ground_args(),
            &FloodFillCache::new(),
            &BuildingFootprintBitmap::new_empty(),
            &RoadMaskBitmap::new_empty(),
            &bridges,
        );
        // The field runs along x, so every headstone row sits on one z, five apart.
        let mut rows = std::collections::BTreeSet::new();
        for x in 6..84 {
            for z in 6..54 {
                let y = editor.get_absolute_y(x, 1, z);
                if editor
                    .get_block_absolute(x, y, z)
                    .is_some_and(|b| b.name().ends_with("_stairs"))
                {
                    rows.insert(z);
                }
            }
        }
        let rows: Vec<i32> = rows.into_iter().collect();
        assert!(rows.len() >= 6, "{rows:?}");
        assert!(rows.windows(2).all(|w| (w[1] - w[0]) % 5 == 0), "{rows:?}");
    }

    #[test]
    fn bed_graves_keep_off_a_mapped_path() {
        let xzbbox = XZBBox::rect_from_xz_lengths(90.0, 90.0).unwrap();
        let path: Vec<(i32, i32)> = (0..90).map(|x| (x, 44)).collect();
        let roads = bitmap_with(&xzbbox, &path);
        let editor = draw(&[("landuse", "cemetery")], &roads);
        let graves = count_blocks(&editor, 6..84, 6..84, 0, &[PODZOL, GRAVEL, MOSS_BLOCK])
            + count_blocks(
                &editor,
                6..84,
                6..84,
                1,
                &[POLISHED_ANDESITE_SLAB, SMOOTH_STONE_SLAB],
            );
        assert!(graves > 400, "{graves} grave columns");
        for x in 6..84 {
            assert!(
                !editor.check_for_block(
                    x,
                    1,
                    44,
                    Some(&[POLISHED_ANDESITE_SLAB, SMOOTH_STONE_SLAB])
                ),
                "a grave on the path at x={x}"
            );
        }
        assert!(
            editor.check_for_block(5, 1, 40, Some(&[STONE_BRICK_WALL])),
            "the wall runs round it"
        );
    }
}
