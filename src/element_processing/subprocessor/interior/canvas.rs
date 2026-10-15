//! One storey as a grid of cells, with the placement helpers the furnishers share.
//!
//! Layout decisions come from the footprint alone and the world is only read per
//! cell, so tiles building parts of one building agree.

use crate::block_definitions::*;
use crate::element_processing::buildings::cached_prop_block;
use crate::world_editor::WorldEditor;
use fnv::{FnvHashMap, FnvHashSet};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Cell {
    /// Not part of this storey.
    Out,
    /// Taken by the shell or by a partition.
    Solid,
    /// An interior door.
    Door,
    /// Walkable and empty.
    Free,
    /// Walkable and to stay empty: inside doors and at the ladder.
    Keep,
    /// Furnished.
    Used,
}

/// Rooms at least this tall get lamps hung lower than the ceiling.
const CHANDELIER_MIN_HEADROOM: i32 = 6;
/// Height above the floor a chandelier hangs at.
const CHANDELIER_DROP: i32 = 4;

/// Blocks that light a room.
fn is_light(b: Block) -> bool {
    matches!(
        b,
        GLOWSTONE | SEA_LANTERN | SHROOMLIGHT | LANTERN | SOUL_LANTERN | END_ROD
    )
}

/// The four neighbours, in a fixed order so every tie breaks the same way.
pub(super) const DIRS: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Bounds {
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
}

impl Bounds {
    pub fn width(&self) -> i32 {
        self.max_x - self.min_x + 1
    }
    pub fn depth(&self) -> i32 {
        self.max_z - self.min_z + 1
    }
    pub fn long_x(&self) -> bool {
        self.width() >= self.depth()
    }
}

/// The walkable cell just inside a door, and the direction into the unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub cell: (i32, i32),
    pub inward: (i32, i32),
}

/// A unit's own coordinates: `v` runs from its front wall (0) to the back, `u` across.
#[derive(Clone, Copy, Debug)]
pub(super) struct Frame {
    ox: i32,
    oz: i32,
    /// From the front towards the back.
    pub n: (i32, i32),
    /// Across, from `u = 0` to `u = width - 1`.
    pub t: (i32, i32),
    pub width: i32,
    pub depth: i32,
}

impl Frame {
    pub fn new(b: Bounds, n: (i32, i32)) -> Self {
        let t = (-n.1, n.0);
        let ox = if n.0 + t.0 > 0 { b.min_x } else { b.max_x };
        let oz = if n.1 + t.1 > 0 { b.min_z } else { b.max_z };
        let (width, depth) = if t.0 != 0 {
            (b.width(), b.depth())
        } else {
            (b.depth(), b.width())
        };
        Self {
            ox,
            oz,
            n,
            t,
            width,
            depth,
        }
    }

    pub fn world(&self, u: i32, v: i32) -> (i32, i32) {
        (
            self.ox + self.t.0 * u + self.n.0 * v,
            self.oz + self.t.1 * u + self.n.1 * v,
        )
    }

    pub fn local(&self, x: i32, z: i32) -> (i32, i32) {
        let (dx, dz) = (x - self.ox, z - self.oz);
        (dx * self.t.0 + dz * self.t.1, dx * self.n.0 + dz * self.n.1)
    }

    /// A direction given in frame terms, as a world direction.
    pub fn dir(&self, du: i32, dv: i32) -> (i32, i32) {
        (self.t.0 * du + self.n.0 * dv, self.t.1 * du + self.n.1 * dv)
    }

    pub fn back(&self) -> (i32, i32) {
        self.n
    }

    pub fn front(&self) -> (i32, i32) {
        (-self.n.0, -self.n.1)
    }
}

/// Distance of every open cell of a unit from its nearest wall, 1 against the wall.
pub(super) struct Depth {
    min_x: i32,
    min_z: i32,
    w: i32,
    dist: Vec<i32>,
}

impl Depth {
    pub fn get(&self, x: i32, z: i32) -> i32 {
        let (dx, dz) = (x - self.min_x, z - self.min_z);
        if dx < 0 || dz < 0 || dx >= self.w {
            return 0;
        }
        self.dist
            .get((dz * self.w + dx) as usize)
            .copied()
            .unwrap_or(0)
    }
}

/// Rooms off a corridor.
pub(super) struct Corridor {
    pub hall: u16,
    pub rooms: Vec<u16>,
}

pub(super) struct Canvas<'e, 'w> {
    pub editor: &'e mut WorldEditor<'w>,
    min_x: i32,
    min_z: i32,
    w: i32,
    h: i32,
    cells: Vec<Cell>,
    zones: Vec<u16>,
    entries: Vec<Option<Entry>>,
    /// Absolute Y of the floor slab.
    pub floor_y: i32,
    /// Absolute Y of the highest air row under the ceiling.
    pub top: i32,
    /// Block interior walls are built from.
    pub partition: Block,
    /// The unit being furnished: nothing is placed outside it. 0 places anywhere.
    focus: u16,
    /// Interior walls asked for, built or not, and the doors through them.
    planned: FnvHashSet<(i32, i32)>,
    built: FnvHashSet<(i32, i32)>,
    doors: FnvHashSet<(i32, i32)>,
    /// Where people come in: inside the outer doors, at the ladder.
    starts: Vec<(i32, i32)>,
    /// Cells of each unit, filled on demand and dropped when units change.
    by_zone: RefCell<FnvHashMap<u16, Vec<(i32, i32)>>>,
}

