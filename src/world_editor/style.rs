//! World styles, applied to each finished region just before it is written: the ruins of
//! a post-apocalyptic world and the snow and ice of a winter one. They work on the final
//! blocks, so they reach everything placed before, trees, roofs and landmarks alike.
use std::sync::{Arc, OnceLock};

use fastnbt::Value;

use super::common::{world_max_y, ChunkToModify, SectionToModify};
use super::WorldEditor;
use crate::block_definitions::{
    intern_props, Block, BlockWithProperties, AIR, ANDESITE, BIRCH_LEAVES, BLACK_CONCRETE, BRICK,
    BROWN_TERRACOTTA, CALCITE, COARSE_DIRT, COBBLESTONE, COBBLESTONE_SLAB, COBBLESTONE_STAIRS,
    COBBLESTONE_WALL, COBWEB, CRACKED_DEEPSLATE_BRICKS, CRACKED_DEEPSLATE_TILES,
    CRACKED_NETHER_BRICKS, CRACKED_POLISHED_BLACKSTONE_BRICKS, CRACKED_STONE_BRICKS, DEAD_BUSH,
    DEEPSLATE_BRICKS, DEEPSLATE_TILES, DIORITE, DIRT, DIRT_PATH, FARMLAND, FIRE, GRANITE, GRASS,
    GRASS_BLOCK, GRAVEL, GRAY_CONCRETE, GRAY_TERRACOTTA, ICE, LIGHT_GRAY_CONCRETE,
    MOSSY_COBBLESTONE, MOSSY_COBBLESTONE_SLAB, MOSSY_COBBLESTONE_STAIRS, MOSSY_COBBLESTONE_WALL,
    MOSSY_STONE_BRICKS, MOSSY_STONE_BRICK_SLAB, MOSSY_STONE_BRICK_STAIRS, MOSSY_STONE_BRICK_WALL,
    MOSS_BLOCK, MOSS_CARPET, MUD, NETHERRACK, NETHER_BRICK, OAK_LEAVES, OAK_LOG, OAK_PLANKS,
    PODZOL, POLISHED_ANDESITE, POLISHED_BLACKSTONE_BRICKS, QUARTZ_BLOCK, RED_TERRACOTTA, SANDSTONE,
    SMOOTH_QUARTZ, SMOOTH_QUARTZ_SLAB, SMOOTH_QUARTZ_STAIRS, SMOOTH_SANDSTONE, SMOOTH_STONE,
    SNOWY_GRASS_BLOCK, SNOWY_PODZOL, SNOW_LAYERS, SPRUCE_PLANKS, STONE, STONE_BRICKS,
    STONE_BRICK_SLAB, STONE_BRICK_STAIRS, STONE_BRICK_WALL, TALL_GRASS_BOTTOM, TALL_GRASS_TOP,
    TERRACOTTA, TINTED_GLASS, TUFF, VINES, WHITE_CONCRETE, WHITE_TERRACOTTA,
};
use crate::coordinate_system::cartesian::XZPoint;
use crate::floodfill_cache::{CoordinateBitmap, SealedSurfaceBitmap};
use crate::ground::Ground;
use crate::ground_generation::patch_noise;
use crate::land_cover::{coord_hash, LC_WATER};

const SALT_JAG: u32 = 0x4A41_4747;
const SALT_HOLE: u32 = 0x484F_4C45;
const SALT_CRACK: u32 = 0x4352_4B53;
const SALT_RUBBLE: u32 = 0x5242_4C45;
const SALT_DEPTH: u32 = 0x534E_4F57;
const SALT_PLOW: u32 = 0x504C_4F57;
const SALT_ICE: u32 = 0x4943_4521;
const SALT_VINE: u32 = 0x5649_4E45;
const SALT_MOSS: u32 = 0x4D4F_5353;
const SALT_ROOF: u32 = 0x524F_4F46;
const SALT_DECAY: u32 = 0x4445_4359;
const SALT_BREAK: u32 = 0x4252_4B31;
const SALT_FLOORS: u32 = 0x464C_5253;
const SALT_ROOF_HOLE: u32 = 0x5248_4F4C;
const SALT_GROWTH: u32 = 0x4752_4F57;

/// Share of street-facing wall that vines climb.
const VINE_SHARE: f64 = 0.45;
/// The highest a patch of vines climbs, in blocks.
const VINE_REACH: f64 = 34.0;

/// Share of slab and stair roofs, which snow cannot rest on, that turn white instead.
const WHITE_ROOF_SHARE: f64 = 0.85;

/// How far into the street a ruin's rubble spreads, in blocks.
const RUBBLE_REACH: i32 = 3;
/// One rubble pile in this many burns: netherrack with a fire on it that never goes out.
/// Rare, since these ruins have stood empty for a long time.
const FIRE_ODDS: u64 = 500;
/// How high moss creeps up a ruin's walls.
const MOSS_REACH: i32 = 9;
/// As far as a building's corner is looked for: past this it is one building no more.
const CORNER_WALK: u32 = 512;
/// From this height a building stays a landmark of the skyline, however far gone.
const TOWER_HEIGHT: i32 = 40;
/// The least of a tower's height left standing.
const TOWER_SHARE: f64 = 0.6;
/// One open-air cell in a ruin in this many has a tree growing in it.
const RUIN_TREE_ODDS: u64 = 170;
/// One chunk with a clear stretch of road in this many has a wrecked car on it.
const WRECK_ODDS: u64 = 5;
/// Lamps stand on short posts, so lights are looked for this far above the ground.
const LAMP_REACH: i32 = 12;

/// Which styles a run applies, and what they need from the generation.
pub(crate) struct WorldStyle<'a> {
    pub winter: bool,
    pub ruins: bool,
    /// Building footprints, which the ruins break open and pile rubble around.
    pub footprints: Option<&'a CoordinateBitmap>,
    /// A ground marking snow leaves bare: the blueprint's building outline.
    pub bare: Option<Block>,
}

impl WorldStyle<'_> {
    pub(crate) fn is_active(&self) -> bool {
        self.winter || self.ruins
    }
}

/// What a block is to the styles, looked up once per block id.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Air,
    Water,
    /// Snow already, or ice, which drops any layer laid on it.
    Snowy,
    /// Small plants: snow buries them.
    Plant,
    /// Plants two blocks tall, whose upper half stands on a lower half of the same name.
    TallPlant,
    Leaves,
    Glass,
    Light,
    /// Panes, fences, rails, torches and the like: nothing settles on them.
    Thin,
    /// Stairs and slabs: snow settles on them, but swapping them for a cube would change
    /// their shape.
    Shaped,
    Solid,
}

fn kind(block: Block) -> Kind {
    static TABLE: OnceLock<Vec<Kind>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        (0..=u16::MAX)
            .map(|id| {
                Block::from_raw_id(id)
                    .try_name()
                    .map_or(Kind::Solid, kind_of)
            })
            .collect()
    });
    table[usize::from(block.id())]
}

const SMALL_PLANTS: [&str; 32] = [
    "short_grass",
    "grass",
    "fern",
    "dead_bush",
    "bush",
    "firefly_bush",
    "short_dry_grass",
    "sweet_berry_bush",
    "wheat",
    "carrots",
    "potatoes",
    "beetroots",
    "dandelion",
    "poppy",
    "blue_orchid",
    "allium",
    "azure_bluet",
    "red_tulip",
    "orange_tulip",
    "white_tulip",
    "pink_tulip",
    "oxeye_daisy",
    "cornflower",
    "lily_of_the_valley",
    "pink_petals",
    "wildflowers",
    "leaf_litter",
    "torchflower",
    "open_eyeblossom",
    "closed_eyeblossom",
    "brown_mushroom",
    "red_mushroom",
];

const TALL_PLANTS: [&str; 8] = [
    "tall_grass",
    "large_fern",
    "sunflower",
    "lilac",
    "rose_bush",
    "peony",
    "tall_dry_grass",
    "pitcher_plant",
];

const LIGHTS: [&str; 8] = [
    "fire",
    "soul_fire",
    "glowstone",
    "sea_lantern",
    "shroomlight",
    "redstone_lamp",
    "jack_o_lantern",
    "end_rod",
];

/// Parts of names that mark a block nothing can settle on.
const THIN: [&str; 50] = [
    "fence",
    "_wall",
    "door",
    "sign",
    "banner",
    "button",
    "lever",
    "rail",
    "carpet",
    "pressure_plate",
    "ladder",
    "vine",
    "chain",
    "bars",
    "_head",
    "skull",
    "potted",
    "flower_pot",
    "bell",
    "anvil",
    "_bed",
    "chest",
    "cake",
    "campfire",
    "hopper",
    "cauldron",
    "grindstone",
    "lectern",
    "scaffolding",
    "cobweb",
    "lily_pad",
    "kelp",
    "seagrass",
    "coral",
    "pickle",
    "lichen",
    "bamboo",
    "sugar_cane",
    "cactus",
    "dripstone",
    "dripleaf",
    "azalea",
    "amethyst_bud",
    "amethyst_cluster",
    "sculk_vein",
    "sculk_sensor",
    "sculk_shrieker",
    "daylight_detector",
    "brewing_stand",
    "composter",
];

