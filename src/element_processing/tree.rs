use crate::block_definitions::*;
use crate::deterministic_rng::coord_rng;
use crate::floodfill_cache::BuildingFootprintBitmap;
use crate::world_editor::WorldEditor;
use rand::Rng;

type Coord = (i32, i32, i32);

// Concentric rings added on top of the trunk column to bulk up the canopy.
#[rustfmt::skip]
const ROUND1_PATTERN: [Coord; 8] = [
    (-2, 0, 0),
    (2, 0, 0),
    (0, 0, -2),
    (0, 0, 2),
    (-1, 0, -1),
    (1, 0, 1),
    (1, 0, -1),
    (-1, 0, 1),
];

const ROUND2_PATTERN: [Coord; 12] = [
    (3, 0, 0),
    (2, 0, -1),
    (2, 0, 1),
    (1, 0, -2),
    (1, 0, 2),
    (-3, 0, 0),
    (-2, 0, -1),
    (-2, 0, 1),
    (-1, 0, 2),
    (-1, 0, -2),
    (0, 0, -3),
    (0, 0, 3),
];

const ROUND3_PATTERN: [Coord; 12] = [
    (3, 0, -1),
    (3, 0, 1),
    (2, 0, -2),
    (2, 0, 2),
    (1, 0, -3),
    (1, 0, 3),
    (-3, 0, -1),
    (-3, 0, 1),
    (-2, 0, -2),
    (-2, 0, 2),
    (-1, 0, 3),
    (-1, 0, -3),
];

const ROUND_PATTERNS: [&[Coord]; 3] = [&ROUND1_PATTERN, &ROUND2_PATTERN, &ROUND3_PATTERN];

// Leaves-fill data per (species, variant): y-axis columns around the trunk.
const OAK_LEAVES_FILL_STANDARD: [(Coord, Coord); 5] = [
    ((-1, 3, 0), (-1, 9, 0)),
    ((1, 3, 0), (1, 9, 0)),
    ((0, 3, -1), (0, 9, -1)),
    ((0, 3, 1), (0, 9, 1)),
    ((0, 9, 0), (0, 10, 0)),
];

const OAK_LEAVES_FILL_TALL_SLIM: [(Coord, Coord); 5] = [
    ((-1, 6, 0), (-1, 11, 0)),
    ((1, 6, 0), (1, 11, 0)),
    ((0, 6, -1), (0, 11, -1)),
    ((0, 6, 1), (0, 11, 1)),
    ((0, 11, 0), (0, 12, 0)),
];

const OAK_LEAVES_FILL_BUSHY: [(Coord, Coord); 5] = [
    ((-1, 3, 0), (-1, 7, 0)),
    ((1, 3, 0), (1, 7, 0)),
    ((0, 3, -1), (0, 7, -1)),
    ((0, 3, 1), (0, 7, 1)),
    ((0, 7, 0), (0, 8, 0)),
];

const OAK_LEAVES_FILL_COMPACT: [(Coord, Coord); 5] = [
    ((-1, 2, 0), (-1, 5, 0)),
    ((1, 2, 0), (1, 5, 0)),
    ((0, 2, -1), (0, 5, -1)),
    ((0, 2, 1), (0, 5, 1)),
    ((0, 5, 0), (0, 6, 0)),
];

// Spruce — three variants. Conifer cone shape with cross pattern.
const SPRUCE_LEAVES_FILL_STANDARD: [(Coord, Coord); 5] = [
    ((-1, 3, 0), (-1, 10, 0)),
    ((0, 3, -1), (0, 10, -1)),
    ((1, 3, 0), (1, 10, 0)),
    ((0, 3, 1), (0, 10, 1)),
    ((0, 11, 0), (0, 11, 0)),
];

const SPRUCE_LEAVES_FILL_TOWERING: [(Coord, Coord); 5] = [
    ((-1, 4, 0), (-1, 13, 0)),
    ((0, 4, -1), (0, 13, -1)),
    ((1, 4, 0), (1, 13, 0)),
    ((0, 4, 1), (0, 13, 1)),
    ((0, 14, 0), (0, 14, 0)),
];

const SPRUCE_LEAVES_FILL_SQUAT: [(Coord, Coord); 5] = [
    ((-1, 2, 0), (-1, 7, 0)),
    ((0, 2, -1), (0, 7, -1)),
    ((1, 2, 0), (1, 7, 0)),
    ((0, 2, 1), (0, 7, 1)),
    ((0, 8, 0), (0, 8, 0)),
];

// Birch — three variants. Tall and slender by default.
const BIRCH_LEAVES_FILL_STANDARD: [(Coord, Coord); 5] = [
    ((-1, 2, 0), (-1, 7, 0)),
    ((1, 2, 0), (1, 7, 0)),
    ((0, 2, -1), (0, 7, -1)),
    ((0, 2, 1), (0, 7, 1)),
    ((0, 7, 0), (0, 8, 0)),
];

const BIRCH_LEAVES_FILL_TALL: [(Coord, Coord); 5] = [
    ((-1, 5, 0), (-1, 10, 0)),
    ((1, 5, 0), (1, 10, 0)),
    ((0, 5, -1), (0, 10, -1)),
    ((0, 5, 1), (0, 10, 1)),
    ((0, 10, 0), (0, 11, 0)),
];