impl<'e, 'w> Canvas<'e, 'w> {
    /// Reads the storey: a cell is free unless the shell fills it. `skip` drops cells.
    pub fn new(
        editor: &'e mut WorldEditor<'w>,
        footprint: &[(i32, i32)],
        floor_y: i32,
        top: i32,
        partition: Block,
        skip: &dyn Fn(i32, i32) -> bool,
    ) -> Self {
        let min_x = footprint.iter().map(|c| c.0).min().unwrap_or(0);
        let max_x = footprint.iter().map(|c| c.0).max().unwrap_or(-1);
        let min_z = footprint.iter().map(|c| c.1).min().unwrap_or(0);
        let max_z = footprint.iter().map(|c| c.1).max().unwrap_or(-1);
        let w = (max_x - min_x + 1).max(0);
        let h = (max_z - min_z + 1).max(0);
        let mut cells = vec![Cell::Out; (w * h) as usize];
        let mut zones = vec![0u16; (w * h) as usize];
        for &(x, z) in footprint {
            if skip(x, z) {
                continue;
            }
            let i = ((z - min_z) * w + (x - min_x)) as usize;
            cells[i] = if editor.get_block_absolute(x, floor_y + 1, z).is_some() {
                Cell::Solid
            } else {
                Cell::Free
            };
            zones[i] = 1;
        }
        Self {
            editor,
            min_x,
            min_z,
            w,
            h,
            cells,
            zones,
            entries: vec![None, None],
            floor_y,
            top,
            partition,
            focus: 0,
            planned: FnvHashSet::default(),
            built: FnvHashSet::default(),
            doors: FnvHashSet::default(),
            starts: Vec::new(),
            by_zone: RefCell::new(FnvHashMap::default()),
        }
    }

    /// A cell people reach the storey from.
    pub fn add_start(&mut self, cell: (i32, i32)) {
        self.starts.push(cell);
    }

    /// Furnish only `zone` until told otherwise.
    pub fn focus(&mut self, zone: u16) {
        self.focus = zone;
    }

    fn in_focus(&self, x: i32, z: i32) -> bool {
        self.focus == 0 || self.zone_of(x, z) == self.focus
    }

    fn idx(&self, x: i32, z: i32) -> Option<usize> {
        let (dx, dz) = (x - self.min_x, z - self.min_z);
        (dx >= 0 && dz >= 0 && dx < self.w && dz < self.h).then(|| (dz * self.w + dx) as usize)
    }

    pub fn cell(&self, x: i32, z: i32) -> Cell {
        self.idx(x, z).map_or(Cell::Out, |i| self.cells[i])
    }

    fn set_cell(&mut self, x: i32, z: i32, cell: Cell) {
        if let Some(i) = self.idx(x, z) {
            self.cells[i] = cell;
        }
    }

    pub fn zone_of(&self, x: i32, z: i32) -> u16 {
        self.idx(x, z).map_or(0, |i| self.zones[i])
    }

    /// Air rows between the floor and the ceiling.
    pub fn headroom(&self) -> i32 {
        self.top - self.floor_y
    }

    /// Free, and part of the unit being furnished.
    pub fn is_free(&self, x: i32, z: i32) -> bool {
        self.cell(x, z) == Cell::Free && self.in_focus(x, z)
    }

    pub fn walkable(&self, x: i32, z: i32) -> bool {
        matches!(self.cell(x, z), Cell::Free | Cell::Keep)
    }

    /// Open space of one unit, furnished or not.
    fn open_in(&self, zone: u16, x: i32, z: i32) -> bool {
        self.zone_of(x, z) == zone
            && matches!(self.cell(x, z), Cell::Free | Cell::Keep | Cell::Used)
    }

    pub fn keep(&mut self, x: i32, z: i32) {
        if self.cell(x, z) == Cell::Free {
            self.set_cell(x, z, Cell::Keep);
        }
    }

    /// Open cells of a unit, row by row.
    pub fn cells(&self, zone: u16) -> Vec<(i32, i32)> {
        self.cells_any(zone)
            .into_iter()
            .filter(|&(x, z)| matches!(self.cell(x, z), Cell::Free | Cell::Keep | Cell::Used))
            .collect()
    }

    /// Every cell of a unit regardless of contents, which differ between tiles.
    pub fn cells_any(&self, zone: u16) -> Vec<(i32, i32)> {
        let mut cache = self.by_zone.borrow_mut();
        if cache.is_empty() {
            for dz in 0..self.h {
                for dx in 0..self.w {
                    let i = (dz * self.w + dx) as usize;
                    if self.zones[i] != 0 && self.cells[i] != Cell::Out {
                        cache
                            .entry(self.zones[i])
                            .or_default()
                            .push((self.min_x + dx, self.min_z + dz));
                    }
                }
            }
        }
        cache.get(&zone).cloned().unwrap_or_default()
    }

    /// Moves a cell to another unit.
    fn set_zone(&mut self, i: usize, zone: u16) {
        self.zones[i] = zone;
        self.by_zone.get_mut().clear();
    }

    /// Size of a unit in cells, for the decisions that depend on it.
    pub fn area(&self, zone: u16) -> usize {
        self.cells_any(zone).len()
    }

