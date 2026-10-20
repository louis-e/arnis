//! World styles, applied to each finished region just before it is written: the ruins of
//! a post-apocalyptic world and the snow and ice of a winter one. They work on the final
//! blocks, so they reach everything placed before, trees, roofs and landmarks alike.
use std::sync::OnceLock;

use super::common::{world_max_y, ChunkToModify, SectionToModify};
use super::WorldEditor;
use crate::block_definitions::{
    Block, BlockWithProperties, AIR, ANDESITE, COARSE_DIRT, COBBLESTONE, COBBLESTONE_SLAB,
    COBBLESTONE_STAIRS, COBBLESTONE_WALL, COBWEB, CRACKED_DEEPSLATE_BRICKS,
    CRACKED_DEEPSLATE_TILES, CRACKED_NETHER_BRICKS, CRACKED_POLISHED_BLACKSTONE_BRICKS,
    CRACKED_STONE_BRICKS, DEAD_BUSH, DEEPSLATE_BRICKS, DEEPSLATE_TILES, FIRE, GRASS, GRASS_BLOCK,
    GRAVEL, ICE, MOSSY_COBBLESTONE, MOSSY_COBBLESTONE_SLAB, MOSSY_COBBLESTONE_STAIRS,
    MOSSY_COBBLESTONE_WALL, MOSSY_STONE_BRICKS, MOSSY_STONE_BRICK_SLAB, MOSSY_STONE_BRICK_STAIRS,
    MOSSY_STONE_BRICK_WALL, MOSS_BLOCK, MOSS_CARPET, NETHERRACK, NETHER_BRICK, PODZOL,
    POLISHED_ANDESITE, POLISHED_BLACKSTONE_BRICKS, SMOOTH_STONE, SNOWY_GRASS_BLOCK, SNOWY_PODZOL,
    SNOW_LAYERS, STONE, STONE_BRICKS, STONE_BRICK_SLAB, STONE_BRICK_STAIRS, STONE_BRICK_WALL,
    TALL_GRASS_BOTTOM, TALL_GRASS_TOP, VINES,
};
use crate::coordinate_system::cartesian::XZPoint;
use crate::floodfill_cache::{CoordinateBitmap, SealedSurfaceBitmap};
use crate::ground::Ground;
use crate::ground_generation::patch_noise;
use crate::land_cover::{coord_hash, LC_WATER};

const SALT_COLLAPSE: u32 = 0x5255_494E;
const SALT_JAG: u32 = 0x4A41_4747;
const SALT_HOLE: u32 = 0x484F_4C45;
const SALT_CRACK: u32 = 0x4352_4B53;
const SALT_RUBBLE: u32 = 0x5242_4C45;
const SALT_DEPTH: u32 = 0x534E_4F57;
const SALT_PLOW: u32 = 0x504C_4F57;
const SALT_ICE: u32 = 0x4943_4521;
const SALT_VINE: u32 = 0x5649_4E45;
const SALT_MOSS: u32 = 0x4D4F_5353;

/// Share of street-facing wall that vines climb.
const VINE_SHARE: f64 = 0.35;

/// How far into the street a ruin throws its rubble, in blocks.
const RUBBLE_REACH: i32 = 2;
/// One rubble pile in this many burns: netherrack with a fire on it that never goes out.
const FIRE_ODDS: u64 = 240;
/// How high moss creeps up a ruin's walls.
const MOSS_REACH: i32 = 6;
/// Lamps stand on short posts, so lights are looked for this far above the ground.
const LAMP_REACH: i32 = 12;

/// Which styles a run applies, and what they need from the generation.
pub(crate) struct WorldStyle<'a> {
    pub winter: bool,
    pub ruins: bool,
    /// Building footprints, which the ruins break open and pile rubble around.
    pub footprints: Option<&'a CoordinateBitmap>,
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

const LIGHTS: [&str; 6] = [
    "glowstone",
    "sea_lantern",
    "shroomlight",
    "redstone_lamp",
    "jack_o_lantern",
    "end_rod",
];