const BIRCH_LEAVES_FILL_CLUSTER: [(Coord, Coord); 5] = [
    ((-1, 3, 0), (-1, 5, 0)),
    ((1, 3, 0), (1, 5, 0)),
    ((0, 3, -1), (0, 5, -1)),
    ((0, 3, 1), (0, 5, 1)),
    ((0, 5, 0), (0, 6, 0)),
];

// Dark oak — three variants. Short trunk, wide canopy.
const DARK_OAK_LEAVES_FILL_STANDARD: [(Coord, Coord); 5] = [
    ((-1, 3, 0), (-1, 6, 0)),
    ((1, 3, 0), (1, 6, 0)),
    ((0, 3, -1), (0, 6, -1)),
    ((0, 3, 1), (0, 6, 1)),
    ((0, 6, 0), (0, 7, 0)),
];

const DARK_OAK_LEAVES_FILL_TALL_BUSHY: [(Coord, Coord); 5] = [
    ((-1, 4, 0), (-1, 9, 0)),
    ((1, 4, 0), (1, 9, 0)),
    ((0, 4, -1), (0, 9, -1)),
    ((0, 4, 1), (0, 9, 1)),
    ((0, 9, 0), (0, 10, 0)),
];

const DARK_OAK_LEAVES_FILL_STUNTED: [(Coord, Coord); 5] = [
    ((-1, 2, 0), (-1, 4, 0)),
    ((1, 2, 0), (1, 4, 0)),
    ((0, 2, -1), (0, 4, -1)),
    ((0, 2, 1), (0, 4, 1)),
    ((0, 4, 0), (0, 5, 0)),
];

// Jungle — two variants. Tall trunk, canopy near top.
const JUNGLE_LEAVES_FILL_STANDARD: [(Coord, Coord); 5] = [
    ((-1, 7, 0), (-1, 11, 0)),
    ((1, 7, 0), (1, 11, 0)),
    ((0, 7, -1), (0, 11, -1)),
    ((0, 7, 1), (0, 11, 1)),
    ((0, 11, 0), (0, 12, 0)),
];

const JUNGLE_LEAVES_FILL_BROAD: [(Coord, Coord); 5] = [
    ((-1, 8, 0), (-1, 12, 0)),
    ((1, 8, 0), (1, 12, 0)),
    ((0, 8, -1), (0, 12, -1)),
    ((0, 8, 1), (0, 12, 1)),
    ((0, 12, 0), (0, 13, 0)),
];

// Acacia — two variants. Umbrella canopy.
const ACACIA_LEAVES_FILL_STANDARD: [(Coord, Coord); 5] = [
    ((-1, 5, 0), (-1, 8, 0)),
    ((1, 5, 0), (1, 8, 0)),
    ((0, 5, -1), (0, 8, -1)),
    ((0, 5, 1), (0, 8, 1)),
    ((0, 8, 0), (0, 9, 0)),
];

const ACACIA_LEAVES_FILL_TALL: [(Coord, Coord); 5] = [
    ((-1, 7, 0), (-1, 10, 0)),
    ((1, 7, 0), (1, 10, 0)),
    ((0, 7, -1), (0, 10, -1)),
    ((0, 7, 1), (0, 10, 1)),
    ((0, 10, 0), (0, 10, 0)),
];

// Cherry — two variants.
const CHERRY_LEAVES_FILL_STANDARD: [(Coord, Coord); 5] = [
    ((-1, 4, 0), (-1, 9, 0)),
    ((1, 4, 0), (1, 9, 0)),
    ((0, 4, -1), (0, 9, -1)),
    ((0, 4, 1), (0, 9, 1)),
    ((0, 9, 0), (0, 10, 0)),
];

const CHERRY_LEAVES_FILL_WEEPING: [(Coord, Coord); 5] = [
    ((-1, 3, 0), (-1, 8, 0)),
    ((1, 3, 0), (1, 8, 0)),
    ((0, 3, -1), (0, 8, -1)),
    ((0, 3, 1), (0, 8, 1)),
    ((0, 8, 0), (0, 9, 0)),
];

// Tall oak — two variants. Extra-tall oak (kept for compatibility).
const TALL_OAK_LEAVES_FILL_STANDARD: [(Coord, Coord); 5] = [
    ((-1, 8, 0), (-1, 12, 0)),
    ((1, 8, 0), (1, 12, 0)),
    ((0, 8, -1), (0, 12, -1)),
    ((0, 8, 1), (0, 12, 1)),
    ((0, 12, 0), (0, 13, 0)),
];

const TALL_OAK_LEAVES_FILL_GIANT: [(Coord, Coord); 5] = [
    ((-1, 9, 0), (-1, 14, 0)),
    ((1, 9, 0), (1, 14, 0)),
    ((0, 9, -1), (0, 14, -1)),
    ((0, 9, 1), (0, 14, 1)),
    ((0, 14, 0), (0, 15, 0)),
];

// Pine — two variants. Narrow conifer using spruce blocks.
const PINE_LEAVES_FILL_STANDARD: [(Coord, Coord); 5] = [
    ((-1, 5, 0), (-1, 12, 0)),
    ((0, 5, -1), (0, 12, -1)),
    ((1, 5, 0), (1, 12, 0)),
    ((0, 5, 1), (0, 12, 1)),
    ((0, 13, 0), (0, 13, 0)),
];