    pub fn bounds(&self, zone: u16) -> Option<Bounds> {
        let cells = self.cells_any(zone);
        Some(Bounds {
            min_x: cells.iter().map(|c| c.0).min()?,
            min_z: cells.iter().map(|c| c.1).min()?,
            max_x: cells.iter().map(|c| c.0).max()?,
            max_z: cells.iter().map(|c| c.1).max()?,
        })
    }

    fn add_zone(&mut self) -> u16 {
        self.entries.push(None);
        (self.entries.len() - 1) as u16
    }

    pub fn entry(&self, zone: u16) -> Option<Entry> {
        self.entries.get(zone as usize).copied().flatten()
    }

    /// Keeps the way in clear before walls go up.
    pub fn reserve_entrances(&mut self, doors: &[Entry]) {
        for door in doors {
            let (x, z) = door.cell;
            self.keep(x, z);
            self.keep(x + door.inward.0, z + door.inward.1);
        }
    }

    /// Outer doors; the first one into a unit becomes its entry.
    pub fn add_entrances(&mut self, doors: &[Entry]) {
        let mut claimed: Vec<u16> = Vec::new();
        for door in doors {
            let zone = self.zone_of(door.cell.0, door.cell.1);
            if zone == 0 {
                continue;
            }
            let (x, z) = door.cell;
            self.keep(x, z);
            self.keep(x + door.inward.0, z + door.inward.1);
            self.starts.push((x, z));
            if !claimed.contains(&zone) {
                claimed.push(zone);
                if let Some(slot) = self.entries.get_mut(zone as usize) {
                    *slot = Some(*door);
                }
            }
        }
    }

    /// Keep the cells inside every door in the shell clear, mapped or not.
    pub fn clear_doorways(&mut self) {
        let mut keep = Vec::new();
        for (x, z) in self.cells_all() {
            if self.cell(x, z) != Cell::Free {
                continue;
            }
            for (dx, dz) in DIRS {
                let door = self
                    .editor
                    .get_block_absolute(x + dx, self.floor_y + 1, z + dz)
                    .is_some_and(|b| b.name().ends_with("_door"));
                if door && self.cell(x + dx, z + dz) != Cell::Door {
                    keep.push((x, z));
                    keep.push((x - dx, z - dz));
                }
            }
        }
        for (x, z) in keep {
            self.keep(x, z);
        }
    }