fn kind_of(name: &str) -> Kind {
    match name {
        "air" | "cave_air" | "void_air" | "light" | "barrier" | "structure_void" => Kind::Air,
        "water" | "bubble_column" => Kind::Water,
        "snow" | "snow_block" | "powder_snow" | "ice" | "packed_ice" | "blue_ice" => Kind::Snowy,
        _ if TALL_PLANTS.contains(&name) => Kind::TallPlant,
        _ if SMALL_PLANTS.contains(&name) || name.ends_with("_sapling") => Kind::Plant,
        _ if name.ends_with("_leaves") => Kind::Leaves,
        _ if name.contains("glass") => Kind::Glass,
        _ if LIGHTS.contains(&name)
            || name.ends_with("lantern")
            || name.ends_with("froglight")
            || name.contains("torch")
            || name.ends_with("candle") =>
        {
            Kind::Light
        }
        _ if THIN.iter().any(|part| name.contains(part)) => Kind::Thin,
        _ if name.ends_with("_stairs") || name.ends_with("_slab") => Kind::Shaped,
        _ => Kind::Solid,
    }
}

/// Parts of names that mark a block fire can catch: wood of every kind, cloth, hay.
const FLAMMABLE: [&str; 27] = [
    "oak",
    "spruce",
    "birch",
    "jungle",
    "acacia",
    "mangrove",
    "cherry",
    "bamboo",
    "planks",
    "log",
    "wood",
    "wool",
    "carpet",
    "vine",
    "hay",
    "bookshelf",
    "barrel",
    "scaffolding",
    "lectern",
    "composter",
    "beehive",
    "bee_nest",
    "target",
    "tnt",
    "coal_block",
    "moss",
    "azalea",
];

fn flammable(block: Block) -> bool {
    static TABLE: OnceLock<Vec<bool>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        (0..=u16::MAX)
            .map(|id| {
                Block::from_raw_id(id).try_name().is_some_and(|name| {
                    matches!(kind_of(name), Kind::Plant | Kind::TallPlant | Kind::Leaves)
                        || FLAMMABLE.iter().any(|part| name.contains(part))
                })
            })
            .collect()
    });
    table[usize::from(block.id())]
}

/// What the styles read about the terrain under a region.
struct Terrain<'a> {
    ground: Option<&'a Ground>,
    origin: (i32, i32),
    overrides: &'a super::CoordMap<i32>,
    sealed: Option<&'a SealedSurfaceBitmap>,
}

impl Terrain<'_> {
    fn ground_y(&self, x: i32, z: i32) -> i32 {
        if let Some(&y) = self.overrides.get(&(x, z)) {
            return y;
        }
        self.ground.map_or(0, |g| g.level(self.point(x, z)))
    }

    fn point(&self, x: i32, z: i32) -> XZPoint {
        XZPoint::new(x - self.origin.0, z - self.origin.1)
    }

    fn sealed(&self, x: i32, z: i32) -> bool {
        self.sealed.is_some_and(|m| m.contains(x, z))
    }

    /// Water freezes along every shore and across anything narrow, but a lake or the sea
    /// stays open beyond a ragged edge of ice.
    fn freezes(&self, x: i32, z: i32) -> bool {
        let Some(g) = self.ground else {
            return true;
        };
        let p = self.point(x, z);
        if g.cover_class(p) != LC_WATER {
            return true;
        }
        let reach = 5 + (patch_noise(x, z, 16, SALT_ICE) * 10.0) as u8;
        let d = g.water_distance(p);
        d != 0 && d <= reach
    }
}