const PINE_LEAVES_FILL_TALL: [(Coord, Coord); 5] = [
    ((-1, 6, 0), (-1, 15, 0)),
    ((0, 6, -1), (0, 15, -1)),
    ((1, 6, 0), (1, 15, 0)),
    ((0, 6, 1), (0, 15, 1)),
    ((0, 16, 0), (0, 16, 0)),
];

// Mangrove — tall, narrow tropical/swamp tree.
const MANGROVE_LEAVES_FILL: [(Coord, Coord); 5] = [
    ((-1, 5, 0), (-1, 10, 0)),
    ((1, 5, 0), (1, 10, 0)),
    ((0, 5, -1), (0, 10, -1)),
    ((0, 5, 1), (0, 10, 1)),
    ((0, 10, 0), (0, 11, 0)),
];

// Bush — no real trunk; small leaf clump at ground level.
const BUSH_LEAVES_FILL: [(Coord, Coord); 3] = [
    ((-1, 0, -1), (1, 1, 1)),
    ((-1, 2, 0), (1, 2, 0)),
    ((0, 2, -1), (0, 2, 1)),
];

// Willow — short trunk, wide canopy. Drooping tendrils added separately.
const WILLOW_LEAVES_FILL: [(Coord, Coord); 5] = [
    ((-1, 4, 0), (-1, 7, 0)),
    ((1, 4, 0), (1, 7, 0)),
    ((0, 4, -1), (0, 7, -1)),
    ((0, 4, 1), (0, 7, 1)),
    ((0, 7, 0), (0, 8, 0)),
];

const MAX_CANOPY_RADIUS: i32 = 3;

#[derive(Clone, Copy)]
pub enum TreeType {
    Oak,
    Spruce,
    Birch,
    DarkOak,
    Jungle,
    Acacia,
    Cherry,
    TallOak,
    Pine,
    Bush,
    AzaleaBush,
    Willow,
    FloweringOak,
    Mangrove,
}

pub struct Tree {
    log_block: Block,
    log_height: i32,
    leaves_block: Block,
    leaves_fill: &'static [(Coord, Coord)],
    round_ranges: [Vec<i32>; 3],
    branch_chance: f32,
    accent_block: Option<Block>,
    /// 0..100 percent chance per surface leaf to be the accent block.
    accent_chance: u8,
    drooping: bool,
}

struct LeafPlacer<'a> {
    leaves_block: Block,
    accent_block: Option<Block>,
    accent_chance: u8,
    check_collision: bool,
    footprints: Option<&'a BuildingFootprintBitmap>,
}

// Deterministic per-position hash driving the organic gap and accent rolls.
fn leaf_hash(x: i32, y: i32, z: i32) -> u64 {
    (x as i64 as u64).wrapping_mul(73856093)
        ^ (y as i64 as u64).wrapping_mul(19349663)
        ^ (z as i64 as u64).wrapping_mul(83492791)
}

// ~4% organic leaf gap, keyed on the position hash.
fn leaf_gap_at(h: u64) -> bool {
    h % 100 < 4
}

impl LeafPlacer<'_> {
    // Shared footprint gate: true if a building occupies (x, z).
    fn blocked(&self, x: i32, z: i32) -> bool {
        self.check_collision && self.footprints.is_some_and(|fp| fp.contains(x, z))
    }

    fn place_core(&self, editor: &mut WorldEditor, x: i32, y: i32, z: i32) {
        self.place_with(editor, x, y, z, false);
    }

    // Apex cap: the lone center-column cover over the trunk; skips the organic
    // gap (but not the footprint gate) so the log is never left exposed.
    fn place_apex_cap(&self, editor: &mut WorldEditor, x: i32, y: i32, z: i32) {
        if self.blocked(x, z) {
            return;
        }
        editor.set_block_absolute(self.leaves_block, x, y, z, None, None);
    }

    fn place_surface(&self, editor: &mut WorldEditor, x: i32, y: i32, z: i32) {
        self.place_with(editor, x, y, z, true);
    }

    fn place_with(&self, editor: &mut WorldEditor, x: i32, y: i32, z: i32, allow_accent: bool) {
        if self.blocked(x, z) {
            return;
        }
        let h = leaf_hash(x, y, z);
        if leaf_gap_at(h) {
            return;
        }
        let block = if allow_accent {
            if let Some(accent) = self.accent_block {
                let r2 = h.wrapping_mul(2654435761) % 100;
                if r2 < self.accent_chance as u64 {
                    accent
                } else {
                    self.leaves_block
                }
            } else {
                self.leaves_block
            }
        } else {
            self.leaves_block
        };
        editor.set_block_absolute(block, x, y, z, None, None);
    }
}

impl Tree {
    fn canopy_might_intersect_building(
        x: i32,
        z: i32,
        building_footprints: Option<&BuildingFootprintBitmap>,
    ) -> bool {
        let Some(footprints) = building_footprints else {
            return false;
        };

        for check_x in (x - MAX_CANOPY_RADIUS)..=(x + MAX_CANOPY_RADIUS) {
            for check_z in (z - MAX_CANOPY_RADIUS)..=(z + MAX_CANOPY_RADIUS) {
                if footprints.contains(check_x, check_z) {
                    return true;
                }
            }
        }

        false
    }