/// Parts of names that mark a block nothing can settle on.
const THIN: [&str; 40] = [
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
                for &(col, ground_y) in &columns {
                    if in_building(&col) {
                        ruin_building(chunk, col, ground_y);
                    } else {
                        ruin_open_ground(chunk, col, ground_y, &terrain, style);
                    }
                }
                // Once every wall of the chunk has come down as far as it will.
                if let Some(footprints) = style.footprints {
                    for &(col, ground_y) in &columns {
                        if !in_building(&col) {
                            hang_vines(chunk, col, ground_y, footprints);
                        }
                    }
                }
            }
            if style.winter {
                for &(col, ground_y) in &columns {
                    snow_over(chunk, col, ground_y, &terrain);
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

    /// Swaps the block at `y` for a variant of it, keeping the facing and shape it stood in.
    fn swap(self, chunk: &mut ChunkToModify, y: i32, block: Block) {
        let index = SectionToModify::index(self.lx, (y & 15) as u8, self.lz);
        let props = chunk
            .sections
            .get((y >> 4) as i8)
            .and_then(|s| s.properties.get(&index).cloned());
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
        let mut sections: Vec<i8> = chunk.sections.keys().collect();
        sections.sort_unstable_by(|a, b| b.cmp(a));
        for sy in sections {
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

/// Breaks a building column: part of it gone, the rest holed, weathered and dark.
fn ruin_building(chunk: &mut ChunkToModify, col: Column, ground_y: i32) {
    let Some((top, _)) = col.top(chunk) else {
        return;
    };
    let height = top - ground_y;
    if height < 2 {
        return;
    }
    // About a quarter of the ground stays under standing walls; elsewhere the building has
    // come down to a tenth to nine tenths of its height, along a jagged break.
    let standing = patch_noise(col.x, col.z, 26, SALT_COLLAPSE);
    let share = if standing > 0.72 {
        1.0
    } else {
        0.1 + standing / 0.72 * 0.8
    };
    let jag = (patch_noise(col.x, col.z, 5, SALT_JAG) - 0.5) * 4.0;
    let cut = ground_y + ((f64::from(height) * share + jag).round() as i32).clamp(1, height);
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
        let hole = patch_noise(col.x + y * 3, col.z - y * 2, 4, SALT_HOLE) < 0.1;
        if hole || k == Kind::Light || (k == Kind::Glass && (4..85).contains(&roll)) {
            col.set(chunk, y, AIR);
        } else if k == Kind::Glass && roll < 4 {
            col.set(chunk, y, COBWEB);
        } else if k == Kind::Solid && moss_creeps(col, y, y - ground_y) {
            col.set(chunk, y, MOSS_BLOCK);
        } else if let Some(variant) = weathered(block, h) {
            col.swap(chunk, y, variant);
        }
    }

    // The broken top carries what fell on it, and moss where it has lain long enough.
    if cut < top
        && cut < world_max_y()
        && col.get(chunk, cut).is_some_and(|b| kind(b) == Kind::Solid)
    {
        match col.hash(cut + 1) % 6 {
            0 | 1 => rubble_or_fire(chunk, col, cut + 1, col.hash(cut + 2)),
            2 => col.set(chunk, cut + 1, MOSS_CARPET),
            _ => {}
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

/// A rubble block at `y`, or now and then a fire burning on netherrack where nothing it
/// could spread to stands within its reach.
fn rubble_or_fire(chunk: &mut ChunkToModify, col: Column, y: i32, h: u64) {
    if (h >> 24).is_multiple_of(FIRE_ODDS) && fire_fits(chunk, col, y) {
        col.set(chunk, y, NETHERRACK);
        col.set(chunk, y + 1, FIRE);
    } else {
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
    let height = 2 + ((VINE_SHARE - patch) / VINE_SHARE * 14.0) as i32;
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

    let near_ruin = style.footprints.is_some_and(|f| {
        (-RUBBLE_REACH..=RUBBLE_REACH)
            .any(|dz| (-RUBBLE_REACH..=RUBBLE_REACH).any(|dx| f.contains(col.x + dx, col.z + dz)))
    });
    if near_ruin && patch_noise(col.x, col.z, 6, SALT_RUBBLE) < 0.45 && h.is_multiple_of(3) {
        rubble_or_fire(chunk, col, top + 1, h >> 8);
        return;
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
                grow(chunk, col, top + 1, h >> 8, 70);
            }
        }
    } else if block == GRASS_BLOCK {
        // Lawns gone wild.
        grow(chunk, col, top + 1, h >> 8, 30);
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
fn snow_over(chunk: &mut ChunkToModify, col: Column, ground_y: i32, terrain: &Terrain) {
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
            // Ploughed roads keep only scattered ridges of snow.
            let road = on_ground && terrain.sealed(col.x, col.z);
            if road && patch_noise(col.x, col.z, 3, SALT_PLOW) < 0.92 {
                return;
            }
            let thin = road || kind(block) == Kind::Leaves;
            col.set(chunk, top + 1, snow_layer(col, on_ground, thin));
            snowy_ground(chunk, col, top);
        }
        _ => {}
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
/// a block update.
fn snowy_ground(chunk: &mut ChunkToModify, col: Column, y: i32) {
    let snowy = match col.get(chunk, y) {
        Some(GRASS_BLOCK) => SNOWY_GRASS_BLOCK,
        Some(PODZOL) => SNOWY_PODZOL,
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
        snow_over(&mut chunk, col, 0, &terrain);
        assert_eq!(col.get(&chunk, 0), Some(SNOWY_GRASS_BLOCK));
        assert!(col.get(&chunk, 1).is_some_and(|b| kind(b) == Kind::Snowy));

        let mut chunk = chunk_with(&[
            (0, GRASS_BLOCK),
            (1, TALL_GRASS_BOTTOM),
            (2, TALL_GRASS_TOP),
        ]);
        snow_over(&mut chunk, col, 0, &terrain);
        assert_eq!(col.get(&chunk, 2), None);
        assert!(col.get(&chunk, 1).is_some_and(|b| kind(b) == Kind::Snowy));

        let mut chunk = chunk_with(&[(0, STONE), (1, GLASS_PANE)]);
        snow_over(&mut chunk, col, 0, &terrain);
        assert_eq!(col.get(&chunk, 2), None);

        let mut chunk = chunk_with(&[(0, crate::block_definitions::WATER)]);
        snow_over(&mut chunk, col, 0, &terrain);
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
        let mut chunk = chunk_with(&blocks);
        ruin_building(&mut chunk, col, 0);
        assert_eq!(col.get(&chunk, 0), Some(STONE));
        assert!((1..=30).all(|y| col.get(&chunk, y) != Some(GLASS) || col.hash(y) % 100 >= 85));
        let top = col.top(&chunk).unwrap().0;
        assert!((1..=30).contains(&top));
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
        assert_eq!(weathered(crate::block_definitions::BRICK, 0), None);
    }
}