impl WorldEditor<'_> {
    /// Applies the run's styles to one region. Call once per region, after its last block
    /// is placed and before it is written.
    pub(crate) fn style_region(&mut self, rx: i32, rz: i32, style: &WorldStyle) {
        if !style.is_active() {
            return;
        }
        let terrain = Terrain {
            ground: self.ground.as_deref(),
            origin: (self.ground_origin_x, self.ground_origin_z),
            overrides: &self.road_surface_overrides,
            sealed: self.sealed_surface.as_deref(),
        };
        let Some(region) = self.world.regions.get_mut(&(rx, rz)) else {
            return;
        };
        let tops = style
            .footprints
            .filter(|_| style.ruins)
            .map(|f| RegionTops::build((rx << 9, rz << 9), region.chunks.iter(), f));
        for ((cx, cz), chunk) in region.chunks.iter_mut() {
            let base = (((rx << 5) + cx) << 4, ((rz << 5) + cz) << 4);
            let columns: Vec<(Column, i32)> = (0..16u8)
                .flat_map(|lz| (0..16u8).map(move |lx| (lx, lz)))
                .map(|(lx, lz)| {
                    let (x, z) = (base.0 + i32::from(lx), base.1 + i32::from(lz));
                    (Column { lx, lz, x, z }, terrain.ground_y(x, z))
                })
                .collect();
            if style.ruins {
                let in_building =
                    |col: &Column| style.footprints.is_some_and(|f| f.contains(col.x, col.z));
                let mut fires = Vec::new();
                let plans = style
                    .footprints
                    .zip(tops.as_ref())
                    .map(|(f, tops)| plan_ruins(&columns, f, tops));
                for &(col, ground_y) in &columns {
                    let plan = plans
                        .as_ref()
                        .and_then(|p| p[usize::from(col.lz)][usize::from(col.lx)]);
                    if let Some(plan) = plan.filter(|_| in_building(&col)) {
                        ruin_building(chunk, col, ground_y, plan, &mut fires);
                    } else {
                        ruin_open_ground(chunk, col, ground_y, &terrain, style, &mut fires);
                    }
                }
                // Once every wall of the chunk has come down as far as it will.
                if let (Some(footprints), Some(plans)) = (style.footprints, &plans) {
                    for &(col, ground_y) in &columns {
                        if !in_building(&col) {
                            drop_overhangs(chunk, col, ground_y, plans);
                            hang_vines(chunk, col, ground_y, footprints);
                        }
                    }
                    wreck_a_car(chunk, base, &columns, &terrain, footprints);
                }
                // Last, so the clearance check sees every plant and vine of the chunk.
                for spot in fires {
                    light_fire(chunk, spot);
                }
            }
            if style.winter {
                for &(col, ground_y) in &columns {
                    snow_over(chunk, col, ground_y, &terrain, style.bare);
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Column {
    lx: u8,
    lz: u8,
    x: i32,
    z: i32,
}

impl Column {
    fn get(self, chunk: &ChunkToModify, y: i32) -> Option<Block> {
        chunk.get_block(self.lx, y, self.lz)
    }

    fn set(self, chunk: &mut ChunkToModify, y: i32, block: Block) {
        chunk.set_block(self.lx, y, self.lz, block);
    }

    /// The state of the block at `y`: what was stored with it, or else what its id bakes in.
    fn properties(self, chunk: &ChunkToModify, y: i32, block: Block) -> Option<Arc<Value>> {
        let index = SectionToModify::index(self.lx, (y & 15) as u8, self.lz);
        chunk
            .sections
            .get((y >> 4) as i8)
            .and_then(|s| s.properties.get(&index).cloned())
            .or_else(|| block.properties().map(intern_props))
    }

    /// Swaps the block at `y` for a variant of it, keeping the facing and shape it stood in.
    fn swap(self, chunk: &mut ChunkToModify, y: i32, block: Block) {
        let props = self
            .get(chunk, y)
            .and_then(|old| self.properties(chunk, y, old));
        chunk.set_block_with_properties(
            self.lx,
            y,
            self.lz,
            BlockWithProperties::from_arc(block, props),
        );
    }

    /// Per-block hash for this column at height `y`.
    fn hash(self, y: i32) -> u64 {
        coord_hash(
            self.x ^ y.wrapping_mul(0x9E37),
            self.z ^ y.wrapping_mul(0x85EB),
        )
    }

    /// Highest block of the column and its height.
    fn top(self, chunk: &ChunkToModify) -> Option<(i32, Block)> {
        self.top_in(chunk, &sections_down(chunk))
    }

    /// The same, given the chunk's sections from the top down.
    fn top_in(self, chunk: &ChunkToModify, sections: &[i8]) -> Option<(i32, Block)> {
        for &sy in sections {
            let Some(section) = chunk.sections.get(sy) else {
                continue;
            };
            for ly in (0..16u8).rev() {
                if let Some(b) = section.get_block(self.lx, ly, self.lz) {
                    if kind(b) != Kind::Air {
                        return Some((i32::from(sy) * 16 + i32::from(ly), b));
                    }
                }
            }
        }
        None
    }
}

/// A chunk's sections, highest first.
fn sections_down(chunk: &ChunkToModify) -> Vec<i8> {
    let mut sections: Vec<i8> = chunk.sections.keys().collect();
    sections.sort_unstable_by(|a, b| b.cmp(a));
    sections
}

/// How far gone a building is. It is much the same all across one, so it still reads as a
/// building, and differs from its neighbour's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Decay {
    /// Roof and walls up, windows out, the odd hole in the roof.
    Shell,
    /// The roof fallen in, the walls still up.
    Roofless,
    /// Walls broken along a ragged line, upper floors fallen inside them.
    Broken,
    /// Stumps of wall around a mound of its own rubble.
    Fallen,
}

impl Decay {
    /// Share of a wall holed through.
    fn holes(self) -> f64 {
        match self {
            Decay::Shell => 0.04,
            Decay::Roofless => 0.07,
            Decay::Broken | Decay::Fallen => 0.1,
        }
    }

    /// Share of an upper floor fallen through.
    fn floor_loss(self) -> f64 {
        match self {
            Decay::Shell => 0.12,
            Decay::Roofless => 0.3,
            Decay::Broken | Decay::Fallen => 0.55,
        }
    }

    /// Share of the street beside it under its rubble.
    fn rubble_share(self) -> f64 {
        match self {
            Decay::Shell => 0.04,
            Decay::Roofless => 0.16,
            Decay::Broken => 0.42,
            Decay::Fallen => 0.6,
        }
    }
}

/// What the ruins do to one building column.
#[derive(Clone, Copy, Debug)]
struct RuinPlan {
    decay: Decay,
    /// A wall, with open air beside it, rather than the inside of the building.
    wall: bool,
    /// Where the walls nearby end and any pitched roof begins.
    eave: i32,
    /// The column's top before the ruins.
    top: i32,
}

/// How far up or down the next column over a corner walk looks, so it follows a slanted
/// edge to the end.
const CORNER_SLIDE: [i32; 17] = [0, -1, 1, -2, 2, -3, 3, -4, 4, -5, 5, -6, 6, -7, 7, -8, 8];

/// One step of the walk to a building's corner: over to lower x by the nearest cell of the
/// next column, or else down to lower z. None at the corner. The walk slides only along the
/// building's own column, so it never jumps a street into the next one.
fn corner_step(footprints: &CoordinateBitmap, x: i32, z: i32) -> Option<(i32, i32)> {
    let inside = |dz: i32| {
        let (lo, hi) = (z.min(z + dz), z.max(z + dz));
        (lo..=hi).all(|t| footprints.contains(x, t))
    };
    CORNER_SLIDE
        .iter()
        .find(|&&dz| footprints.contains(x - 1, z + dz) && inside(dz))
        .map(|&dz| (x - 1, z + dz))
        .or_else(|| footprints.contains(x, z - 1).then_some((x, z - 1)))
}

/// The building's far corner, reached from (x, z) by stepping to lower x, or lower z where
/// x can go no lower. It names the building for every column of it, square to the grid or
/// not, so long as it has no notch the walk can end in.
fn building_corner(footprints: &CoordinateBitmap, x: i32, z: i32) -> (i32, i32) {
    let mut at = (x, z);
    for _ in 0..CORNER_WALK {
        match corner_step(footprints, at.0, at.1) {
            Some(next) => at = next,
            None => break,
        }
    }
    at
}

/// The heights of a region's building columns before the ruins, so a building's walls and
/// eaves read alike from each of its chunks.
struct RegionTops {
    min: (i32, i32),
    tops: Vec<i16>,
}

const NO_TOP: i16 = i16::MIN;
/// How far inside a building its eaves are looked for.
const EAVE_REACH: i32 = 48;

impl RegionTops {
    fn build<'c>(
        min: (i32, i32),
        chunks: impl Iterator<Item = ((i32, i32), &'c ChunkToModify)>,
        footprints: &CoordinateBitmap,
    ) -> Self {
        let mut tops = vec![NO_TOP; 512 * 512];
        for ((cx, cz), chunk) in chunks {
            let sections = sections_down(chunk);
            for lz in 0..16u8 {
                for lx in 0..16u8 {
                    let (x, z) = (
                        min.0 + cx * 16 + i32::from(lx),
                        min.1 + cz * 16 + i32::from(lz),
                    );
                    if !footprints.contains(x, z) {
                        continue;
                    }
                    if let Some((y, _)) = (Column { lx, lz, x, z }).top_in(chunk, &sections) {
                        let at = ((z - min.1) * 512 + (x - min.0)) as usize;
                        if let Some(slot) = tops.get_mut(at) {
                            *slot = y.clamp(i32::from(NO_TOP) + 1, i32::from(i16::MAX)) as i16;
                        }
                    }
                }
            }
        }
        Self { min, tops }
    }

    fn get(&self, x: i32, z: i32) -> Option<i32> {
        let (dx, dz) = (x - self.min.0, z - self.min.1);
        if !(0..512).contains(&dx) || !(0..512).contains(&dz) {
            return None;
        }
        let top = self.tops[(dz * 512 + dx) as usize];
        (top != NO_TOP).then_some(i32::from(top))
    }
}

/// Mostly the building's own, with a slow drift for terraces, whose footprints join up.
fn decay_of(corner: (i32, i32), x: i32, z: i32) -> Decay {
    let own = (coord_hash(corner.0, corner.1) >> 16) as f64 % 1000.0 / 1000.0;
    let v = 0.75 * own + 0.25 * patch_noise(x, z, 40, SALT_DECAY);
    match v {
        v if v < 0.17 => Decay::Fallen,
        v if v < 0.39 => Decay::Broken,
        v if v < 0.72 => Decay::Roofless,
        _ => Decay::Shell,
    }
}

/// What the ruins do to each building column of a chunk, indexed by z then x. Read from
/// the heights before any block moves, so no column's plan depends on the order they are
/// broken in.
fn plan_ruins(
    columns: &[(Column, i32)],
    footprints: &CoordinateBitmap,
    tops: &RegionTops,
) -> [[Option<RuinPlan>; 16]; 16] {
    let mut plans = [[None; 16]; 16];
    let Some(&(first, _)) = columns.first() else {
        return plans;
    };
    let base = (first.x - i32::from(first.lx), first.z - i32::from(first.lz));
    let is_wall = |x: i32, z: i32| {
        footprints.contains(x, z)
            && [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|&(dx, dz)| !footprints.contains(x + dx, z + dz))
    };
    // Corners met on the way, so the chunk's columns share their walks.
    let mut corners: [[Option<(i32, i32)>; 16]; 16] = [[None; 16]; 16];
    let mut corner_of = |x: i32, z: i32| {
        let mut path = Vec::new();
        let mut at = (x, z);
        let corner = loop {
            let (lx, lz) = (at.0 - base.0, at.1 - base.1);
            if !(0..16).contains(&lx) || !(0..16).contains(&lz) {
                break building_corner(footprints, at.0, at.1);
            }
            if let Some(corner) = corners[lz as usize][lx as usize] {
                break corner;
            }
            path.push((lx as usize, lz as usize));
            match corner_step(footprints, at.0, at.1) {
                Some(next) => at = next,
                None => break at,
            }
        };
        for (lx, lz) in path {
            corners[lz][lx] = Some(corner);
        }
        corner
    };
    for &(col, _) in columns {
        let (x, z) = (col.x, col.z);
        if !footprints.contains(x, z) {
            continue;
        }
        let Some(top) = tops.get(x, z) else {
            continue;
        };
        let wall = is_wall(x, z);
        // The lowest wall top in sight along the axes, inside this building.
        let mut eave = top;
        if !wall {
            for (dx, dz) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                for i in 1..=EAVE_REACH {
                    let (wx, wz) = (x + dx * i, z + dz * i);
                    if !footprints.contains(wx, wz) {
                        break;
                    }
                    if is_wall(wx, wz) {
                        if let Some(y) = tops.get(wx, wz) {
                            eave = eave.min(y);
                        }
                        break;
                    }
                }
            }
        }
        plans[usize::from(col.lz)][usize::from(col.lx)] = Some(RuinPlan {
            decay: decay_of(corner_of(x, z), x, z),
            wall,
            eave,
            top,
        });
    }
    plans
}

/// How many blocks of a column of `height` stay up.
fn standing_height(col: Column, height: i32, ground_y: i32, plan: RuinPlan) -> i32 {
    let tower = height >= TOWER_HEIGHT;
    let line = patch_noise(col.x, col.z, 11, SALT_BREAK) - 0.5;
    let jag = (patch_noise(col.x, col.z, 5, SALT_JAG) - 0.5) * 4.0;
    let of = |h: i32, share: f64| {
        let share = if tower { share.max(TOWER_SHARE) } else { share };
        f64::from(h) * share
    };
    let standing = if plan.wall {
        match plan.decay {
            Decay::Shell => f64::from(height),
            Decay::Roofless => of(height, 0.96 + line * 0.08) + jag,
            Decay::Broken => of(height, 0.62 + line * 0.5) + jag,
            Decay::Fallen => of(height, 0.22 + line * 0.3) + jag,
        }
    } else {
        // A pitched roof stands above the eaves, so that is as high as the floors go.
        let eave = (plan.eave - ground_y).min(height);
        let floors = patch_noise(col.x, col.z, 9, SALT_FLOORS);
        match plan.decay {
            Decay::Shell if patch_noise(col.x, col.z, 7, SALT_ROOF_HOLE) < 0.22 => {
                f64::from(height - 1 - (col.hash(0) % 3) as i32)
            }
            Decay::Shell => f64::from(height),
            Decay::Roofless => f64::from(eave - 1) - (col.hash(1) % 2) as f64,
            Decay::Broken => of(eave, 0.2 + floors * 0.4),
            Decay::Fallen => of(eave, 0.0),
        }
    };
    (standing.round() as i32).clamp(1, height)
}

/// Breaks a building column: walls stand to a line that fits how far gone the building is,
/// floors and roof fall inside them, and what fell lies where it landed.
fn ruin_building(
    chunk: &mut ChunkToModify,
    col: Column,
    ground_y: i32,
    plan: RuinPlan,
    fires: &mut Vec<FireSpot>,
) {
    let Some((top, _)) = col.top(chunk) else {
        return;
    };
    let height = top - ground_y;
    if height < 2 {
        return;
    }
    let cut = ground_y + standing_height(col, height, ground_y, plan);
    let material = (cut + 1..=top)
        .chain((ground_y + 1..=cut).rev())
        .find_map(|y| col.get(chunk, y).filter(|&b| kind(b) == Kind::Solid));
    let mut fallen = top - cut;
    for y in cut + 1..=top {
        col.set(chunk, y, AIR);
    }

    for y in ground_y + 1..=cut {
        let Some(block) = col.get(chunk, y) else {
            continue;
        };
        let (k, h) = (kind(block), col.hash(y));
        let roll = h % 100;
        // Holes run a few blocks across and up, on a plane skewed through the walls.
        let hole = patch_noise(col.x + y * 3, col.z - y * 2, 4, SALT_HOLE) < plan.decay.holes();
        // Floors fall through in patches, each floor in its own.
        let floor_gone = !plan.wall
            && y > ground_y + 1
            && matches!(k, Kind::Solid | Kind::Shaped)
            && col.get(chunk, y - 1).is_none()
            && patch_noise(col.x, col.z, 6, SALT_FLOORS.wrapping_add(y as u32))
                < plan.decay.floor_loss();
        if floor_gone {
            fallen += 1;
            col.set(chunk, y, AIR);
            // What stood on it comes down too.
            if y < cut
                && col
                    .get(chunk, y + 1)
                    .is_some_and(|b| kind(b) != Kind::Solid)
            {
                col.set(chunk, y + 1, AIR);
            }
        } else if hole || k == Kind::Light || (k == Kind::Glass && (4..85).contains(&roll)) {
            col.set(chunk, y, AIR);
        } else if k == Kind::Glass && roll < 4 {
            col.set(chunk, y, COBWEB);
        } else if k == Kind::Solid && moss_creeps(col, y, y - ground_y) {
            col.set(chunk, y, MOSS_BLOCK);
        } else if let Some(variant) = weathered(block, h) {
            col.swap(chunk, y, variant);
        }
    }

    let open_top = fallen > 0;
    if plan.wall {
        // The broken top of a wall carries a little of what fell, and moss in time.
        if open_top
            && cut < world_max_y()
            && col.get(chunk, cut).is_some_and(|b| kind(b) == Kind::Solid)
        {
            match col.hash(cut + 1) % 6 {
                0 => col.set(chunk, cut + 1, rubble(col.hash(cut + 2))),
                1 | 2 => col.set(chunk, cut + 1, MOSS_CARPET),
                _ => {}
            }
        }
        return;
    }

    let h = col.hash(cut + 3);
    let lying = patch_noise(col.x, col.z, 7, SALT_RUBBLE);
    let pile = match plan.decay {
        Decay::Shell => 0,
        Decay::Roofless => i32::from(lying < 0.25),
        Decay::Broken if fallen >= 3 => (lying * 3.5) as i32,
        Decay::Broken => 0,
        Decay::Fallen => 1 + (lying * 4.0) as i32,
    };
    let floor = (ground_y + 1..=world_max_y())
        .find(|&y| col.get(chunk, y).is_none())
        .unwrap_or(world_max_y());
    if pile > 0 {
        pile_up(chunk, col, floor, pile, material, h, fires);
    }
    if plan.decay == Decay::Shell {
        if open_top && col.hash(cut + 1).is_multiple_of(4) {
            overgrow(chunk, col, cut, h);
        }
        return;
    }
    // Rain and light reach what lies under the open sky, so things grow on it.
    let Some((surface, _)) = col.top(chunk) else {
        return;
    };
    if patch_noise(col.x, col.z, 8, SALT_GROWTH) < 0.6 {
        overgrow(chunk, col, surface, h);
    }
    if plan.decay != Decay::Roofless && (h >> 20).is_multiple_of(RUIN_TREE_ODDS) {
        grow_tree(chunk, col, surface, h);
    }
}

/// A heap `height` blocks tall from `y`, of what the building was made of and common rubble,
/// its top broken into slabs here and there. It stops at anything in the way.
fn pile_up(
    chunk: &mut ChunkToModify,
    col: Column,
    y: i32,
    height: i32,
    material: Option<Block>,
    h: u64,
    fires: &mut Vec<FireSpot>,
) {
    for i in 0..height {
        let at = y + i;
        if at > world_max_y() || col.get(chunk, at).is_some() {
            return;
        }
        let mix = h.wrapping_add(u64::from(i as u32).wrapping_mul(0x9E37_79B9));
        if i == height - 1 {
            if mix.is_multiple_of(3) {
                col.set(chunk, at, COBBLESTONE_SLAB);
            } else {
                rubble_or_fire(chunk, col, at, mix, fires);
            }
        } else {
            col.set(chunk, at, debris(material, mix));
        }
    }
}

/// A block of a heap: as often as not what the building was made of.
fn debris(material: Option<Block>, h: u64) -> Block {
    match material {
        Some(block) if h % 5 < 2 => block,
        _ => rubble(h >> 3),
    }
}

/// Moss on the block at `y` and something growing on it.
fn overgrow(chunk: &mut ChunkToModify, col: Column, y: i32, h: u64) {
    if y >= world_max_y() {
        return;
    }
    if col.get(chunk, y).is_some_and(|b| kind(b) == Kind::Solid) {
        col.set(chunk, y, MOSS_BLOCK);
        grow(chunk, col, y + 1, h >> 8, 55);
    } else if col.get(chunk, y).is_some_and(|b| kind(b) == Kind::Shaped) && h.is_multiple_of(2) {
        col.set(chunk, y + 1, MOSS_CARPET);
    }
}

/// A small tree come up in the ruin, if it fits the chunk and the space over it is free.
fn grow_tree(chunk: &mut ChunkToModify, col: Column, surface: i32, h: u64) {
    if !(2..=13).contains(&col.lx) || !(2..=13).contains(&col.lz) {
        return;
    }
    let trunk = 3 + ((h >> 4) % 3) as i32;
    let crown = surface + trunk;
    if crown + 2 > world_max_y() {
        return;
    }
    let around = |dx: i32, dz: i32| Column {
        lx: (i32::from(col.lx) + dx) as u8,
        lz: (i32::from(col.lz) + dz) as u8,
        x: col.x + dx,
        z: col.z + dz,
    };
    // A wall in the crown's way means there is no room for it here.
    for dz in -2..=2 {
        for dx in -2..=2 {
            if (surface + 1..=crown + 1).any(|y| around(dx, dz).get(chunk, y).is_some()) {
                return;
            }
        }
    }
    for y in surface + 1..=crown {
        col.set(chunk, y, OAK_LOG);
    }
    for dz in -2i32..=2 {
        for dx in -2i32..=2 {
            let reach = dx.abs() + dz.abs();
            for (y, most) in [(crown - 1, 3), (crown, 3), (crown + 1, 1)] {
                if reach <= most && !(dx == 0 && dz == 0 && y <= crown) {
                    around(dx, dz).set(chunk, y, OAK_LEAVES);
                }
            }
        }
    }
}

/// Moss climbs a few blocks up a ruin from the ground, in patches that thin out with height.
fn moss_creeps(col: Column, y: i32, above_ground: i32) -> bool {
    if above_ground > MOSS_REACH {
        return false;
    }
    let fade = 1.0 - f64::from(above_ground - 1) / f64::from(MOSS_REACH);
    patch_noise(col.x + y, col.z - y, 5, SALT_MOSS) < 0.22 * fade
}

/// A rubble pile kept for a fire, lit once the rest of the chunk is done.
#[derive(Clone, Copy)]
struct FireSpot {
    col: Column,
    y: i32,
    h: u64,
}

/// A rubble block at `y`, or now and then a spot kept for a fire.
fn rubble_or_fire(
    chunk: &mut ChunkToModify,
    col: Column,
    y: i32,
    h: u64,
    fires: &mut Vec<FireSpot>,
) {
    if (h >> 24).is_multiple_of(FIRE_ODDS) {
        fires.push(FireSpot { col, y, h });
    } else {
        col.set(chunk, y, rubble(h));
    }
}

/// A fire burning on netherrack where nothing it could spread to stands within its reach,
/// and the rubble it would have been where something does.
fn light_fire(chunk: &mut ChunkToModify, FireSpot { col, y, h }: FireSpot) {
    if fire_fits(chunk, col, y) {
        col.set(chunk, y, NETHERRACK);
        col.set(chunk, y + 1, FIRE);
    } else if col.get(chunk, y).is_none() {
        col.set(chunk, y, rubble(h));
    }
}

/// Whether netherrack at `y` and a fire above it have room, with nothing flammable in the
/// box a fire spreads through: a block around it, one below and four above. That box must
/// lie inside the chunk, the only blocks this pass can read.
fn fire_fits(chunk: &ChunkToModify, col: Column, y: i32) -> bool {
    if y + 1 > world_max_y() || !(1..15).contains(&col.lx) || !(1..15).contains(&col.lz) {
        return false;
    }
    if col.get(chunk, y).is_some() || col.get(chunk, y + 1).is_some() {
        return false;
    }
    (-1..=1).all(|dz: i32| {
        (-1..=1).all(|dx: i32| {
            let (lx, lz) = (
                (i32::from(col.lx) + dx) as u8,
                (i32::from(col.lz) + dz) as u8,
            );
            (y..=y + 5).all(|yy| !chunk.get_block(lx, yy, lz).is_some_and(flammable))
        })
    })
}

/// Vines climb the walls facing the street, in patches up to a few storeys high. Only
/// walls inside this chunk are read, so none hangs beside a wall that came down.
fn hang_vines(
    chunk: &mut ChunkToModify,
    col: Column,
    ground_y: i32,
    footprints: &CoordinateBitmap,
) {
    let patch = patch_noise(col.x, col.z, 9, SALT_VINE);
    if patch > VINE_SHARE {
        return;
    }
    let height = 2 + ((VINE_SHARE - patch) / VINE_SHARE * VINE_REACH) as i32;
    // The wall to the north, east, south and west, in the order of `VINES`.
    for (vine, (dx, dz)) in VINES.into_iter().zip([(0, -1), (1, 0), (0, 1), (-1, 0)]) {
        let (wx, wz) = (i32::from(col.lx) + dx, i32::from(col.lz) + dz);
        if !(0..16).contains(&wx)
            || !(0..16).contains(&wz)
            || !footprints.contains(col.x + dx, col.z + dz)
        {
            continue;
        }
        let wall = Column {
            lx: wx as u8,
            lz: wz as u8,
            x: col.x + dx,
            z: col.z + dz,
        };
        // A vine needs a face to cling to, so window holes leave gaps in it.
        let wall_top = wall.top(chunk).map_or(ground_y, |(y, _)| y);
        for y in ground_y + 1..=(ground_y + height).min(wall_top) {
            if wall.get(chunk, y).is_some_and(|b| kind(b) == Kind::Solid)
                && col.get(chunk, y).is_none()
            {
                col.set(chunk, y, vine);
            }
        }
        return;
    }
}

/// Streets and yards: cracked paving with grass coming through, rubble along the ruins,
/// and lamps gone dark.
fn ruin_open_ground(
    chunk: &mut ChunkToModify,
    col: Column,
    ground_y: i32,
    terrain: &Terrain,
    style: &WorldStyle,
    fires: &mut Vec<FireSpot>,
) {
    let Some((top, block)) = col.top(chunk) else {
        return;
    };
    for y in (ground_y + 1..=top.min(ground_y + LAMP_REACH)).rev() {
        if col.get(chunk, y).is_some_and(|b| kind(b) == Kind::Light) {
            col.set(chunk, y, AIR);
        }
    }
    if top != ground_y || kind(block) != Kind::Solid || top + 1 > world_max_y() {
        return;
    }
    let h = col.hash(top);

    if let Some(footprints) = style.footprints {
        if let Some((d, wx, wz)) = nearest_footprint(footprints, col.x, col.z) {
            let decay = decay_of(building_corner(footprints, wx, wz), wx, wz);
            let height = heap_height(col, d, decay);
            if height > 0 {
                let material = wall_material(chunk, col, wx, wz, ground_y);
                pile_up(chunk, col, top + 1, height, material, h >> 8, fires);
                return;
            }
        }
    }

    if terrain.sealed(col.x, col.z) {
        // A crack network: broken patches the roots have opened up.
        if patch_noise(col.x, col.z, 7, SALT_CRACK) < 0.32 {
            let ground = match h % 10 {
                0..=3 => GRASS_BLOCK,
                4 | 5 => COARSE_DIRT,
                6 | 7 => GRAVEL,
                8 => MOSS_BLOCK,
                _ => COBBLESTONE,
            };
            col.set(chunk, top, ground);
            if matches!(ground, GRASS_BLOCK | MOSS_BLOCK) {
                if (h >> 16).is_multiple_of(14) {
                    col.set(chunk, top + 1, street_bush(h));
                } else {
                    grow(chunk, col, top + 1, h >> 8, 70);
                }
            }
        }
    } else if block == GRASS_BLOCK {
        // Lawns gone wild.
        grow(chunk, col, top + 1, h >> 8, 30);
    }
}

/// A bush come up through the paving.
fn street_bush(h: u64) -> Block {
    if (h >> 24).is_multiple_of(3) {
        BIRCH_LEAVES
    } else {
        OAK_LEAVES
    }
}

/// How far the nearest footprint cell is, up to `RUBBLE_REACH`, and where it is.
fn nearest_footprint(footprints: &CoordinateBitmap, x: i32, z: i32) -> Option<(i32, i32, i32)> {
    (1..=RUBBLE_REACH).find_map(|d| {
        (-d..=d).find_map(|dz| {
            (-d..=d)
                .filter(|dx| dx.abs() == d || dz.abs() == d)
                .find(|dx| footprints.contains(x + dx, z + dz))
                .map(|dx| (d, x + dx, z + dz))
        })
    })
}

/// How tall a heap lies `d` blocks out from a building, 0 for none: in patches, highest
/// against the wall, and with more of them the further gone the building is.
fn heap_height(col: Column, d: i32, decay: Decay) -> i32 {
    let share = decay.rubble_share();
    let patch = patch_noise(col.x, col.z, 6, SALT_RUBBLE);
    if patch >= share {
        return 0;
    }
    let most = RUBBLE_REACH + 1 - d;
    (1 + ((share - patch) / share * f64::from(most)) as i32).min(most)
}

/// What a wall at (wx, wz) is made of, if it is in this chunk.
fn wall_material(
    chunk: &ChunkToModify,
    col: Column,
    wx: i32,
    wz: i32,
    ground_y: i32,
) -> Option<Block> {
    let wall = in_chunk(col, wx - col.x, wz - col.z)?;
    (ground_y + 1..=ground_y + 4)
        .find_map(|y| wall.get(chunk, y).filter(|&b| kind(b) == Kind::Solid))
}

/// The column `dx`, `dz` from `col`, if the chunk holds it.
fn in_chunk(col: Column, dx: i32, dz: i32) -> Option<Column> {
    let (lx, lz) = (i32::from(col.lx) + dx, i32::from(col.lz) + dz);
    ((0..16).contains(&lx) && (0..16).contains(&lz)).then_some(Column {
        lx: lx as u8,
        lz: lz as u8,
        x: col.x + dx,
        z: col.z + dz,
    })
}

/// Eaves stick out past a building's footprint: where the wall they hung from has come
/// down, they come down too. Only a thin overhang with nothing under it goes, so trees,
/// posts and decks beside a ruin stay. Only walls inside this chunk are read.
fn drop_overhangs(
    chunk: &mut ChunkToModify,
    col: Column,
    ground_y: i32,
    plans: &[[Option<RuinPlan>; 16]; 16],
) {
    let Some((top, _)) = col.top(chunk) else {
        return;
    };
    let solid = |y: i32| {
        col.get(chunk, y)
            .is_some_and(|b| matches!(kind(b), Kind::Solid | Kind::Shaped))
    };
    if top <= ground_y + 2 || !solid(top) {
        return;
    }
    let mut bottom = top;
    while bottom > top - 1 && solid(bottom - 1) {
        bottom -= 1;
    }
    if col.get(chunk, bottom - 1).is_some() || bottom <= ground_y + 2 {
        return;
    }
    let hung_from_a_fallen_wall = [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .filter_map(|(dx, dz)| in_chunk(col, dx, dz))
        .any(|wall| {
            plans[usize::from(wall.lz)][usize::from(wall.lx)].is_some_and(|plan| {
                let now = wall.top(chunk).map_or(ground_y, |(y, _)| y);
                top <= plan.top + 1 && bottom > now
            })
        });
    if hung_from_a_fallen_wall {
        for y in bottom..=top {
            col.set(chunk, y, AIR);
        }
    }
}

/// Now and then a car left to rust where it stopped, on a clear stretch of road.
fn wreck_a_car(
    chunk: &mut ChunkToModify,
    base: (i32, i32),
    columns: &[(Column, i32)],
    terrain: &Terrain,
    footprints: &CoordinateBitmap,
) {
    let h = coord_hash(base.0, base.1);
    if !h.is_multiple_of(WRECK_ODDS) {
        return;
    }
    let (lx, lz) = (4 + ((h >> 8) % 8) as i32, 4 + ((h >> 16) % 8) as i32);
    let Some(&(centre, ground_y)) = columns
        .iter()
        .find(|(c, _)| i32::from(c.lx) == lx && i32::from(c.lz) == lz)
    else {
        return;
    };
    // Along the road: the way its surface runs further.
    let run = |dx: i32, dz: i32| {
        (1..=8)
            .take_while(|&i| terrain.sealed(centre.x + dx * i, centre.z + dz * i))
            .count()
    };
    let along_x = run(1, 0) + run(-1, 0) >= run(0, 1) + run(0, -1);
    let (ax, az) = if along_x { (1, 0) } else { (0, 1) };
    let cell = |along: i32, across: i32| {
        in_chunk(centre, along * ax + across * az, along * az + across * ax)
    };
    let body: Vec<(Column, i32)> = (-2..=1)
        .flat_map(|along| (0..=1).map(move |across| (along, across)))
        .filter_map(|(along, across)| cell(along, across).map(|c| (c, along)))
        .collect();
    let clear = body.len() == 8
        && body.iter().all(|&(c, _)| {
            terrain.sealed(c.x, c.z)
                && !footprints.contains(c.x, c.z)
                && terrain.ground_y(c.x, c.z) == ground_y
                && c.top(chunk)
                    .is_some_and(|(y, b)| y == ground_y && kind(b) == Kind::Solid)
        });
    if !clear || ground_y + 3 > world_max_y() {
        return;
    }
    let paint = [
        BROWN_TERRACOTTA,
        GRAY_TERRACOTTA,
        RED_TERRACOTTA,
        BLACK_CONCRETE,
    ][(h >> 24) as usize % 4];
    for &(c, along) in &body {
        c.set(chunk, ground_y + 1, paint);
        if along == -1 || along == 0 {
            c.set(chunk, ground_y + 2, TINTED_GLASS);
        } else if (h >> 28).is_multiple_of(2) {
            c.set(chunk, ground_y + 2, MOSS_CARPET);
        }
    }
}

/// Grass, tall grass or a dead bush at `y`, in `percent` of the cases it is asked.
fn grow(chunk: &mut ChunkToModify, col: Column, y: i32, h: u64, percent: u64) {
    if h % 100 >= percent || col.get(chunk, y).is_some() {
        return;
    }
    match (h >> 7) % 10 {
        0..=5 => col.set(chunk, y, GRASS),
        6..=8 if y < world_max_y() => {
            col.set(chunk, y, TALL_GRASS_BOTTOM);
            col.set(chunk, y + 1, TALL_GRASS_TOP);
        }
        _ => col.set(chunk, y, DEAD_BUSH),
    }
}

/// The cracked or mossy variant some of a material's blocks weather into.
fn weathered(block: Block, h: u64) -> Option<Block> {
    let roll = h % 100;
    let (variant, share) = match block {
        STONE_BRICKS if roll < 30 => (CRACKED_STONE_BRICKS, 30),
        STONE_BRICKS => (MOSSY_STONE_BRICKS, 55),
        STONE_BRICK_STAIRS => (MOSSY_STONE_BRICK_STAIRS, 35),
        STONE_BRICK_SLAB => (MOSSY_STONE_BRICK_SLAB, 35),
        STONE_BRICK_WALL => (MOSSY_STONE_BRICK_WALL, 35),
        COBBLESTONE => (MOSSY_COBBLESTONE, 40),
        COBBLESTONE_STAIRS => (MOSSY_COBBLESTONE_STAIRS, 35),
        COBBLESTONE_SLAB => (MOSSY_COBBLESTONE_SLAB, 35),
        COBBLESTONE_WALL => (MOSSY_COBBLESTONE_WALL, 35),
        DEEPSLATE_BRICKS => (CRACKED_DEEPSLATE_BRICKS, 40),
        DEEPSLATE_TILES => (CRACKED_DEEPSLATE_TILES, 40),
        POLISHED_BLACKSTONE_BRICKS => (CRACKED_POLISHED_BLACKSTONE_BRICKS, 40),
        NETHER_BRICK => (CRACKED_NETHER_BRICKS, 40),
        // Smooth stone facing breaks back down to rubble stone.
        STONE | SMOOTH_STONE | POLISHED_ANDESITE if roll < 12 => (COBBLESTONE, 12),
        STONE | SMOOTH_STONE | POLISHED_ANDESITE => (MOSSY_COBBLESTONE, 18),
        // Pale render and stone goes dirty and rough.
        WHITE_CONCRETE | SMOOTH_QUARTZ | QUARTZ_BLOCK if roll < 18 => (CALCITE, 18),
        WHITE_CONCRETE | SMOOTH_QUARTZ | QUARTZ_BLOCK => (DIORITE, 30),
        WHITE_TERRACOTTA => (CALCITE, 22),
        // Concrete spalls down to its aggregate.
        LIGHT_GRAY_CONCRETE if roll < 18 => (ANDESITE, 18),
        LIGHT_GRAY_CONCRETE => (TUFF, 28),
        GRAY_CONCRETE if roll < 16 => (TUFF, 16),
        GRAY_CONCRETE => (COBBLESTONE, 24),
        BRICK if roll < 14 => (GRANITE, 14),
        BRICK => (COBBLESTONE, 22),
        SMOOTH_SANDSTONE => (SANDSTONE, 40),
        OAK_PLANKS => (SPRUCE_PLANKS, 25),
        TERRACOTTA => (COARSE_DIRT, 12),
        _ => return None,
    };
    (roll < share).then_some(variant)
}

fn rubble(h: u64) -> Block {
    match h % 6 {
        0 | 1 => COBBLESTONE,
        2 => MOSSY_COBBLESTONE,
        3 => GRAVEL,
        4 => ANDESITE,
        _ => COBBLESTONE_SLAB,
    }
}

/// Lays snow over the column's top, buries its small plants, and freezes open water.
fn snow_over(
    chunk: &mut ChunkToModify,
    col: Column,
    ground_y: i32,
    terrain: &Terrain,
    bare: Option<Block>,
) {
    let Some((top, block)) = col.top(chunk) else {
        return;
    };
    let on_ground = top <= ground_y + 1;
    match kind(block) {
        Kind::Water => {
            if terrain.freezes(col.x, col.z) {
                col.set(chunk, top, ICE);
            }
        }
        Kind::Plant => {
            col.set(chunk, top, snow_layer(col, on_ground, false));
            snowy_ground(chunk, col, top - 1);
        }
        Kind::TallPlant => {
            let lower = col.get(chunk, top - 1);
            let at = if lower == Some(block) || lower.is_some_and(|b| kind(b) == Kind::TallPlant) {
                col.set(chunk, top, AIR);
                top - 1
            } else {
                top
            };
            col.set(chunk, at, snow_layer(col, on_ground, false));
            snowy_ground(chunk, col, at - 1);
        }
        Kind::Leaves | Kind::Solid | Kind::Shaped if top < world_max_y() => {
            if on_ground && bare == Some(block) {
                return;
            }
            // Ploughed roads keep only scattered ridges of snow.
            let road = on_ground && terrain.sealed(col.x, col.z);
            if road && patch_noise(col.x, col.z, 3, SALT_PLOW) < 0.92 {
                return;
            }
            if kind(block) == Kind::Shaped
                && !full_top(col.properties(chunk, top, block).as_deref())
            {
                // Snow slides off a bottom slab or stair, so most such roofs turn white.
                if patch_noise(col.x, col.z, 10, SALT_ROOF) < WHITE_ROOF_SHARE {
                    if let Some(white) = whitened(block) {
                        col.swap(chunk, top, white);
                    }
                }
                return;
            }
            let thin = road || kind(block) == Kind::Leaves;
            col.set(chunk, top + 1, snow_layer(col, on_ground, thin));
            snowy_ground(chunk, col, top);
        }
        _ => {}
    }
}

/// Whether a slab or stair has the full top face snow needs: a top or double slab, or a
/// stair in the upper half. Without a state it is a bottom one.
fn full_top(props: Option<&Value>) -> bool {
    let Some(Value::Compound(map)) = props else {
        return false;
    };
    let is = |key: &str, values: &[&str]| matches!(map.get(key), Some(Value::String(v)) if values.contains(&v.as_str()));
    is("type", &["top", "double"]) || is("half", &["top"])
}

/// The white slab or stair a bottom one turns into under snow.
fn whitened(block: Block) -> Option<Block> {
    let name = block.try_name()?;
    if name.ends_with("_stairs") {
        Some(SMOOTH_QUARTZ_STAIRS)
    } else if name.ends_with("_slab") {
        Some(SMOOTH_QUARTZ_SLAB)
    } else {
        None
    }
}

/// Snow one to four eighths deep on open ground, drifting in patches; roofs, crowns and
/// roads hold a thinner coat.
fn snow_layer(col: Column, on_ground: bool, thin: bool) -> Block {
    let drift = patch_noise(col.x, col.z, 20, SALT_DEPTH);
    let eighths = if thin {
        1
    } else if on_ground {
        1 + (drift * 3.99) as usize
    } else {
        1 + usize::from(drift > 0.6)
    };
    SNOW_LAYERS[eighths - 1]
}

/// Grass and podzol under snow turn to their snowy variants, which the game only does on
/// a block update. Farmland, paths and mud sit lower than a full block, which snow needs,
/// so they turn to dirt.
fn snowy_ground(chunk: &mut ChunkToModify, col: Column, y: i32) {
    let snowy = match col.get(chunk, y) {
        Some(GRASS_BLOCK) => SNOWY_GRASS_BLOCK,
        Some(PODZOL) => SNOWY_PODZOL,
        Some(FARMLAND | DIRT_PATH | MUD) => DIRT,
        _ => return,
    };
    col.set(chunk, y, snowy);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_definitions::{
        GLASS, GLASS_PANE, LANTERN, OAK_LEAVES, OAK_STAIRS, SNOW_BLOCK, STONE,
    };

    #[test]
    fn blocks_sort_into_what_snow_and_ruin_do_with_them() {
        assert_eq!(kind(AIR), Kind::Air);
        assert_eq!(kind(crate::block_definitions::WATER), Kind::Water);
        assert_eq!(kind(SNOW_BLOCK), Kind::Snowy);
        assert_eq!(kind(ICE), Kind::Snowy);
        assert_eq!(kind(GRASS), Kind::Plant);
        assert_eq!(kind(TALL_GRASS_TOP), Kind::TallPlant);
        assert_eq!(kind(OAK_LEAVES), Kind::Leaves);
        assert_eq!(kind(GLASS), Kind::Glass);
        assert_eq!(kind(GLASS_PANE), Kind::Glass);
        assert_eq!(kind(LANTERN), Kind::Light);
        assert_eq!(kind(STONE), Kind::Solid);
        assert_eq!(kind(OAK_STAIRS), Kind::Shaped);
        assert_eq!(kind(crate::block_definitions::COBBLESTONE_WALL), Kind::Thin);
        assert_eq!(
            kind(crate::block_definitions::LIGHT_BLUE_CONCRETE),
            Kind::Solid
        );
    }

    #[test]
    fn snow_is_never_deeper_than_half_a_block() {
        for x in 0..200 {
            let col = Column {
                lx: 0,
                lz: 0,
                x,
                z: x * 7,
            };
            let depth = SNOW_LAYERS
                .iter()
                .position(|&b| b == snow_layer(col, true, false))
                .unwrap();
            assert!(depth < 4);
            assert_eq!(snow_layer(col, true, true), SNOW_LAYERS[0]);
        }
    }

    fn chunk_with(blocks: &[(i32, Block)]) -> ChunkToModify {
        let mut chunk = ChunkToModify::default();
        for &(y, b) in blocks {
            chunk.set_block(3, y, 4, b);
        }
        chunk
    }

    fn terrain_without_ground(overrides: &crate::world_editor::CoordMap<i32>) -> Terrain<'_> {
        Terrain {
            ground: None,
            origin: (0, 0),
            overrides,
            sealed: None,
        }
    }

    #[test]
    fn snow_buries_grass_and_greys_nothing_it_cannot_sit_on() {
        let overrides = Default::default();
        let terrain = terrain_without_ground(&overrides);
        let col = Column {
            lx: 3,
            lz: 4,
            x: 3,
            z: 4,
        };

        let mut chunk = chunk_with(&[(0, GRASS_BLOCK), (1, GRASS)]);
        snow_over(&mut chunk, col, 0, &terrain, None);
        assert_eq!(col.get(&chunk, 0), Some(SNOWY_GRASS_BLOCK));
        assert!(col.get(&chunk, 1).is_some_and(|b| kind(b) == Kind::Snowy));

        let mut chunk = chunk_with(&[
            (0, GRASS_BLOCK),
            (1, TALL_GRASS_BOTTOM),
            (2, TALL_GRASS_TOP),
        ]);
        snow_over(&mut chunk, col, 0, &terrain, None);
        assert_eq!(col.get(&chunk, 2), None);
        assert!(col.get(&chunk, 1).is_some_and(|b| kind(b) == Kind::Snowy));

        let mut chunk = chunk_with(&[(0, STONE), (1, GLASS_PANE)]);
        snow_over(&mut chunk, col, 0, &terrain, None);
        assert_eq!(col.get(&chunk, 2), None);

        let mut chunk = chunk_with(&[(0, crate::block_definitions::WATER)]);
        snow_over(&mut chunk, col, 0, &terrain, None);
        assert_eq!(col.get(&chunk, 0), Some(ICE));
    }

    #[test]
    fn a_ruin_keeps_its_ground_floor_and_loses_its_glass() {
        let col = Column {
            lx: 3,
            lz: 4,
            x: 3,
            z: 4,
        };
        let mut blocks: Vec<(i32, Block)> = vec![(0, STONE)];
        for y in 1..=30 {
            blocks.push((
                y,
                if y.rem_euclid(3) == 0 {
                    GLASS
                } else {
                    STONE_BRICKS
                },
            ));
        }
        for decay in [Decay::Shell, Decay::Roofless, Decay::Broken, Decay::Fallen] {
            for wall in [true, false] {
                let mut chunk = chunk_with(&blocks);
                let plan = RuinPlan {
                    decay,
                    wall,
                    eave: 30,
                    top: 30,
                };
                ruin_building(&mut chunk, col, 0, plan, &mut Vec::new());
                assert_eq!(col.get(&chunk, 0), Some(STONE));
                assert!(
                    (1..=30).all(|y| col.get(&chunk, y) != Some(GLASS) || col.hash(y) % 100 >= 85)
                );
                let top = col.top(&chunk).unwrap().0;
                assert!((1..=33).contains(&top), "{decay:?} {wall} {top}");
            }
        }
    }

    /// Stands a block of `size` by `size` at the origin of a chunk and maps its footprint.
    fn square_building(
        size: i32,
        height: i32,
    ) -> (ChunkToModify, Vec<(Column, i32)>, CoordinateBitmap) {
        let bbox = crate::coordinate_system::cartesian::XZBBox::rect_from_min_max(-64, -64, 64, 64)
            .unwrap();
        let mut footprints = CoordinateBitmap::new(&bbox);
        let mut chunk = ChunkToModify::default();
        for x in 2..2 + size {
            for z in 2..2 + size {
                footprints.set(x, z);
                let wall = x == 2 || z == 2 || x == 1 + size || z == 1 + size;
                for y in 0..=height {
                    if wall || y == height || y == 0 {
                        chunk.set_block(x as u8, y, z as u8, STONE_BRICKS);
                    }
                }
            }
        }
        let columns = (0..16u8)
            .flat_map(|lz| (0..16u8).map(move |lx| (lx, lz)))
            .map(|(lx, lz)| {
                (
                    Column {
                        lx,
                        lz,
                        x: i32::from(lx),
                        z: i32::from(lz),
                    },
                    0,
                )
            })
            .collect();
        (chunk, columns, footprints)
    }

    fn tops_of(chunk: &ChunkToModify, footprints: &CoordinateBitmap) -> RegionTops {
        RegionTops::build((0, 0), std::iter::once(((0, 0), chunk)), footprints)
    }

    #[test]
    fn a_building_askew_to_the_grid_has_one_corner() {
        let bbox = crate::coordinate_system::cartesian::XZBBox::rect_from_min_max(-64, -64, 64, 64)
            .unwrap();
        let mut footprints = CoordinateBitmap::new(&bbox);
        // A square turned 45 degrees: every cell on its lower left edge is a corner of its own
        // to a walk that only steps straight.
        let cells: Vec<(i32, i32)> = (-12i32..=12)
            .flat_map(|x| (-12i32..=12).map(move |z| (x, z)))
            .filter(|(x, z)| x.abs() + z.abs() <= 12)
            .collect();
        for &(x, z) in &cells {
            footprints.set(x, z);
        }
        let corner = building_corner(&footprints, 0, 0);
        for &(x, z) in &cells {
            assert_eq!(building_corner(&footprints, x, z), corner, "from {x},{z}");
        }
    }

    #[test]
    fn a_corner_walk_keeps_to_its_own_side_of_a_street() {
        let bbox = crate::coordinate_system::cartesian::XZBBox::rect_from_min_max(-64, -64, 64, 64)
            .unwrap();
        let mut footprints = CoordinateBitmap::new(&bbox);
        // One building, and another across a five-block street off its corner.
        for x in 10..=20 {
            for z in 10..=20 {
                footprints.set(x, z);
            }
        }
        for x in 0..=9 {
            for z in 0..=4 {
                footprints.set(x, z);
            }
        }
        for x in 10..=20 {
            for z in 10..=20 {
                assert_eq!(building_corner(&footprints, x, z), (10, 10), "from {x},{z}");
            }
        }
        assert_eq!(building_corner(&footprints, 9, 4), (0, 0));
    }

    #[test]
    fn a_building_has_one_corner_and_one_plan_all_across() {
        let (chunk, columns, footprints) = square_building(10, 20);
        let plans = plan_ruins(&columns, &footprints, &tops_of(&chunk, &footprints));
        assert!(plans[0][0].is_none());
        for z in 2..12 {
            for x in 2..12 {
                assert_eq!(building_corner(&footprints, x, z), (2, 2));
                let plan = plans[z as usize][x as usize].unwrap();
                assert_eq!(plan.wall, x == 2 || z == 2 || x == 11 || z == 11);
                assert_eq!(plan.eave, 20);
            }
        }
    }

    #[test]
    fn a_ruin_keeps_its_walls_above_what_is_left_inside() {
        let (mut chunk, columns, footprints) = square_building(10, 20);
        let at = |x: u8, z: u8| {
            columns
                .iter()
                .find(|(c, _)| c.lx == x && c.lz == z)
                .unwrap()
                .0
        };
        let plan = |wall| RuinPlan {
            decay: Decay::Roofless,
            wall,
            eave: 20,
            top: 20,
        };
        let (wall, inside) = (at(2, 6), at(6, 6));
        ruin_building(&mut chunk, wall, 0, plan(true), &mut Vec::new());
        ruin_building(&mut chunk, inside, 0, plan(false), &mut Vec::new());
        let wall_top = wall.top(&chunk).unwrap().0;
        assert!(wall_top >= 17, "the wall came down to {wall_top}");
        // The roof slab over the inside has gone.
        assert_eq!(inside.get(&chunk, 20), None);
        let _ = footprints;
    }

    #[test]
    fn towers_stay_on_the_skyline_however_far_gone() {
        let col = Column {
            lx: 3,
            lz: 4,
            x: 3,
            z: 4,
        };
        let plan = RuinPlan {
            decay: Decay::Fallen,
            wall: true,
            eave: 100,
            top: 100,
        };
        assert!(standing_height(col, 100, 0, plan) >= 55);
        assert!(standing_height(col, 20, 0, plan) <= 10);
    }

    #[test]
    fn a_heap_lies_highest_against_the_wall() {
        let col = Column {
            lx: 3,
            lz: 4,
            x: 3,
            z: 4,
        };
        for d in 1..=RUBBLE_REACH {
            assert!(heap_height(col, d, Decay::Fallen) <= RUBBLE_REACH + 1 - d);
        }
        // A heap stops at what is in the way, and only its top is ever a slab.
        let mut chunk = chunk_with(&[(0, STONE), (3, STONE_BRICKS)]);
        pile_up(&mut chunk, col, 1, 4, None, 7, &mut Vec::new());
        assert!(col.get(&chunk, 1).is_some());
        assert!(col.get(&chunk, 2).is_some());
        assert_eq!(col.get(&chunk, 3), Some(STONE_BRICKS));
        assert_eq!(col.get(&chunk, 4), None);
    }

    #[test]
    fn eaves_come_down_with_the_wall_under_them() {
        let bbox = crate::coordinate_system::cartesian::XZBBox::rect_from_min_max(-64, -64, 64, 64)
            .unwrap();
        let mut footprints = CoordinateBitmap::new(&bbox);
        footprints.set(4, 4);
        let at = |x: u8, z: u8| Column {
            lx: x,
            lz: z,
            x: i32::from(x),
            z: i32::from(z),
        };
        let (wall, eave) = (at(4, 4), at(3, 4));
        let columns: Vec<(Column, i32)> = (0..16u8)
            .flat_map(|lz| (0..16u8).map(move |lx| (at(lx, lz), 0)))
            .collect();
        let standing = |height: i32| {
            let mut chunk = ChunkToModify::default();
            for y in 0..=height {
                wall.set(&mut chunk, y, STONE_BRICKS);
            }
            eave.set(&mut chunk, 0, STONE);
            eave.set(&mut chunk, 12, STONE_BRICK_SLAB);
            chunk
        };
        // The wall stood to 12 and has come down to 6: the slab hanging off it goes.
        let mut chunk = standing(12);
        let plans = plan_ruins(&columns, &footprints, &tops_of(&chunk, &footprints));
        for y in 7..=12 {
            wall.set(&mut chunk, y, AIR);
        }
        drop_overhangs(&mut chunk, eave, 0, &plans);
        assert_eq!(eave.get(&chunk, 12), None);
        assert_eq!(eave.get(&chunk, 0), Some(STONE));

        // Beside a wall still standing that high it stays.
        let mut chunk = standing(12);
        let plans = plan_ruins(&columns, &footprints, &tops_of(&chunk, &footprints));
        drop_overhangs(&mut chunk, eave, 0, &plans);
        assert_eq!(eave.get(&chunk, 12), Some(STONE_BRICK_SLAB));

        // A trunk standing on the ground beside a fallen wall is no overhang.
        let mut chunk = standing(12);
        for y in 1..=12 {
            eave.set(&mut chunk, y, OAK_LOG);
        }
        let plans = plan_ruins(&columns, &footprints, &tops_of(&chunk, &footprints));
        for y in 7..=12 {
            wall.set(&mut chunk, y, AIR);
        }
        drop_overhangs(&mut chunk, eave, 0, &plans);
        assert_eq!(eave.get(&chunk, 12), Some(OAK_LOG));
    }

    #[test]
    fn modern_materials_weather_too_and_stay_full_blocks() {
        for block in [WHITE_CONCRETE, LIGHT_GRAY_CONCRETE, GRAY_CONCRETE, BRICK] {
            let turned: Vec<Block> = (0..100u64).filter_map(|h| weathered(block, h)).collect();
            assert!(!turned.is_empty() && turned.len() < 60, "{turned:?}");
            assert!(turned.iter().all(|&b| kind(b) == Kind::Solid));
        }
    }

    #[test]
    fn fire_burns_only_where_nothing_can_catch() {
        let col = Column {
            lx: 3,
            lz: 4,
            x: 3,
            z: 4,
        };
        let chunk = chunk_with(&[(0, STONE)]);
        assert!(fire_fits(&chunk, col, 1));

        // Planks beside the fire, or a crown four blocks over it, keep it away.
        let mut chunk = chunk_with(&[(0, STONE)]);
        chunk.set_block(4, 2, 4, crate::block_definitions::OAK_PLANKS);
        assert!(!fire_fits(&chunk, col, 1));
        let chunk = chunk_with(&[(0, STONE), (6, OAK_LEAVES)]);
        assert!(!fire_fits(&chunk, col, 1));

        // At the chunk's edge the neighbours cannot be read, so no fire.
        let edge = Column {
            lx: 0,
            lz: 4,
            x: 0,
            z: 4,
        };
        assert!(!fire_fits(&chunk_with(&[(0, STONE)]), edge, 1));
    }

    #[test]
    fn a_fire_is_lit_only_after_the_vines_have_grown() {
        let col = Column {
            lx: 3,
            lz: 4,
            x: 3,
            z: 4,
        };
        let spot = FireSpot { col, y: 1, h: 0 };
        let mut chunk = chunk_with(&[(0, STONE)]);
        light_fire(&mut chunk, spot);
        assert_eq!(col.get(&chunk, 1), Some(NETHERRACK));
        assert_eq!(col.get(&chunk, 2), Some(FIRE));
        assert_eq!(kind(FIRE), Kind::Light);

        // A vine hung beside it after the spot was kept leaves rubble there instead.
        let mut chunk = chunk_with(&[(0, STONE)]);
        chunk.set_block(4, 3, 4, VINES[3]);
        light_fire(&mut chunk, spot);
        assert_eq!(col.get(&chunk, 1), Some(rubble(0)));
        assert_eq!(col.get(&chunk, 2), None);
    }

    #[test]
    fn snow_rests_only_on_full_slabs_and_upper_stairs() {
        let overrides = Default::default();
        let terrain = terrain_without_ground(&overrides);
        let col = Column {
            lx: 3,
            lz: 4,
            x: 3,
            z: 4,
        };
        let state = |key: &str, value: &str| {
            let mut map = std::collections::HashMap::new();
            map.insert(key.to_string(), Value::String(value.to_string()));
            Some(Value::Compound(map))
        };
        let covered =
            |chunk: &ChunkToModify| col.get(chunk, 6).is_some_and(|b| kind(b) == Kind::Snowy);

        for (block, props) in [
            (STONE_BRICK_SLAB, state("type", "top")),
            (STONE_BRICK_SLAB, state("type", "double")),
            (STONE_BRICK_STAIRS, state("half", "top")),
        ] {
            let mut chunk = chunk_with(&[(0, STONE)]);
            chunk.set_block_with_properties(3, 5, 4, BlockWithProperties::new(block, props));
            snow_over(&mut chunk, col, 0, &terrain, None);
            assert!(covered(&chunk));
        }

        // A bottom stair holds no snow; it turns white, or stays bare, keeping its facing.
        let stair = state("half", "bottom");
        let mut chunk = chunk_with(&[(0, STONE)]);
        chunk.set_block_with_properties(
            3,
            5,
            4,
            BlockWithProperties::new(STONE_BRICK_STAIRS, stair.clone()),
        );
        snow_over(&mut chunk, col, 0, &terrain, None);
        assert!(!covered(&chunk));
        assert!(matches!(
            col.get(&chunk, 5),
            Some(STONE_BRICK_STAIRS | SMOOTH_QUARTZ_STAIRS)
        ));
        assert_eq!(
            col.properties(&chunk, 5, STONE_BRICK_STAIRS).as_deref(),
            stair.as_ref()
        );

        // A slab without a state is a bottom one.
        let mut chunk = chunk_with(&[(0, STONE), (5, STONE_BRICK_SLAB)]);
        snow_over(&mut chunk, col, 0, &terrain, None);
        assert!(!covered(&chunk));
    }

    #[test]
    fn snow_on_a_field_turns_its_farmland_to_dirt() {
        let overrides = Default::default();
        let terrain = terrain_without_ground(&overrides);
        let col = Column {
            lx: 3,
            lz: 4,
            x: 3,
            z: 4,
        };
        let mut chunk = chunk_with(&[(0, FARMLAND), (1, crate::block_definitions::WHEAT)]);
        snow_over(&mut chunk, col, 0, &terrain, None);
        assert_eq!(col.get(&chunk, 0), Some(DIRT));
        assert!(col.get(&chunk, 1).is_some_and(|b| kind(b) == Kind::Snowy));
    }

    #[test]
    fn snow_leaves_a_blueprint_outline_bare() {
        use crate::block_definitions::LIGHT_BLUE_CONCRETE;
        let overrides = Default::default();
        let terrain = terrain_without_ground(&overrides);
        let col = Column {
            lx: 3,
            lz: 4,
            x: 3,
            z: 4,
        };
        let mut chunk = chunk_with(&[(0, GRASS_BLOCK), (1, LIGHT_BLUE_CONCRETE)]);
        snow_over(&mut chunk, col, 0, &terrain, Some(LIGHT_BLUE_CONCRETE));
        assert_eq!(col.get(&chunk, 2), None);
        // The same block anywhere else, or in another mode, is snowed over.
        let mut roof = chunk_with(&[(0, GRASS_BLOCK), (5, LIGHT_BLUE_CONCRETE)]);
        snow_over(&mut roof, col, 0, &terrain, Some(LIGHT_BLUE_CONCRETE));
        assert!(col.get(&roof, 6).is_some_and(|b| kind(b) == Kind::Snowy));
        snow_over(&mut chunk, col, 0, &terrain, None);
        assert!(col.get(&chunk, 2).is_some_and(|b| kind(b) == Kind::Snowy));
    }

    #[test]
    fn weathered_stairs_keep_their_facing() {
        use crate::block_definitions::{
            create_stair_with_properties, StairFacing, StairShape, MOSSY_STONE_BRICK_STAIRS,
            STONE_BRICK_STAIRS,
        };
        let col = Column {
            lx: 3,
            lz: 4,
            x: 3,
            z: 4,
        };
        let mut chunk = ChunkToModify::default();
        let stair = create_stair_with_properties(
            STONE_BRICK_STAIRS,
            StairFacing::East,
            StairShape::Straight,
        );
        let props = stair.properties.clone();
        chunk.set_block_with_properties(3, 5, 4, stair);
        col.swap(&mut chunk, 5, MOSSY_STONE_BRICK_STAIRS);
        assert_eq!(col.get(&chunk, 5), Some(MOSSY_STONE_BRICK_STAIRS));
        let index = SectionToModify::index(3, 5, 4);
        assert_eq!(
            chunk.sections.get(0).unwrap().properties.get(&index),
            props.as_ref()
        );
    }

    #[test]
    fn weathering_turns_only_a_share_of_a_material() {
        let turned = (0..1000u64)
            .filter(|&h| weathered(STONE_BRICKS, h).is_some())
            .count();
        assert_eq!(turned, 550);
        assert_eq!(weathered(OAK_LOG, 0), None);
    }
}