    /// Creates a tree at the specified coordinates.
    pub fn create(
        editor: &mut WorldEditor,
        (x, y, z): Coord,
        building_footprints: Option<&BuildingFootprintBitmap>,
    ) {
        let mut rng = coord_rng(x, z, 0);

        let tree_type = match rng.random_range(1..=100) {
            1..=20 => TreeType::Oak,
            21..=32 => TreeType::Spruce,
            33..=44 => TreeType::Birch,
            45..=50 => TreeType::DarkOak,
            51..=56 => TreeType::Jungle,
            57..=62 => TreeType::Acacia,
            63..=64 => TreeType::Cherry,
            65..=70 => TreeType::TallOak,
            71..=77 => TreeType::Pine,
            78..=84 => TreeType::Bush,
            85..=88 => TreeType::AzaleaBush,
            89..=92 => TreeType::Willow,
            93..=98 => TreeType::FloweringOak,
            99..=100 => TreeType::Mangrove,
            _ => unreachable!(),
        };

        Self::create_of_type(editor, (x, y, z), tree_type, building_footprints);
    }

    /// Creates a tree of a specific type at the specified coordinates.
    pub fn create_of_type(
        editor: &mut WorldEditor,
        (x, y, z): Coord,
        tree_type: TreeType,
        building_footprints: Option<&BuildingFootprintBitmap>,
    ) {
        if let Some(footprints) = building_footprints {
            if footprints.contains(x, z) {
                return;
            }
        }

        // Skip road/path/water surfaces.
        if editor.check_for_block(
            x,
            0,
            z,
            Some(&[
                BLACK_CONCRETE,
                GRAY_CONCRETE_POWDER,
                CYAN_TERRACOTTA,
                GRAY_CONCRETE,
                LIGHT_GRAY_CONCRETE,
                DIRT_PATH,
                SMOOTH_STONE,
                WATER,
            ]),
        ) {
            return;
        }

        let mut blacklist: Vec<Block> = Vec::new();
        blacklist.extend(Self::get_building_wall_blocks());
        blacklist.extend(Self::get_building_floor_blocks());
        blacklist.extend(Self::get_structural_blocks());
        blacklist.extend(Self::get_functional_blocks());
        blacklist.push(WATER);

        // Salt 1 keeps the shape RNG independent of the type-pick RNG.
        let mut shape_rng = coord_rng(x, z, 1);
        let variant_idx: u32 = shape_rng.random();

        let tree = Self::get_tree(tree_type, variant_idx);
        let check_canopy_collision =
            Self::canopy_might_intersect_building(x, z, building_footprints);

        // One base_y for the whole tree so the canopy doesn't warp to follow terrain.
        let base_y = editor.get_absolute_y(x, y, z);

        // Trunk jitter clamped so the canopy cap always sits above the trunk top.
        let height_jitter = ((variant_idx >> 8) & 0x3) as i32 - 1;
        let min_trunk = if tree.log_height == 0 { 0 } else { 2 };
        let canopy_top = tree
            .leaves_fill
            .iter()
            .map(|((_, _, _), (_, j2, _))| *j2)
            .max()
            .unwrap_or(0);
        let trunk_cap = (canopy_top - 1).max(min_trunk);
        let trunk_height = (tree.log_height + height_jitter)
            .max(min_trunk)
            .min(trunk_cap);

        if tree.log_height > 0 {
            editor.fill_blocks_absolute(
                tree.log_block,
                x,
                base_y,
                z,
                x,
                base_y + trunk_height,
                z,
                None,
                Some(&blacklist),
            );
        }

        let placer = LeafPlacer {
            leaves_block: tree.leaves_block,
            accent_block: tree.accent_block,
            accent_chance: tree.accent_chance,
            check_collision: check_canopy_collision,
            footprints: building_footprints,
        };

        // Inner canopy columns — accent only on the outermost ring (below).
        for ((i1, j1, k1), (i2, j2, k2)) in tree.leaves_fill {
            for leaf_x in (x + i1)..=(x + i2) {
                for leaf_y in (base_y + j1)..=(base_y + j2) {
                    for leaf_z in (z + k1)..=(z + k2) {
                        placer.place_core(editor, leaf_x, leaf_y, leaf_z);
                    }
                }
            }
        }

        // Force the apex so the organic gap never leaves the trunk top exposed.
        placer.place_apex_cap(editor, x, base_y + canopy_top, z);

        // Only the outermost non-empty ring gets surface (accent-eligible) leaves.
        let outermost_ring_idx: Option<usize> = tree
            .round_ranges
            .iter()
            .enumerate()
            .rev()
            .find(|(_, r)| !r.is_empty())
            .map(|(i, _)| i);
        for (idx, (round_range, round_pattern)) in
            tree.round_ranges.iter().zip(ROUND_PATTERNS).enumerate()
        {
            let is_surface = Some(idx) == outermost_ring_idx;
            for offset in round_range {
                for &(i, j, k) in round_pattern {
                    let lx = x + i;
                    let ly = base_y + offset + j;
                    let lz = z + k;
                    if is_surface {
                        placer.place_surface(editor, lx, ly, lz);
                    } else {
                        placer.place_core(editor, lx, ly, lz);
                    }
                }
            }
        }

        let branch_roll = ((variant_idx >> 16) & 0xFF) as f32 / 255.0;
        if branch_roll < tree.branch_chance && trunk_height >= 5 {
            let (dx, dz) = match (variant_idx >> 24) & 0x3 {
                0 => (1, 0),
                1 => (-1, 0),
                2 => (0, 1),
                _ => (0, -1),
            };
            let branch_y_off = trunk_height - 2 - ((variant_idx >> 12) & 0x1) as i32;
            let branch_y = base_y + branch_y_off;
            for step in 1..=2 {
                editor.set_block_absolute(
                    tree.log_block,
                    x + dx * step,
                    branch_y,
                    z + dz * step,
                    None,
                    Some(&blacklist),
                );
            }
            // Tapered cluster (Manhattan <= 2): rounder than a 3x3x3 cube.
            let tip_x = x + dx * 2;
            let tip_z = z + dz * 2;
            for ddx in -1i32..=1 {
                for ddy in -1i32..=1 {
                    for ddz in -1i32..=1 {
                        if ddx.abs() + ddy.abs() + ddz.abs() <= 2 {
                            placer.place_surface(editor, tip_x + ddx, branch_y + ddy, tip_z + ddz);
                        }
                    }
                }
            }
        }

        if tree.drooping {
            let droop_dirs: [(i32, i32); 8] = [
                (2, 0),
                (-2, 0),
                (0, 2),
                (0, -2),
                (1, 1),
                (-1, -1),
                (1, -1),
                (-1, 1),
            ];
            for (idx, &(dx, dz)) in droop_dirs.iter().enumerate() {
                let len_bits = ((variant_idx >> (idx as u32 * 2)) & 0x3) as i32;
                let droop_len = 2 + len_bits;
                let top = base_y + 5;
                for n in 0..droop_len {
                    placer.place_core(editor, x + dx, top - n, z + dz);
                }
            }
        }
    }

