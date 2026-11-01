//! Cemeteries: graves in rows along each grave field's outline, lawn, paths, trees and a wall.

use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::decals::region::SignRegion;
use crate::element_processing::area_layout::{
    bounds, cell_hash, centroid, inside, point_in_rings, AreaMask, Frame,
};
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::bush::{place_bush, BushKind};
use crate::element_processing::landuse::ground_is_protected;
use crate::element_processing::tree::Tree;
use crate::floodfill_cache::{
    in_hole, BuildingFootprintBitmap, FloodFillCache, Holes, RoadMaskBitmap,
};
use crate::land_cover::coord_hash;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use crate::structures::tombstone;
use crate::world_editor::WorldEditor;
use std::collections::HashMap;

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
            Style::Beds => (3.0, 4.0, 3, 85),
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
                sectors.push((i, centroid(&w.nodes)));
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
        Rows::new(way.id, style, frame, mask, !paths)
    });

    for &(x, z) in fill.iter() {
        if in_hole(holes, x, z) || !editor.in_bounds(x, z) || ground_is_protected(editor, x, z) {
            continue;
        }
        let path = plan.as_ref().is_some_and(|p| p.on_path(x, z));
        let ground = if path { GRAVEL } else { GRASS_BLOCK };
        editor.set_block(ground, x, 0, z, None, None);
    }

    if let Some(rows) = &plan {
        if style == Style::Beds {
            for &(x, z) in fill.iter() {
                if !in_hole(holes, x, z) && editor.owns(x, z) {
                    rows.bed_column(editor, road_mask, building_footprints, x, z);
                }
            }
        } else {
            rows.place(editor, road_mask, building_footprints);
        }
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
    for w in way.nodes.windows(2) {
        for (bx, _, bz) in bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z) {
            if road_mask.contains(bx, bz) || footprints.contains(bx, bz) {
                continue;
            }
            editor.set_block(STONE_BRICK_WALL, bx, 1, bz, None, None);
            editor.set_block(STONE_BRICK_SLAB, bx, 2, bz, None, None);
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
    /// Gravel paths cut through bigger fields where none are mapped.
    sections: bool,
    dir: (i32, i32),
}

/// Rows between paths, and graves between cross paths.
const ROWS_PER_SECTION: i64 = 8;
const SLOTS_PER_SECTION: i64 = 14;
/// Columns a bed grave takes along its row; the rest of a slot is the gap to the next one.
const BED_WIDTH: f64 = 2.0;

#[derive(Clone, Copy)]
struct Grave {
    head: (i32, i32),
    hash: u64,
}

impl Rows {
    fn new(id: u64, style: Style, frame: Frame, mask: AreaMask, unmapped: bool) -> Self {
        let (slot, row, len, occupancy) = style.lattice();
        let sections = style == Style::Beds
            && unmapped
            && frame.length().min(frame.width()) >= 3.0 * row * 2.0;
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
            sections,
        }
    }

    fn path_at(&self, s: i64, r: i64) -> bool {
        self.sections
            && ((r + 1).rem_euclid(ROWS_PER_SECTION) == 0
                || (s + 1).rem_euclid(SLOTS_PER_SECTION) == 0)
    }

    fn on_path(&self, x: i32, z: i32) -> bool {
        if !self.sections {
            return false;
        }
        let (a, c) = self.frame.local(x, z);
        self.path_at(
            (a / self.slot).floor() as i64,
            (c / self.row).floor() as i64,
        )
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
        if hash % 100 >= self.occupancy || self.path_at(s, r) {
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
            // Bed graves are drawn column by column, in `bed_column`.
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
        self.mask.contains(x, z)
            && !road_mask.contains(x, z)
            && !footprints.contains(x, z)
            && !editor.surface_is_sealed(x, z)
            && !editor.is_lc_water(x, z)
            && !(self.style != Style::War
                && editor.place_prop(crate::structures::Prop::Tombstone)
                && tombstone::near_crypt(x, z))
    }

    /// One column of a bed grave; rows are bands (headstones, beds, aisle) that stay straight when turned.
    fn bed_column(
        &self,
        editor: &mut WorldEditor,
        road_mask: &RoadMaskBitmap,
        footprints: &BuildingFootprintBitmap,
        x: i32,
        z: i32,
    ) {
        let (a, c) = self.frame.local(x, z);
        let (s, r) = (
            (a / self.slot).floor() as i64,
            (c / self.row).floor() as i64,
        );
        let (da, dc) = (a - s as f64 * self.slot, c - r as f64 * self.row);
        let len = self.len as f64;
        if da >= BED_WIDTH || dc >= len || self.path_at(s, r) {
            return;
        }
        let hash = cell_hash(self.id, s, r);
        if hash % 100 >= self.occupancy || !self.free(editor, road_mask, footprints, (x, z)) {
            return;
        }
        // The whole grave fits, with room to stand at its head.
        let (a0, c0) = (s as f64 * self.slot, r as f64 * self.row);
        let corners = [
            (0.5, -0.5),
            (BED_WIDTH - 0.5, -0.5),
            (0.5, len - 0.5),
            (BED_WIDTH - 0.5, len - 0.5),
        ];
        if !corners.iter().all(|&(ea, ec)| {
            self.free(
                editor,
                road_mask,
                footprints,
                self.frame.column(a0 + ea, c0 + ec),
            )
        }) {
            return;
        }
        let h = hash >> 8;
        if dc < 1.0 {
            self.headstone(editor, (x, z), h, false);
            return;
        }
        let ch = coord_hash(x, z ^ 0x9E37);
        let over = Some(&[GRASS_BLOCK][..]);
        match (h >> 12) % 20 {
            0..=8 => {
                editor.set_block(PODZOL, x, 0, z, over, None);
                if dc < 2.0 && ch.is_multiple_of(4) {
                    editor.set_block(SPRUCE_LEAVES, x, 1, z, None, None);
                } else if ch.is_multiple_of(3) {
                    editor.set_block(flower(ch >> 3), x, 1, z, None, None);
                } else if ch % 3 == 1 {
                    editor.set_block(FERN, x, 1, z, None, None);
                }
            }
            9..=12 => {
                editor.set_block(GRAVEL, x, 0, z, over, None);
                if dc >= len - 1.0 && da < 1.0 && ch.is_multiple_of(3) {
                    editor.set_block(LANTERN, x, 1, z, None, None);
                }
            }
            13..=17 => {
                let slab = if h.is_multiple_of(2) {
                    POLISHED_ANDESITE_SLAB
                } else {
                    SMOOTH_STONE_SLAB
                };
                editor.set_block(slab, x, 1, z, None, None);
            }
            _ => {
                editor.set_block(MOSS_BLOCK, x, 0, z, over, None);
            }
        }
    }

    /// An upright stone facing along its grave, or a low marker on a lawn.
    fn headstone(&self, editor: &mut WorldEditor, (x, z): (i32, i32), h: u64, low: bool) {
        if !editor.owns(x, z) {
            return;
        }
        let facing = match self.dir {
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