    fn cells_all(&self) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for dz in 0..self.h {
            for dx in 0..self.w {
                if self.cells[(dz * self.w + dx) as usize] != Cell::Out {
                    out.push((self.min_x + dx, self.min_z + dz));
                }
            }
        }
        out
    }

    /// Facing of the unit: from its entry inwards, else along its long axis.
    pub fn frame(&self, zone: u16) -> Option<Frame> {
        let b = self.bounds(zone)?;
        let n = match self.entry(zone) {
            Some(e) => e.inward,
            None if b.long_x() => (1, 0),
            None => (0, 1),
        };
        Some(Frame::new(b, n))
    }

    /// Facing along the long axis, with the entry at the front end.
    pub fn long_frame(&self, zone: u16) -> Option<Frame> {
        let b = self.bounds(zone)?;
        let positive = if b.long_x() { (1, 0) } else { (0, 1) };
        let frame = Frame::new(b, positive);
        let n = match self.entry(zone) {
            Some(e) if frame.local(e.cell.0, e.cell.1).1 > frame.depth / 2 => {
                (-positive.0, -positive.1)
            }
            _ => positive,
        };
        Some(Frame::new(b, n))
    }

    /// True when the neighbour at `d` closes this unit off: shell, partition or another unit.
    pub fn closed_towards(&self, zone: u16, x: i32, z: i32, d: (i32, i32)) -> bool {
        !self.open_in(zone, x + d.0, z + d.1)
    }

    /// Direction into the unit from a wall the free cell stands against.
    pub fn edge_normal(&self, zone: u16, x: i32, z: i32) -> Option<(i32, i32)> {
        if self.zone_of(x, z) != zone {
            return None;
        }
        DIRS.iter()
            .find(|&&d| self.closed_towards(zone, x, z, d))
            .map(|&(dx, dz)| (-dx, -dz))
    }

    /// Free cells standing against a wall of the unit, with the direction into the room.
    pub fn wall_cells(&self, zone: u16) -> Vec<((i32, i32), (i32, i32))> {
        self.cells(zone)
            .into_iter()
            .filter(|&(x, z)| self.cell(x, z) == Cell::Free)
            .filter_map(|(x, z)| self.edge_normal(zone, x, z).map(|n| ((x, z), n)))
            .collect()
    }

    pub fn depth(&self, zone: u16) -> Depth {
        let mut dist = vec![0i32; (self.w * self.h) as usize];
        let mut queue = VecDeque::new();
        for (x, z) in self.cells(zone) {
            if DIRS.iter().any(|&d| self.closed_towards(zone, x, z, d)) {
                let i = self.idx(x, z).unwrap();
                dist[i] = 1;
                queue.push_back((x, z));
            }
        }
        while let Some((x, z)) = queue.pop_front() {
            let here = dist[self.idx(x, z).unwrap()];
            for (dx, dz) in DIRS {
                let (nx, nz) = (x + dx, z + dz);
                if !self.open_in(zone, nx, nz) {
                    continue;
                }
                let i = self.idx(nx, nz).unwrap();
                if dist[i] == 0 {
                    dist[i] = here + 1;
                    queue.push_back((nx, nz));
                }
            }
        }
        Depth {
            min_x: self.min_x,
            min_z: self.min_z,
            w: self.w,
            dist,
        }
    }

    /// Places a block `dy` rows up. Standing height needs a free cell and furnishes it.
    pub fn put(&mut self, x: i32, dy: i32, z: i32, block: Block) -> bool {
        self.put_with(x, dy, z, BlockWithProperties::simple(block))
    }

    pub fn put_with(&mut self, x: i32, dy: i32, z: i32, block: BlockWithProperties) -> bool {
        if dy < 0 || dy > self.headroom() || !self.in_focus(x, z) {
            return false;
        }
        match self.cell(x, z) {
            Cell::Free if dy == 1 => self.set_cell(x, z, Cell::Used),
            Cell::Free | Cell::Used | Cell::Keep if dy != 1 => {}
            _ => return false,
        }
        self.editor
            .set_block_with_properties_absolute(block, x, self.floor_y + dy, z, None, None);
        true
    }

    /// Stacks blocks upwards from standing height, stopping under the ceiling.
    pub fn stack(&mut self, x: i32, z: i32, blocks: &[Block]) -> bool {
        if !self.is_free(x, z) {
            return false;
        }
        for (i, &b) in blocks.iter().enumerate() {
            self.put(x, 1 + i as i32, z, b);
        }
        true
    }

    /// Replaces the floor slab under a cell.
    pub fn floor(&mut self, x: i32, z: i32, block: Block) {
        if self.in_focus(x, z) && matches!(self.cell(x, z), Cell::Free | Cell::Keep | Cell::Used) {
            self.editor
                .set_block_absolute(block, x, self.floor_y, z, None, Some(&[]));
        }
    }

    /// A wall-mounted block above a free cell, kept only where a wall stands behind it.
    pub fn mount(&mut self, x: i32, dy: i32, z: i32, inward: (i32, i32), block: Block) -> bool {
        let behind = (x - inward.0, z - inward.1);
        let backed = !matches!(
            self.cell(behind.0, behind.1),
            Cell::Free | Cell::Keep | Cell::Used
        ) || self
            .editor
            .get_block_absolute(behind.0, self.floor_y + dy, behind.1)
            .is_some();
        backed && self.put(x, dy, z, block)
    }

    /// True when the wall behind an edge cell has a window at eye height.
    pub fn window_behind(&self, x: i32, z: i32, inward: (i32, i32)) -> bool {
        let (bx, bz) = (x - inward.0, z - inward.1);
        self.editor
            .get_block_absolute(bx, self.floor_y + 2, bz)
            .is_some_and(|b| {
                let name = b.name();
                name.contains("glass") || name.contains("pane")
            })
    }

    /// Chandeliers in tall rooms, and a light wherever a room has none nearby.
    /// Each cell decides from its own neighbourhood, so tiles agree.
    pub fn light_up(&mut self, slab: Block, slab_above: bool) {
        let headroom = self.headroom();
        // Low rooms sink lights into a slab, under the roof into their own floor.
        let sunk = if slab_above {
            self.top + 1
        } else {
            self.floor_y
        };
        let open: Vec<(i32, i32)> = self
            .cells_all()
            .into_iter()
            .filter(|&(x, z)| {
                self.zone_of(x, z) != 0
                    && matches!(self.cell(x, z), Cell::Free | Cell::Keep | Cell::Used)
            })
            .collect();
        let mut lit: std::collections::HashSet<(i32, i32)> = open
            .iter()
            .copied()
            .filter(|&(x, z)| {
                [self.top, sunk].iter().any(|&y| {
                    self.editor
                        .get_block_absolute(x, y, z)
                        .is_some_and(is_light)
                })
            })
            .collect();

        if headroom >= CHANDELIER_MIN_HEADROOM {
            let chain = cached_prop_block(CHAIN_X, &[("axis", "y")]);
            for &(x, z) in &open {
                if x.rem_euclid(5) != 2 || z.rem_euclid(5) != 2 || !self.walkable(x, z) {
                    continue;
                }
                for dy in CHANDELIER_DROP + 1..=headroom {
                    self.editor.set_block_with_properties_absolute(
                        chain.clone(),
                        x,
                        self.floor_y + dy,
                        z,
                        None,
                        None,
                    );
                }
                self.editor.set_block_with_properties_absolute(
                    hanging_lantern(),
                    x,
                    self.floor_y + CHANDELIER_DROP,
                    z,
                    None,
                    None,
                );
                lit.insert((x, z));
            }
        }

        for &(x, z) in &open {
            if (x + 2 * z).rem_euclid(5) != 0 {
                continue;
            }
            let zone = self.zone_of(x, z);
            let near = (-2..=2).any(|dx| {
                (-2..=2).any(|dz| {
                    lit.contains(&(x + dx, z + dz)) && self.zone_of(x + dx, z + dz) == zone
                })
            });
            if near {
                continue;
            }
            if headroom >= 3 {
                self.editor
                    .set_block_absolute(GLOWSTONE, x, self.top, z, None, None);
            } else {
                self.editor
                    .set_block_absolute(GLOWSTONE, x, sunk, z, Some(&[slab]), None);
            }
        }
    }

    /// Interior wall from floor to ceiling.
    pub fn partition(&mut self, x: i32, z: i32) {
        self.planned.insert((x, z));
        if self.cell(x, z) != Cell::Free {
            return;
        }
        self.built.insert((x, z));
        for y in self.floor_y + 1..=self.top {
            self.editor
                .set_block_absolute(self.partition, x, y, z, None, None);
        }
        self.set_cell(x, z, Cell::Solid);
    }

    /// Interior door in a partition, passable along `across`.
    pub fn door(&mut self, x: i32, z: i32, across: (i32, i32)) {
        self.doors.insert((x, z));
        if self.cell(x, z) != Cell::Free {
            return;
        }
        let facing = facing(across);
        let lower = cached_prop_block(
            DARK_OAK_DOOR_LOWER,
            &[("half", "lower"), ("facing", facing), ("hinge", "left")],
        );
        let upper = cached_prop_block(
            DARK_OAK_DOOR_UPPER,
            &[("half", "upper"), ("facing", facing), ("hinge", "left")],
        );
        self.editor
            .set_block_with_properties_absolute(lower, x, self.floor_y + 1, z, None, None);
        if self.headroom() >= 2 {
            self.editor.set_block_with_properties_absolute(
                upper,
                x,
                self.floor_y + 2,
                z,
                None,
                None,
            );
        }
        for y in self.floor_y + 3..=self.top {
            self.editor
                .set_block_absolute(self.partition, x, y, z, None, None);
        }
        self.set_cell(x, z, Cell::Door);
        self.keep(x + across.0, z + across.1);
        self.keep(x - across.0, z - across.1);
    }

    /// Gives each cell to its nearest anchor and walls the units apart, one door per
    /// border. Returns unit ids in anchor order.
    pub fn split_nearest(&mut self, anchors: &[(i32, i32)]) -> Vec<u16> {
        let ids: Vec<u16> = anchors.iter().map(|_| self.add_zone()).collect();
        if ids.len() < 2 {
            for z in &ids {
                self.relabel(1, *z);
            }
            return ids;
        }
        for (x, z) in self.cells_all() {
            let i = self.idx(x, z).unwrap();
            if self.zones[i] != 1 {
                continue;
            }
            let nearest = anchors
                .iter()
                .enumerate()
                .min_by_key(|(_, a)| {
                    let (dx, dz) = ((x - a.0) as i64, (z - a.1) as i64);
                    dx * dx + dz * dz
                })
                .map(|(k, _)| k)
                .unwrap_or(0);
            self.set_zone(i, ids[nearest]);
        }
        self.wall_between_zones();
        self.connect();
        ids
    }

    fn relabel(&mut self, from: u16, to: u16) {
        for z in self.zones.iter_mut() {
            if *z == from {
                *z = to;
            }
        }
        self.by_zone.get_mut().clear();
    }

    /// Walls on every border between units, with a door in its middle.
    fn wall_between_zones(&mut self) {
        // (lower zone, higher zone) -> wall cells of that border, row by row.
        let mut borders: BTreeMap<(u16, u16), Vec<(i32, i32)>> = BTreeMap::new();
        for (x, z) in self.cells_all() {
            let here = self.zone_of(x, z);
            for (dx, dz) in DIRS {
                let (nx, nz) = (x + dx, z + dz);
                let there = self.zone_of(nx, nz);
                if there == 0 || there == here || self.cell(nx, nz) == Cell::Out {
                    continue;
                }
                if here > there {
                    let cells = borders.entry((there, here)).or_default();
                    if !cells.contains(&(x, z)) {
                        cells.push((x, z));
                    }
                }
            }
        }
        for ((low, high), cells) in borders {
            let door = self.pick_door(&cells, low, high);
            for &(x, z) in &cells {
                if Some((x, z)) != door.map(|d| d.0) {
                    self.partition(x, z);
                }
            }
            if let Some(((x, z), across)) = door {
                self.door(x, z, across);
                self.set_entry_if_none(high, (x + across.0, z + across.1), across);
                self.set_entry_if_none(low, (x - across.0, z - across.1), (-across.0, -across.1));
            }
        }
    }

    fn set_entry_if_none(&mut self, zone: u16, cell: (i32, i32), inward: (i32, i32)) {
        if let Some(slot) = self.entries.get_mut(zone as usize) {
            slot.get_or_insert(Entry { cell, inward });
        }
    }

    /// A border cell with `low` on one side and `high` across, nearest the middle.
    fn pick_door(
        &self,
        cells: &[(i32, i32)],
        low: u16,
        high: u16,
    ) -> Option<((i32, i32), (i32, i32))> {
        let mid = cells.len() / 2;
        let mut order: Vec<usize> = (0..cells.len()).collect();
        order.sort_by_key(|&i| (i as i64 - mid as i64).abs());
        for i in order {
            let (x, z) = cells[i];
            for (dx, dz) in DIRS {
                let (bx, bz) = (x - dx, z - dz);
                let (fx, fz) = (x + dx, z + dz);
                if self.zone_of(bx, bz) == low
                    && self.walkable(bx, bz)
                    && self.zone_of(fx, fz) == high
                    && self.walkable(fx, fz)
                {
                    return Some(((x, z), (dx, dz)));
                }
            }
        }
        None
    }

    /// Walls off the part behind row `v_wall` as its own unit, with a door.
    pub fn split_back(&mut self, zone: u16, f: &Frame, v_wall: i32) -> Option<u16> {
        let back = self.add_zone();
        let mut wall = Vec::new();
        for (x, z) in self.cells_any(zone) {
            let v = f.local(x, z).1;
            if v > v_wall {
                let i = self.idx(x, z).unwrap();
                self.set_zone(i, back);
            } else if v == v_wall {
                wall.push((x, z));
            }
        }
        if self.area(back) < 6 || wall.is_empty() {
            self.relabel(back, zone);
            return None;
        }
        let mid = f.width / 2;
        wall.sort_by_key(|&(x, z)| (f.local(x, z).0 - mid).abs());
        let n = f.n;
        let door = wall.iter().copied().find(|&(x, z)| {
            let (fx, fz) = (x - n.0, z - n.1);
            let (bx, bz) = (x + n.0, z + n.1);
            self.zone_of(fx, fz) == zone
                && self.walkable(fx, fz)
                && self.zone_of(bx, bz) == back
                && self.walkable(bx, bz)
        });
        let Some(door) = door else {
            self.relabel(back, zone);
            return None;
        };
        for &(x, z) in &wall {
            if (x, z) != door {
                self.partition(x, z);
            }
        }
        self.door(door.0, door.1, n);
        self.set_entry_if_none(back, (door.0 + n.0, door.1 + n.1), n);
        self.connect();
        Some(back)
    }

    /// Rooms off a corridor along the long axis, on one or both sides. `None` if the
    /// unit is too narrow.
    pub fn split_corridor(
        &mut self,
        zone: u16,
        room_len: i32,
        room_depth: i32,
    ) -> Option<Corridor> {
        let b = self.bounds(zone)?;
        let along_x = b.long_x();
        let (a0, a1, c0, c1) = if along_x {
            (b.min_x, b.max_x, b.min_z, b.max_z)
        } else {
            (b.min_z, b.max_z, b.min_x, b.max_x)
        };
        let span = c1 - c0 + 1;
        // Corridor rows, then per side the wall row and the direction to the corridor.
        let (hall, sides): ((i32, i32), Vec<(i32, i32)>) = if span >= 2 * (room_depth + 1) + 2 {
            let h0 = c0 + (span - 2) / 2;
            ((h0, h0 + 1), vec![(h0 - 1, 1), (h0 + 2, -1)])
        } else if span >= room_depth + 3 {
            ((c0, c0 + 1), vec![(c0 + 2, -1)])
        } else {
            return None;
        };
        let length = a1 - a0 + 1;
        let count = ((length + 1) / (room_len + 1)).max(1) as usize;
        // Room k spans [starts[k], starts[k + 1] - 1), its last cell being a wall.
        let starts: Vec<i32> = (0..=count as i32)
            .map(|k| a0 + (k * (length + 1)) / count as i32)
            .collect();
        let side_of = |c: i32| -> Option<usize> {
            if c >= hall.0 && c <= hall.1 {
                None
            } else if sides.len() == 1 || c < hall.0 {
                Some(0)
            } else {
                Some(1)
            }
        };
        let room_ids: Vec<Vec<u16>> = sides
            .iter()
            .map(|_| (0..count).map(|_| self.add_zone()).collect())
            .collect();

        // The room an outside door opens into becomes part of the hall, as its lobby.
        let lobby = self.entry(zone).and_then(|e| {
            let (a, c) = if along_x {
                e.cell
            } else {
                (e.cell.1, e.cell.0)
            };
            let side = side_of(c)?;
            let k = starts.windows(2).position(|w| a >= w[0] && a < w[1])?;
            Some((side, k))
        });

        let mut walls: Vec<(i32, i32)> = Vec::new();
        for (x, z) in self.cells_any(zone) {
            let (a, c) = if along_x { (x, z) } else { (z, x) };
            let Some(side) = side_of(c) else {
                continue;
            };
            let wall_row = sides[side].0;
            let k = starts
                .windows(2)
                .position(|w| a >= w[0] && a < w[1])
                .unwrap_or(count - 1);
            let in_lobby = lobby == Some((side, k));
            if c == wall_row {
                if !in_lobby {
                    walls.push((x, z));
                }
                continue;
            }
            if in_lobby {
                if k + 1 < count && a == starts[k + 1] - 1 {
                    walls.push((x, z));
                }
                continue;
            }
            let i = self.idx(x, z).unwrap();
            self.set_zone(i, room_ids[side][k]);
            if k + 1 < count && a == starts[k + 1] - 1 {
                walls.push((x, z));
            }
        }

        // One door per room, mid-wall: (wall cell, into the room, room).
        type Door = ((i32, i32), (i32, i32), u16);
        let mut doors: Vec<Door> = Vec::new();
        for (side, &(wall_row, toward_hall)) in sides.iter().enumerate() {
            let d = if along_x {
                (0, toward_hall)
            } else {
                (toward_hall, 0)
            };
            for k in 0..count {
                if lobby == Some((side, k)) {
                    continue;
                }
                let room = room_ids[side][k];
                let (lo, hi) = (starts[k], starts[k + 1] - 2);
                let mid = (lo + hi) / 2;
                let found = (0..=(hi - lo).max(0)).find_map(|off| {
                    let a = if off % 2 == 0 {
                        mid + off / 2
                    } else {
                        mid - off / 2 - 1
                    };
                    if a < lo || a > hi {
                        return None;
                    }
                    let w = if along_x {
                        (a, wall_row)
                    } else {
                        (wall_row, a)
                    };
                    let room_side = (w.0 - d.0, w.1 - d.1);
                    let hall_side = (w.0 + d.0, w.1 + d.1);
                    (walls.contains(&w)
                        && self.zone_of(room_side.0, room_side.1) == room
                        && self.walkable(room_side.0, room_side.1)
                        && self.zone_of(hall_side.0, hall_side.1) == zone
                        && self.walkable(hall_side.0, hall_side.1))
                    .then_some(w)
                });
                if let Some(w) = found {
                    doors.push((w, (-d.0, -d.1), room));
                }
            }
        }

        for &(x, z) in &walls {
            if !doors.iter().any(|(w, _, _)| *w == (x, z)) {
                self.partition(x, z);
            }
        }
        for (w, into_room, room) in doors {
            self.door(w.0, w.1, into_room);
            self.set_entry_if_none(room, (w.0 + into_room.0, w.1 + into_room.1), into_room);
        }
        // Rooms the corridor could not reach get a door from a neighbour.
        self.connect();
        let rooms = room_ids
            .into_iter()
            .flatten()
            .filter(|&room| self.area(room) >= 6)
            .collect();
        Some(Corridor { hall: zone, rooms })
    }

    /// Opens a door into every part its walls shut off. Reach is worked out on the
    /// outline and the planned walls, so tiles agree.
    pub fn connect(&mut self) {
        if self.starts.is_empty() {
            return;
        }
        let cells = self.cells_all();
        let passable = |c: &Self, p: (i32, i32)| {
            c.zone_of(p.0, p.1) != 0 && (!c.planned.contains(&p) || c.doors.contains(&p))
        };
        for _ in 0..64 {
            let mut reached: FnvHashSet<(i32, i32)> = FnvHashSet::default();
            let mut queue: VecDeque<(i32, i32)> = self
                .starts
                .iter()
                .copied()
                .filter(|&p| passable(self, p))
                .collect();
            reached.extend(queue.iter().copied());
            while let Some((x, z)) = queue.pop_front() {
                for (dx, dz) in DIRS {
                    let n = (x + dx, z + dz);
                    if passable(self, n) && reached.insert(n) {
                        queue.push_back(n);
                    }
                }
            }
            // Shut-off parts, each labelled with its first cell.
            let mut part: BTreeMap<(i32, i32), (i32, i32)> = BTreeMap::new();
            for &c in &cells {
                if !passable(self, c) || reached.contains(&c) || part.contains_key(&c) {
                    continue;
                }
                part.insert(c, c);
                let mut queue = VecDeque::from([c]);
                while let Some((x, z)) = queue.pop_front() {
                    for (dx, dz) in DIRS {
                        let n = (x + dx, z + dz);
                        if passable(self, n) && !reached.contains(&n) && !part.contains_key(&n) {
                            part.insert(n, c);
                            queue.push_back(n);
                        }
                    }
                }
            }
            if part.is_empty() {
                return;
            }
            // Per part, the wall cell with reach on one side, nearest the part's middle.
            let mut sums: BTreeMap<(i32, i32), (i64, i64, i64)> = BTreeMap::new();
            for (&(x, z), &label) in &part {
                let e = sums.entry(label).or_default();
                e.0 += x as i64;
                e.1 += z as i64;
                e.2 += 1;
            }
            let mut walls: Vec<(i32, i32)> = self.built.iter().copied().collect();
            walls.sort_unstable();
            // Per part: (distance, wall cell, direction into the part).
            type Opening = (i64, (i32, i32), (i32, i32));
            let mut best: BTreeMap<(i32, i32), Opening> = BTreeMap::new();
            for w in walls {
                if self.doors.contains(&w) {
                    continue;
                }
                for d in DIRS {
                    let from = (w.0 - d.0, w.1 - d.1);
                    let into = (w.0 + d.0, w.1 + d.1);
                    let Some(&label) = part.get(&into) else {
                        continue;
                    };
                    if !reached.contains(&from) {
                        continue;
                    }
                    let (sx, sz, n) = sums[&label];
                    let (mx, mz) = (sx / n, sz / n);
                    let dist = (w.0 as i64 - mx).pow(2) + (w.1 as i64 - mz).pow(2);
                    if best.get(&label).is_none_or(|b| dist < b.0) {
                        best.insert(label, (dist, w, d));
                    }
                }
            }
            if best.is_empty() {
                return;
            }
            for (_, (_, w, d)) in best {
                self.open_wall(w, d);
            }
        }
    }

    /// Turns a built interior wall cell into a door passable along `across`.
    fn open_wall(&mut self, w: (i32, i32), across: (i32, i32)) {
        let (x, z) = w;
        let f = facing(across);
        let lower = cached_prop_block(
            DARK_OAK_DOOR_LOWER,
            &[("half", "lower"), ("facing", f), ("hinge", "left")],
        );
        let upper = cached_prop_block(
            DARK_OAK_DOOR_UPPER,
            &[("half", "upper"), ("facing", f), ("hinge", "left")],
        );
        self.editor.set_block_with_properties_absolute(
            lower,
            x,
            self.floor_y + 1,
            z,
            None,
            Some(&[]),
        );
        if self.headroom() >= 2 {
            self.editor.set_block_with_properties_absolute(
                upper,
                x,
                self.floor_y + 2,
                z,
                None,
                Some(&[]),
            );
        }
        self.doors.insert(w);
        self.set_cell(x, z, Cell::Door);
        // Clear the way on both sides unless something already stands there.
        let into = (x + across.0, z + across.1);
        self.keep(into.0, into.1);
        self.keep(x - across.0, z - across.1);
        let zone = self.zone_of(into.0, into.1);
        self.set_entry_if_none(zone, into, across);
    }
}