    fn get_tree(kind: TreeType, variant_idx: u32) -> Self {
        match kind {
            TreeType::Oak => match variant_idx % 5 {
                0 => Self::oak_standard(),
                1 => Self::oak_tall_slim(),
                2 => Self::oak_bushy(),
                3 => Self::oak_compact(),
                _ => Self::oak_lopsided(),
            },
            TreeType::Spruce => match variant_idx % 3 {
                0 => Self::spruce_standard(),
                1 => Self::spruce_towering(),
                _ => Self::spruce_squat(),
            },
            TreeType::Birch => match variant_idx % 3 {
                0 => Self::birch_standard(),
                1 => Self::birch_tall(),
                _ => Self::birch_cluster(),
            },
            TreeType::DarkOak => match variant_idx % 3 {
                0 => Self::dark_oak_standard(),
                1 => Self::dark_oak_tall_bushy(),
                _ => Self::dark_oak_stunted(),
            },
            TreeType::Jungle => match variant_idx % 2 {
                0 => Self::jungle_standard(),
                _ => Self::jungle_broad(),
            },
            TreeType::Acacia => match variant_idx % 2 {
                0 => Self::acacia_standard(),
                _ => Self::acacia_tall(),
            },
            TreeType::Cherry => match variant_idx % 2 {
                0 => Self::cherry_standard(),
                _ => Self::cherry_weeping(),
            },
            TreeType::TallOak => match variant_idx % 2 {
                0 => Self::tall_oak_standard(),
                _ => Self::tall_oak_giant(),
            },
            TreeType::Pine => match variant_idx % 2 {
                0 => Self::pine_standard(),
                _ => Self::pine_tall(),
            },
            TreeType::Bush => Self::bush(),
            TreeType::AzaleaBush => Self::azalea_bush(),
            TreeType::Willow => Self::willow(),
            TreeType::FloweringOak => Self::flowering_oak(),
            TreeType::Mangrove => Self::mangrove(),
        }
    }

    fn make(
        log_block: Block,
        log_height: i32,
        leaves_block: Block,
        leaves_fill: &'static [(Coord, Coord)],
        round_ranges: [Vec<i32>; 3],
    ) -> Self {
        Self {
            log_block,
            log_height,
            leaves_block,
            leaves_fill,
            round_ranges,
            branch_chance: 0.0,
            accent_block: None,
            accent_chance: 0,
            drooping: false,
        }
    }

    fn with_branch(mut self, chance: f32) -> Self {
        self.branch_chance = chance;
        self
    }

    fn with_accent(mut self, block: Block, chance: u8) -> Self {
        self.accent_block = Some(block);
        self.accent_chance = chance;
        self
    }

    fn drooping(mut self) -> Self {
        self.drooping = true;
        self
    }

    fn oak_standard() -> Self {
        Self::make(
            OAK_LOG,
            8,
            OAK_LEAVES,
            &OAK_LEAVES_FILL_STANDARD,
            [
                (3..=8).rev().collect(),
                (4..=7).rev().collect(),
                (5..=6).rev().collect(),
            ],
        )
        .with_branch(0.30)
    }

    fn oak_tall_slim() -> Self {
        Self::make(
            OAK_LOG,
            10,
            OAK_LEAVES,
            &OAK_LEAVES_FILL_TALL_SLIM,
            [(7..=11).rev().collect(), (8..=10).rev().collect(), vec![]],
        )
        .with_branch(0.40)
    }

    fn oak_bushy() -> Self {
        Self::make(
            OAK_LOG,
            6,
            OAK_LEAVES,
            &OAK_LEAVES_FILL_BUSHY,
            [
                (3..=7).rev().collect(),
                (3..=6).rev().collect(),
                (4..=5).rev().collect(),
            ],
        )
        .with_branch(0.20)
    }

    fn oak_compact() -> Self {
        Self::make(
            OAK_LOG,
            5,
            OAK_LEAVES,
            &OAK_LEAVES_FILL_COMPACT,
            [(2..=5).rev().collect(), (3..=4).rev().collect(), vec![]],
        )
    }

    // Standard oak silhouette with a guaranteed side branch (the branch is the asymmetry).
    fn oak_lopsided() -> Self {
        Self::make(
            OAK_LOG,
            8,
            OAK_LEAVES,
            &OAK_LEAVES_FILL_STANDARD,
            [
                (3..=8).rev().collect(),
                (4..=7).rev().collect(),
                (5..=6).rev().collect(),
            ],
        )
        .with_branch(1.0)
    }

    fn spruce_standard() -> Self {
        Self::make(
            SPRUCE_LOG,
            9,
            SPRUCE_LEAVES,
            &SPRUCE_LEAVES_FILL_STANDARD,
            [vec![9, 7, 6, 4, 3], vec![6, 3], vec![]],
        )
    }

    fn spruce_towering() -> Self {
        Self::make(
            SPRUCE_LOG,
            12,
            SPRUCE_LEAVES,
            &SPRUCE_LEAVES_FILL_TOWERING,
            [vec![12, 10, 8, 6, 4], vec![9, 6, 4], vec![]],
        )
    }

    fn spruce_squat() -> Self {
        Self::make(
            SPRUCE_LOG,
            6,
            SPRUCE_LEAVES,
            &SPRUCE_LEAVES_FILL_SQUAT,
            [vec![6, 4, 3], vec![4, 2], vec![3]],
        )
    }

    fn birch_standard() -> Self {
        Self::make(
            BIRCH_LOG,
            6,
            BIRCH_LEAVES,
            &BIRCH_LEAVES_FILL_STANDARD,
            [(2..=6).rev().collect(), (2..=4).collect(), vec![]],
        )
        .with_branch(0.20)
    }

    fn birch_tall() -> Self {
        Self::make(
            BIRCH_LOG,
            9,
            BIRCH_LEAVES,
            &BIRCH_LEAVES_FILL_TALL,
            [(5..=9).rev().collect(), (6..=8).rev().collect(), vec![]],
        )
        .with_branch(0.25)
    }

    fn birch_cluster() -> Self {
        Self::make(
            BIRCH_LOG,
            4,
            BIRCH_LEAVES,
            &BIRCH_LEAVES_FILL_CLUSTER,
            [(2..=4).rev().collect(), vec![3], vec![]],
        )
    }

    fn dark_oak_standard() -> Self {
        Self::make(
            DARK_OAK_LOG,
            5,
            DARK_OAK_LEAVES,
            &DARK_OAK_LEAVES_FILL_STANDARD,
            [
                (3..=6).rev().collect(),
                (3..=5).rev().collect(),
                (4..=5).rev().collect(),
            ],
        )
        .with_branch(0.40)
    }

    fn dark_oak_tall_bushy() -> Self {
        Self::make(
            DARK_OAK_LOG,
            8,
            DARK_OAK_LEAVES,
            &DARK_OAK_LEAVES_FILL_TALL_BUSHY,
            [
                (4..=9).rev().collect(),
                (5..=8).rev().collect(),
                (6..=7).rev().collect(),
            ],
        )
        .with_branch(0.50)
    }

    fn dark_oak_stunted() -> Self {
        Self::make(
            DARK_OAK_LOG,
            3,
            DARK_OAK_LEAVES,
            &DARK_OAK_LEAVES_FILL_STUNTED,
            [vec![3, 2], vec![2], vec![]],
        )
    }

    fn jungle_standard() -> Self {
        Self::make(
            JUNGLE_LOG,
            10,
            JUNGLE_LEAVES,
            &JUNGLE_LEAVES_FILL_STANDARD,
            [(7..=11).rev().collect(), (8..=10).rev().collect(), vec![]],
        )
        .with_branch(0.50)
    }

    fn jungle_broad() -> Self {
        Self::make(
            JUNGLE_LOG,
            11,
            JUNGLE_LEAVES,
            &JUNGLE_LEAVES_FILL_BROAD,
            [
                (8..=12).rev().collect(),
                (9..=11).rev().collect(),
                (10..=10).rev().collect(),
            ],
        )
        .with_branch(0.60)
    }

    fn acacia_standard() -> Self {
        Self::make(
            ACACIA_LOG,
            6,
            ACACIA_LEAVES,
            &ACACIA_LEAVES_FILL_STANDARD,
            [
                (5..=8).rev().collect(),
                (5..=7).rev().collect(),
                (6..=7).rev().collect(),
            ],
        )
        .with_branch(0.35)
    }