/// Facing string for a horizontal direction.
pub(super) fn facing(d: (i32, i32)) -> &'static str {
    match d {
        (1, _) => "east",
        (-1, _) => "west",
        (_, 1) => "south",
        _ => "north",
    }
}

/// Stable per-cell hash, so variety does not depend on the order cells are visited.
pub(super) fn mix(x: i32, z: i32, salt: u64) -> u64 {
    let mut h = (x as u32 as u64) << 32 | (z as u32 as u64);
    h ^= salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

pub(super) fn pick<T: Copy>(options: &[T], h: u64) -> T {
    options[(h % options.len() as u64) as usize]
}

/// A stair used as a seat, the sitter looking along `look`.
pub(super) fn seat(stairs: Block, look: (i32, i32)) -> BlockWithProperties {
    cached_prop_block(
        stairs,
        &[
            ("facing", facing((-look.0, -look.1))),
            ("half", "bottom"),
            ("shape", "straight"),
        ],
    )
}

pub(super) fn top_slab(slab: Block) -> BlockWithProperties {
    cached_prop_block(slab, &[("type", "top")])
}

/// Furnaces, smokers, looms and lecterns showing their front along `front`.
pub(super) fn facing_block(block: Block, front: (i32, i32)) -> BlockWithProperties {
    cached_prop_block(block, &[("facing", facing(front))])
}

/// A grindstone standing on the floor, its wheel turning along `front`.
pub(super) fn grindstone(front: (i32, i32)) -> BlockWithProperties {
    cached_prop_block(GRINDSTONE, &[("face", "floor"), ("facing", facing(front))])
}

pub(super) fn hanging_lantern() -> BlockWithProperties {
    cached_prop_block(LANTERN, &[("hanging", "true")])
}

/// A fence post joined to its neighbours along one axis.
pub(super) fn fence_run(fence: Block, along_x: bool) -> BlockWithProperties {
    if along_x {
        cached_prop_block(fence, &[("east", "true"), ("west", "true")])
    } else {
        cached_prop_block(fence, &[("north", "true"), ("south", "true")])
    }
}

/// One wood species for a building's furniture.
#[derive(Clone, Copy, Debug)]
pub(super) struct Wood {
    pub planks: Block,
    pub stairs: Block,
    pub slab: Block,
    pub fence: Block,
}

const WOODS: [Wood; 3] = [
    Wood {
        planks: OAK_PLANKS,
        stairs: OAK_STAIRS,
        slab: OAK_SLAB,
        fence: OAK_FENCE,
    },
    Wood {
        planks: SPRUCE_PLANKS,
        stairs: SPRUCE_STAIRS,
        slab: SPRUCE_SLAB,
        fence: SPRUCE_FENCE,
    },
    Wood {
        planks: DARK_OAK_PLANKS,
        stairs: DARK_OAK_STAIRS,
        slab: DARK_OAK_SLAB,
        fence: DARK_OAK_FENCE,
    },
];

pub(super) fn wood_for(seed: u64) -> Wood {
    WOODS[(mix(0, 0, seed ^ 0x3D00_D5E1) % WOODS.len() as u64) as usize]
}

/// A bed whose foot is at `foot` and whose head lies one cell along `toward_head`.
pub(super) fn bed(c: &mut Canvas, foot: (i32, i32), toward_head: (i32, i32), base: Block) -> bool {
    let head = (foot.0 + toward_head.0, foot.1 + toward_head.1);
    if !c.is_free(foot.0, foot.1) || !c.is_free(head.0, head.1) {
        return false;
    }
    let f = facing(toward_head);
    let y = c.floor_y + 1;
    for (cell, part) in [(foot, "foot"), (head, "head")] {
        c.put_with(
            cell.0,
            1,
            cell.1,
            cached_prop_block(
                base,
                &[("facing", f), ("part", part), ("occupied", "false")],
            ),
        );
        if c.editor
            .get_block_absolute(cell.0, y, cell.1)
            .map(|b| b.id())
            == Some(base.id())
        {
            c.editor.set_bed_block_entity_absolute(cell.0, y, cell.1);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_in_every_direction() {
        let b = Bounds {
            min_x: 10,
            min_z: 20,
            max_x: 17,
            max_z: 24,
        };
        for n in DIRS {
            let f = Frame::new(b, n);
            for x in b.min_x..=b.max_x {
                for z in b.min_z..=b.max_z {
                    let (u, v) = f.local(x, z);
                    assert!(u >= 0 && u < f.width, "{n:?} u {u}");
                    assert!(v >= 0 && v < f.depth, "{n:?} v {v}");
                    assert_eq!(f.world(u, v), (x, z));
                }
            }
            // v grows towards the back.
            let (x0, z0) = f.world(0, 0);
            let (x1, z1) = f.world(0, 1);
            assert_eq!((x1 - x0, z1 - z0), n);
        }
    }

    #[test]
    fn mix_spreads_neighbouring_cells() {
        let a = mix(0, 0, 1);
        let b = mix(1, 0, 1);
        let c = mix(0, 1, 1);
        assert!(a != b && b != c && a != c);
    }
}