    fn acacia_tall() -> Self {
        Self::make(
            ACACIA_LOG,
            8,
            ACACIA_LEAVES,
            &ACACIA_LEAVES_FILL_TALL,
            [(7..=10).rev().collect(), (8..=9).rev().collect(), vec![9]],
        )
        .with_branch(0.45)
    }

    fn cherry_standard() -> Self {
        Self::make(
            CHERRY_LOG,
            7,
            CHERRY_LEAVES,
            &CHERRY_LEAVES_FILL_STANDARD,
            [
                (4..=9).rev().collect(),
                (5..=8).rev().collect(),
                (6..=7).rev().collect(),
            ],
        )
        .with_branch(0.30)
    }

    fn cherry_weeping() -> Self {
        Self::make(
            CHERRY_LOG,
            6,
            CHERRY_LEAVES,
            &CHERRY_LEAVES_FILL_WEEPING,
            [
                (3..=8).rev().collect(),
                (4..=7).rev().collect(),
                (5..=6).rev().collect(),
            ],
        )
        .drooping()
    }

    fn tall_oak_standard() -> Self {
        Self::make(
            OAK_LOG,
            11,
            OAK_LEAVES,
            &TALL_OAK_LEAVES_FILL_STANDARD,
            [(8..=12).rev().collect(), (9..=11).rev().collect(), vec![10]],
        )
        .with_branch(0.40)
    }

    fn tall_oak_giant() -> Self {
        Self::make(
            OAK_LOG,
            13,
            OAK_LEAVES,
            &TALL_OAK_LEAVES_FILL_GIANT,
            [
                (9..=14).rev().collect(),
                (10..=13).rev().collect(),
                (11..=12).rev().collect(),
            ],
        )
        .with_branch(0.60)
    }

    fn pine_standard() -> Self {
        Self::make(
            SPRUCE_LOG,
            12,
            SPRUCE_LEAVES,
            &PINE_LEAVES_FILL_STANDARD,
            [vec![11, 9, 7, 5], vec![8, 5], vec![]],
        )
    }

    fn pine_tall() -> Self {
        Self::make(
            SPRUCE_LOG,
            15,
            SPRUCE_LEAVES,
            &PINE_LEAVES_FILL_TALL,
            [vec![14, 12, 10, 8, 6], vec![11, 7], vec![]],
        )
    }

    // log_height == 0 → no trunk placed.
    fn bush() -> Self {
        Self::make(
            OAK_LOG,
            0,
            OAK_LEAVES,
            &BUSH_LEAVES_FILL,
            [vec![], vec![], vec![]],
        )
    }

    fn azalea_bush() -> Self {
        Self::make(
            OAK_LOG,
            0,
            AZALEA_LEAVES,
            &BUSH_LEAVES_FILL,
            [vec![], vec![], vec![]],
        )
    }

    fn willow() -> Self {
        Self::make(
            OAK_LOG,
            5,
            OAK_LEAVES,
            &WILLOW_LEAVES_FILL,
            [(4..=6).rev().collect(), (4..=5).rev().collect(), vec![5]],
        )
        .drooping()
    }

    // Oak silhouette with cherry-pink blossom accents on the outermost ring.
    fn flowering_oak() -> Self {
        Self::make(
            OAK_LOG,
            8,
            OAK_LEAVES,
            &OAK_LEAVES_FILL_STANDARD,
            [
                (3..=8).rev().collect(),
                (4..=7).rev().collect(),
                (5..=6).rev().collect(),
            ],
        )
        .with_branch(0.40)
        .with_accent(CHERRY_LEAVES, 18)
    }

    fn mangrove() -> Self {
        Self::make(
            MANGROVE_LOG,
            8,
            MANGROVE_LEAVES,
            &MANGROVE_LEAVES_FILL,
            [
                (5..=10).rev().collect(),
                (6..=9).rev().collect(),
                (7..=8).rev().collect(),
            ],
        )
        .with_branch(0.55)
    }

    /// Get all possible building wall blocks
    fn get_building_wall_blocks() -> Vec<Block> {
        vec![
            BLACKSTONE,
            BLACK_TERRACOTTA,
            BRICK,
            BROWN_CONCRETE,
            BROWN_TERRACOTTA,
            DEEPSLATE_BRICKS,
            END_STONE_BRICKS,
            GRAY_CONCRETE,
            GRAY_TERRACOTTA,
            LIGHT_BLUE_TERRACOTTA,
            LIGHT_GRAY_CONCRETE,
            MUD_BRICKS,
            NETHER_BRICK,
            NETHERITE_BLOCK,
            POLISHED_ANDESITE,
            POLISHED_BLACKSTONE,
            POLISHED_BLACKSTONE_BRICKS,
            POLISHED_DEEPSLATE,
            POLISHED_GRANITE,
            QUARTZ_BLOCK,
            QUARTZ_BRICKS,
            SANDSTONE,
            SMOOTH_SANDSTONE,
            SMOOTH_STONE,
            STONE_BRICKS,
            WHITE_CONCRETE,
            WHITE_TERRACOTTA,
            ORANGE_TERRACOTTA,
            GREEN_STAINED_HARDENED_CLAY,
            BLUE_TERRACOTTA,
            YELLOW_TERRACOTTA,
            BLACK_CONCRETE,
            GRAY_CONCRETE_POWDER,
            CYAN_TERRACOTTA,
            WHITE_CONCRETE,
            GRAY_CONCRETE,
            LIGHT_GRAY_CONCRETE,
            BROWN_CONCRETE,
            RED_CONCRETE,
            ORANGE_TERRACOTTA,
            YELLOW_CONCRETE,
            LIME_CONCRETE,
            GREEN_STAINED_HARDENED_CLAY,
            CYAN_CONCRETE,
            LIGHT_BLUE_CONCRETE,
            BLUE_CONCRETE,
            PURPLE_CONCRETE,
            MAGENTA_CONCRETE,
            RED_TERRACOTTA,
        ]
    }

    /// Get all possible building floor blocks
    fn get_building_floor_blocks() -> Vec<Block> {
        vec![
            GRAY_CONCRETE,
            LIGHT_GRAY_CONCRETE,
            WHITE_CONCRETE,
            SMOOTH_STONE,
            POLISHED_ANDESITE,
            STONE_BRICKS,
        ]
    }

    /// Get structural blocks (fences, walls, stairs, slabs, rails, etc.)
    fn get_structural_blocks() -> Vec<Block> {
        vec![
            // Fences
            OAK_FENCE,
            // Walls
            COBBLESTONE_WALL,
            ANDESITE_WALL,
            STONE_BRICK_WALL,
            // Stairs
            OAK_STAIRS,
            // Slabs
            OAK_SLAB,
            STONE_BLOCK_SLAB,
            STONE_BRICK_SLAB,
            // Rails
            RAIL,
            RAIL_NORTH_SOUTH,
            RAIL_EAST_WEST,
            RAIL_ASCENDING_EAST,
            RAIL_ASCENDING_WEST,
            RAIL_ASCENDING_NORTH,
            RAIL_ASCENDING_SOUTH,
            RAIL_NORTH_EAST,
            RAIL_NORTH_WEST,
            RAIL_SOUTH_EAST,
            RAIL_SOUTH_WEST,
            // Doors and trapdoors
            OAK_DOOR,
            DARK_OAK_DOOR_LOWER,
            DARK_OAK_DOOR_UPPER,
            OAK_TRAPDOOR,
            // Ladders
            LADDER,
        ]
    }

    /// Get functional blocks (furniture, decorative items, etc.)
    fn get_functional_blocks() -> Vec<Block> {
        vec![
            // Furniture and functional blocks
            CHEST,
            CRAFTING_TABLE,
            FURNACE,
            ANVIL,
            BREWING_STAND,
            NOTE_BLOCK,
            BOOKSHELF,
            CAULDRON,
            // Beds
            RED_BED_NORTH_HEAD,
            RED_BED_NORTH_FOOT,
            RED_BED_EAST_HEAD,
            RED_BED_EAST_FOOT,
            RED_BED_SOUTH_HEAD,
            RED_BED_SOUTH_FOOT,
            RED_BED_WEST_HEAD,
            RED_BED_WEST_FOOT,
            // Pressure plates and signs
            OAK_PRESSURE_PLATE,
            SIGN,
            // Glass blocks (windows)
            GLASS,
            WHITE_STAINED_GLASS,
            GRAY_STAINED_GLASS,
            LIGHT_GRAY_STAINED_GLASS,
            BROWN_STAINED_GLASS,
            CYAN_STAINED_GLASS,
            BLUE_STAINED_GLASS,
            LIGHT_BLUE_STAINED_GLASS,
            TINTED_GLASS,
            // Carpets
            WHITE_CARPET,
            RED_CARPET,
            // Other structural/building blocks
            IRON_BARS,
            IRON_BLOCK,
            SCAFFOLDING,
            BEDROCK,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;

    // The apex cap must place even on a cell the ~4% organic gap skips.
    #[test]
    fn place_apex_cap_fills_the_organic_gap() {
        let xzbbox = XZBBox::rect_from_min_max(0, 0, 63, 63).unwrap();
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        // Editor is never saved here; a temp dir keeps the path valid + portable.
        let mut editor = WorldEditor::new(std::env::temp_dir(), &xzbbox, llbbox);

        let placer = LeafPlacer {
            leaves_block: OAK_LEAVES,
            accent_block: None,
            accent_chance: 0,
            check_collision: false,
            footprints: None,
        };

        // Pick a gap cell via the same predicate place_with uses (no drift).
        let (gx, gz) = (0..64)
            .flat_map(|x| (0..64).map(move |z| (x, z)))
            .find(|&(x, z)| leaf_gap_at(leaf_hash(x, 10, z)))
            .expect("a 4% gap cell exists in 64x64");

        placer.place_core(&mut editor, gx, 10, gz);
        assert!(
            !editor.check_for_block_absolute(gx, 10, gz, Some(&[OAK_LEAVES]), None),
            "the organic gap should skip this cell"
        );
        placer.place_apex_cap(&mut editor, gx, 10, gz);
        assert!(
            editor.check_for_block_absolute(gx, 10, gz, Some(&[OAK_LEAVES]), None),
            "apex cap must place despite the organic gap"
        );
    }
}
