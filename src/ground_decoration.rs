//! Plant patches laid over the finished ground, after the way Minecraft decorates
//! its own terrain.
//!
//! The game scatters each feature as a patch: a few dozen tries around one origin,
//! the offsets summed from two dice so they bunch in the middle. Each chunk here
//! rolls a few origins, the habitat under an origin (land cover, climate, altitude)
//! picks what grows there, and the patch keeps to that habitat. Flowers come in
//! drifts of one or two species, ferns and berries gather under conifers,
//! mushrooms only in shade, cane only beside water, lily pads on wetland pools.
//!
//! Everything is keyed on world coordinates and only writes inside the pass's
//! bounds, so tiles stitch without seams.

use crate::args::Args;
use crate::block_definitions::{
    Block, ACACIA_LEAVES, ACACIA_LOG, ALLIUM, AZALEA_LEAVES, BIRCH_LEAVES, BIRCH_LOG, BLUE_FLOWER,
    BROWN_MUSHROOM, CACTUS, CHERRY_LOG, COARSE_DIRT, CORNFLOWER, DARK_OAK_LEAVES, DARK_OAK_LOG,
    DEAD_BUSH, DIRT, FERN, FLOWERING_AZALEA_LEAVES, GRASS, GRASS_BLOCK, JUNGLE_LEAVES, JUNGLE_LOG,
    LARGE_FERN_LOWER, LARGE_FERN_UPPER, LILAC_LOWER, LILAC_UPPER, LILY_OF_THE_VALLEY, LILY_PAD,
    MANGROVE_LOG, MOSS_BLOCK, MOSS_CARPET, MUD, MYCELIUM, OAK_LEAVES, OAK_LOG, ORANGE_TERRACOTTA,
    ORANGE_TULIP, OXEYE_DAISY, PEONY_LOWER, PEONY_UPPER, PINK_TULIP, PODZOL, PUMPKIN, RED_FLOWER,
    RED_MUSHROOM, RED_TERRACOTTA, RED_TULIP, ROSE_BUSH_LOWER, ROSE_BUSH_UPPER, SAND, SPRUCE_LEAVES,
    SPRUCE_LOG, SUGAR_CANE, SUNFLOWER_LOWER, SUNFLOWER_UPPER, SWEET_BERRY_BUSH, TALL_GRASS_BOTTOM,
    TALL_GRASS_TOP, TERRACOTTA, WATER, WHITE_FLOWER, WHITE_TULIP, YELLOW_FLOWER, YELLOW_TERRACOTTA,
};
use crate::climate::Climate;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ecoregion::{EcoBiome, Ecoregion};
use crate::ground::Ground;
use crate::land_cover::{
    LC_BARE, LC_GRASSLAND, LC_MANGROVES, LC_MOSS, LC_SHRUBLAND, LC_TREE_COVER, LC_WATER, LC_WETLAND,
};
use crate::world_editor::WorldEditor;
use rand::Rng;
use rand_chacha::ChaCha8Rng;

/// Loose plants standing on the ground, which later passes may strip again
/// (road cleanup, fresh water carves). Upper halves of tall plants included.
pub(crate) const LOOSE_PLANTS: &[Block] = &[
    GRASS,
    TALL_GRASS_BOTTOM,
    TALL_GRASS_TOP,
    FERN,
    LARGE_FERN_LOWER,
    LARGE_FERN_UPPER,
    DEAD_BUSH,
    RED_FLOWER,
    YELLOW_FLOWER,
    BLUE_FLOWER,
    WHITE_FLOWER,
    CORNFLOWER,
    OXEYE_DAISY,
    ALLIUM,
    LILY_OF_THE_VALLEY,
    RED_TULIP,
    ORANGE_TULIP,
    WHITE_TULIP,
    PINK_TULIP,
    SUNFLOWER_LOWER,
    SUNFLOWER_UPPER,
    LILAC_LOWER,
    LILAC_UPPER,
    ROSE_BUSH_LOWER,
    ROSE_BUSH_UPPER,
    PEONY_LOWER,
    PEONY_UPPER,
    SWEET_BERRY_BUSH,
    BROWN_MUSHROOM,
    RED_MUSHROOM,
    MOSS_CARPET,
    SUGAR_CANE,
    PUMPKIN,
    CACTUS,
    OAK_LEAVES,
    BIRCH_LEAVES,
    SPRUCE_LEAVES,
    DARK_OAK_LEAVES,
    JUNGLE_LEAVES,
    ACACIA_LEAVES,
    AZALEA_LEAVES,
    FLOWERING_AZALEA_LEAVES,
];

/// A loose plant other than a leaf-block bush, which can't be told apart from
/// a tree's own foliage.
pub(crate) fn is_undergrowth(block: Block) -> bool {
    !crate::element_processing::bush::BUSH_LEAVES.contains(&block) && LOOSE_PLANTS.contains(&block)
}

/// Tree logs. Nothing that grows on the ground belongs directly beneath one: a
/// plant squeezed under a branch or trunk reads as having taken the wood's place.
pub(crate) const WOOD: &[Block] = &[
    OAK_LOG,
    SPRUCE_LOG,
    BIRCH_LOG,
    DARK_OAK_LOG,
    JUNGLE_LOG,
    ACACIA_LOG,
    CHERRY_LOG,
    MANGROVE_LOG,
];

/// Lower halves of the two-block plants in `LOOSE_PLANTS`.
pub(crate) const PLANT_LOWER_HALVES: &[Block] = &[
    TALL_GRASS_BOTTOM,
    LARGE_FERN_LOWER,
    SUNFLOWER_LOWER,
    LILAC_LOWER,
    ROSE_BUSH_LOWER,
    PEONY_LOWER,
];

/// What stands above the ground block of a plant in `LOOSE_PLANTS`: upper halves
/// of two-block plants, and the rest of a stack of cane or cactus.
pub(crate) const STACKED_PLANT_PARTS: &[Block] = &[
    TALL_GRASS_TOP,
    LARGE_FERN_UPPER,
    SUNFLOWER_UPPER,
    LILAC_UPPER,
    ROSE_BUSH_UPPER,
    PEONY_UPPER,
    SUGAR_CANE,
    CACTUS,
];

/// Patch origins rolled per chunk; each picks at most one feature.
const ORIGINS_PER_CHUNK: u64 = 4;
/// Largest patch spread, so a pass reaches origins this far outside its bounds.
const MAX_SPREAD: i32 = 7;
/// Blocks across one drift of flowers sharing their main species.
const DRIFT_CELL: i32 = 40;
/// Metres below the snow line where alpine meadows start.
pub(crate) const ALPINE_BAND_METRES: f64 = 1000.0;

const SALT_ORIGIN: u64 = 0x9A7C_4E11_D00D_0001;
const SALT_DRIFT: u64 = 0x9A7C_4E11_D00D_0002;
const SALT_SCATTER: u32 = 0x5CA7_7E11;
const SALT_CLUMP: u32 = 0x00C1_0A9F;
const SALT_BED: u32 = 0x0BED_F10E;

/// What a patch origin is standing in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Habitat {
    Meadow,
    Alpine,
    Forest,
    Taiga,
    Jungle,
    Shrub,
    Steppe,
    Desert,
    Tundra,
    Wetland,
    /// Temperate grassland: tall grass, sunflowers and prairie flowers.
    Prairie,
    /// Mediterranean scrub: poppies, dry grass and low shrubs.
    Maquis,
    /// Tropical grassland: tall grass with the odd dry bush.
    Savanna,
}

impl Habitat {
    /// Meadow and alpine meadow are one sward at different heights.
    fn same_ground(self, other: Habitat) -> bool {
        self == other
            || matches!(
                (self, other),
                (Habitat::Meadow, Habitat::Alpine) | (Habitat::Alpine, Habitat::Meadow)
            )
    }
}

pub(crate) fn habitat(
    cover: u8,
    climate: Climate,
    abs_lat: f64,
    alpine: bool,
    eco: Option<Ecoregion>,
) -> Option<Habitat> {
    if let Some(h) = eco.and_then(|e| ecoregion_habitat(cover, climate, alpine, e)) {
        return Some(h);
    }
    let arid = matches!(climate, Climate::HotDesert | Climate::ColdDesert);
    let dry = matches!(
        climate,
        Climate::HotSteppe
            | Climate::ColdSteppe
            | Climate::TropicalSavanna
            | Climate::DryContinental
    );
    let polar = matches!(climate, Climate::Tundra | Climate::IceCap);
    Some(match cover {
        LC_TREE_COVER => {
            if matches!(climate, Climate::Boreal) || polar || abs_lat > 55.0 {
                Habitat::Taiga
            } else if arid || dry {
                Habitat::Shrub
            } else if abs_lat < 23.5 {
                Habitat::Jungle
            } else {
                Habitat::Forest
            }
        }
        LC_SHRUBLAND | LC_GRASSLAND if alpine && !arid => Habitat::Alpine,
        LC_SHRUBLAND if arid || dry => Habitat::Steppe,
        LC_SHRUBLAND if polar => Habitat::Tundra,
        LC_SHRUBLAND => Habitat::Shrub,
        LC_GRASSLAND if arid => Habitat::Desert,
        LC_GRASSLAND if dry => Habitat::Steppe,
        LC_GRASSLAND if polar => Habitat::Tundra,
        LC_GRASSLAND => Habitat::Meadow,
        LC_MOSS => Habitat::Tundra,
        LC_WETLAND | LC_MANGROVES => Habitat::Wetland,
        LC_BARE if arid || dry => Habitat::Desert,
        _ => return None,
    })
}

/// Habitat by ecoregion; alpine, arid and snowy ground keeps the climate reading.
fn ecoregion_habitat(cover: u8, climate: Climate, alpine: bool, eco: Ecoregion) -> Option<Habitat> {
    use EcoBiome::*;
    if alpine
        || matches!(
            climate,
            Climate::HotDesert
                | Climate::ColdDesert
                | Climate::Boreal
                | Climate::Tundra
                | Climate::IceCap
        )
    {
        return None;
    }
    let steppe_climate = matches!(climate, Climate::HotSteppe | Climate::ColdSteppe);
    Some(match (cover, eco.biome) {
        (LC_TREE_COVER, MoistTropical) => Habitat::Jungle,
        (LC_TREE_COVER, DryTropical | TropicalGrassland | Mediterranean | Desert) => Habitat::Shrub,
        (LC_TREE_COVER, TemperateConifer | Boreal | Tundra) => Habitat::Taiga,
        (
            LC_TREE_COVER,
            TemperateBroadleaf | TropicalConifer | TemperateGrassland | MontaneGrassland,
        ) => Habitat::Forest,
        (LC_GRASSLAND | LC_SHRUBLAND, TemperateGrassland) if !steppe_climate => Habitat::Prairie,
        (LC_GRASSLAND | LC_SHRUBLAND, Mediterranean) => Habitat::Maquis,
        (LC_GRASSLAND | LC_SHRUBLAND, TropicalGrassland) => Habitat::Savanna,
        (LC_GRASSLAND | LC_SHRUBLAND, MontaneGrassland) => Habitat::Alpine,
        (LC_GRASSLAND | LC_SHRUBLAND, Desert) => Habitat::Steppe,
        _ => return None,
    })
}

type Palette = &'static [(Block, u32)];
type TallPalette = &'static [((Block, Block), u32)];

const MEADOW_FLOWERS: Palette = &[
    (YELLOW_FLOWER, 30),
    (RED_FLOWER, 25),
    (OXEYE_DAISY, 20),
    (CORNFLOWER, 12),
    (WHITE_FLOWER, 12),
    (ALLIUM, 4),
    (RED_TULIP, 3),
    (ORANGE_TULIP, 2),
    (WHITE_TULIP, 2),
    (PINK_TULIP, 2),
];
const FOREST_FLOWERS: Palette = &[
    (RED_FLOWER, 30),
    (YELLOW_FLOWER, 25),
    (LILY_OF_THE_VALLEY, 25),
    (ALLIUM, 5),
    (OXEYE_DAISY, 5),
];
const BOREAL_FLOWERS: Palette = &[
    (YELLOW_FLOWER, 40),
    (LILY_OF_THE_VALLEY, 25),
    (RED_FLOWER, 20),
    (WHITE_FLOWER, 15),
];
const TROPICAL_FLOWERS: Palette = &[(RED_FLOWER, 45), (BLUE_FLOWER, 30), (YELLOW_FLOWER, 25)];
const SHRUB_FLOWERS: Palette = &[
    (YELLOW_FLOWER, 30),
    (RED_FLOWER, 30),
    (ALLIUM, 20),
    (WHITE_FLOWER, 20),
];
const STEPPE_FLOWERS: Palette = &[(YELLOW_FLOWER, 40), (ALLIUM, 30), (RED_FLOWER, 30)];
const TUNDRA_FLOWERS: Palette = &[(WHITE_FLOWER, 40), (YELLOW_FLOWER, 30), (OXEYE_DAISY, 30)];
const WETLAND_FLOWERS: Palette = &[(BLUE_FLOWER, 70), (OXEYE_DAISY, 15), (YELLOW_FLOWER, 15)];
const PRAIRIE_FLOWERS: Palette = &[
    (YELLOW_FLOWER, 35),
    (OXEYE_DAISY, 20),
    (ALLIUM, 15),
    (CORNFLOWER, 15),
    (RED_FLOWER, 10),
    (ORANGE_TULIP, 5),
];
const MAQUIS_FLOWERS: Palette = &[
    (RED_FLOWER, 35),
    (YELLOW_FLOWER, 25),
    (ALLIUM, 20),
    (WHITE_FLOWER, 20),
];
const ALPINE_FLOWERS: Palette = &[
    (WHITE_FLOWER, 30),
    (CORNFLOWER, 20),
    (ALLIUM, 20),
    (OXEYE_DAISY, 15),
    (YELLOW_FLOWER, 15),
];
const FOREST_TALL_FLOWERS: TallPalette = &[
    ((LILAC_LOWER, LILAC_UPPER), 35),
    ((ROSE_BUSH_LOWER, ROSE_BUSH_UPPER), 35),
    ((PEONY_LOWER, PEONY_UPPER), 30),
];
const SUNFLOWERS: TallPalette = &[((SUNFLOWER_LOWER, SUNFLOWER_UPPER), 1)];
/// Park beds and gardens: the cultivated flowers.
const GARDEN_FLOWERS: Palette = &[
    (RED_TULIP, 18),
    (PINK_TULIP, 14),
    (WHITE_TULIP, 12),
    (ORANGE_TULIP, 12),
    (ALLIUM, 12),
    (CORNFLOWER, 10),
    (OXEYE_DAISY, 12),
    (RED_FLOWER, 10),
];

/// Where a mapped area scatters single flowers.
#[derive(Clone, Copy)]
pub(crate) enum FlowerSetting {
    Meadow,
    Forest,
    Garden,
}

/// Plants a flower one block above a mapped meadow, wood or park, whose own
/// code scatters single flowers. Only inside clumps, with plain sward between
/// them, and the species follows a smooth field, so neighbouring flowers mostly
/// share one and the scatter reads as drifts, not confetti.
pub(crate) fn place_scattered_flower(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    setting: FlowerSetting,
) {
    if crate::ground_generation::patch_noise(x, z, 11, SALT_CLUMP) >= 0.6 {
        editor.set_block(scattered_flower(x, z, setting), x, 1, z, None, None);
    }
}

/// Dense garden flowers for a mapped flower bed, one variety per patch.
pub(crate) fn place_bed_flower(editor: &mut WorldEditor, x: i32, z: i32) {
    if crate::land_cover::coord_hash(x ^ 0x0BED, z ^ 0x5EED) % 100 < 12 {
        return;
    }
    let zone = crate::ground_generation::patch_noise(x, z, 4, SALT_BED);
    let flower = pick_weighted(GARDEN_FLOWERS, (zone * 999.0) as u32);
    editor.set_block(flower, x, 1, z, None, None);
}

fn scattered_flower(x: i32, z: i32, setting: FlowerSetting) -> Block {
    let palette = match setting {
        FlowerSetting::Meadow => MEADOW_FLOWERS,
        FlowerSetting::Forest => FOREST_FLOWERS,
        FlowerSetting::Garden => GARDEN_FLOWERS,
    };
    let zone = crate::ground_generation::patch_noise(x, z, 24, SALT_SCATTER);
    let roll = crate::land_cover::coord_hash(x ^ 0x5CA7, z ^ 0x7E11);
    if roll % 100 < 85 {
        pick_weighted(palette, (zone * 999.0) as u32)
    } else {
        pick_weighted(palette, ((roll >> 8) % 1000) as u32)
    }
}

#[derive(Clone, Copy)]
enum Feature {
    Flowers(Palette),
    TallFlowers(TallPalette),
    TallGrass,
    Ferns,
    Mushrooms,
    SugarCane,
    Pumpkins,
    SweetBerries,
    DeadBushes,
    Cactus,
    LilyPads,
    Moss,
}

impl Feature {
    /// Tries and spread of one patch, after the game's own patch sizes.
    fn shape(self) -> (u32, i32) {
        match self {
            Feature::Flowers(_) => (48, 6),
            Feature::TallFlowers(_) => (24, 5),
            Feature::TallGrass => (32, 5),
            Feature::Ferns => (40, 6),
            Feature::Mushrooms => (12, 3),
            Feature::SugarCane => (20, 4),
            Feature::Pumpkins => (8, 3),
            Feature::SweetBerries => (16, 4),
            Feature::DeadBushes => (6, 4),
            Feature::Cactus => (6, 5),
            Feature::LilyPads => (28, 6),
            Feature::Moss => (32, 4),
        }
    }
}

/// Features an origin may start, per mille per origin; the remainder grows nothing.
fn features(habitat: Habitat) -> &'static [(Feature, u32)] {
    use Feature::*;
    match habitat {
        Habitat::Meadow => &[
            (Flowers(MEADOW_FLOWERS), 90),
            (TallGrass, 100),
            (TallFlowers(SUNFLOWERS), 6),
            (Pumpkins, 1),
            (SugarCane, 70),
        ],
        Habitat::Alpine => &[(Flowers(ALPINE_FLOWERS), 160), (TallGrass, 50)],
        Habitat::Forest => &[
            (Flowers(FOREST_FLOWERS), 50),
            (TallFlowers(FOREST_TALL_FLOWERS), 35),
            (Ferns, 80),
            (Mushrooms, 60),
            (SugarCane, 50),
            (Pumpkins, 1),
        ],
        Habitat::Taiga => &[
            (Ferns, 200),
            (SweetBerries, 60),
            (Mushrooms, 80),
            (Flowers(BOREAL_FLOWERS), 20),
            (Moss, 40),
        ],
        Habitat::Jungle => &[
            (Ferns, 180),
            (Flowers(TROPICAL_FLOWERS), 30),
            (SugarCane, 100),
            (TallGrass, 60),
        ],
        Habitat::Shrub => &[(Flowers(SHRUB_FLOWERS), 50), (TallGrass, 70)],
        Habitat::Steppe => &[
            (TallGrass, 140),
            (DeadBushes, 80),
            (Flowers(STEPPE_FLOWERS), 20),
            (SugarCane, 60),
            (Cactus, 30),
        ],
        Habitat::Desert => &[(DeadBushes, 160), (Cactus, 80), (SugarCane, 100)],
        Habitat::Tundra => &[
            (Flowers(TUNDRA_FLOWERS), 30),
            (Moss, 60),
            (SweetBerries, 20),
        ],
        Habitat::Wetland => &[
            (LilyPads, 180),
            (Flowers(WETLAND_FLOWERS), 60),
            (SugarCane, 100),
            (TallGrass, 60),
            (Mushrooms, 20),
        ],
        Habitat::Prairie => &[
            (TallGrass, 180),
            (Flowers(PRAIRIE_FLOWERS), 70),
            (TallFlowers(SUNFLOWERS), 25),
            (SugarCane, 40),
        ],
        Habitat::Maquis => &[
            (Flowers(MAQUIS_FLOWERS), 60),
            (TallGrass, 60),
            (DeadBushes, 40),
            (SweetBerries, 30),
            (SugarCane, 30),
        ],
        Habitat::Savanna => &[
            (TallGrass, 220),
            (DeadBushes, 30),
            (Flowers(SHRUB_FLOWERS), 20),
            (SugarCane, 40),
        ],
    }
}

const SOIL: &[Block] = &[GRASS_BLOCK, PODZOL, DIRT, COARSE_DIRT, MOSS_BLOCK];
const SHADE_SOIL: &[Block] = &[GRASS_BLOCK, PODZOL, DIRT, COARSE_DIRT, MOSS_BLOCK, MYCELIUM];
const CANE_SOIL: &[Block] = &[GRASS_BLOCK, DIRT, COARSE_DIRT, PODZOL, SAND, MUD];
const DRY_SOIL: &[Block] = &[
    SAND,
    COARSE_DIRT,
    DIRT,
    TERRACOTTA,
    RED_TERRACOTTA,
    ORANGE_TERRACOTTA,
    YELLOW_TERRACOTTA,
];

/// Everything one pass needs to know about where it is.
struct Site<'g> {
    ground: &'g Ground,
    origin_x: i32,
    origin_z: i32,
    bounds: (i32, i32, i32, i32),
    terrain: bool,
    flat_y: i32,
    climate: Climate,
    abs_lat: f64,
    /// Cacti are American: this longitude test stands in where no ecoregion's realm decides.
    cactus_country: bool,
    alpine_from_y: i32,
}

impl Site<'_> {
    fn contains(&self, x: i32, z: i32) -> bool {
        let (min_x, max_x, min_z, max_z) = self.bounds;
        x >= min_x
            && x <= max_x
            && z >= min_z
            && z <= max_z
            && self.ground.is_in_rotated_bounds(x, z)
    }

    fn ground_y(&self, editor: &WorldEditor, x: i32, z: i32) -> i32 {
        if self.terrain {
            editor.get_ground_level(x, z)
        } else {
            self.flat_y
        }
    }

    fn cover(&self, x: i32, z: i32) -> u8 {
        self.ground
            .cover_class(XZPoint::new(x - self.origin_x, z - self.origin_z))
    }

    fn ecoregion(&self, x: i32, z: i32) -> Option<Ecoregion> {
        self.ground
            .ecoregion(XZPoint::new(x - self.origin_x, z - self.origin_z))
    }

    fn cactus_country(&self, x: i32, z: i32) -> bool {
        self.ecoregion(x, z)
            .map_or(self.cactus_country, |e| e.realm.is_americas())
    }

    /// Read from the data grids alone, so an origin outside this pass's bounds
    /// resolves exactly as its own tile resolves it.
    fn habitat(&self, x: i32, z: i32) -> Option<Habitat> {
        let terrain_y = if self.terrain {
            self.ground
                .level(XZPoint::new(x - self.origin_x, z - self.origin_z))
        } else {
            self.flat_y
        };
        habitat(
            self.cover(x, z),
            self.climate,
            self.abs_lat,
            terrain_y >= self.alpine_from_y,
            self.ecoregion(x, z),
        )
    }
}

/// Lays plant patches over `[min_x..=max_x] x [min_z..=max_z]`, which the ground
/// pass has just finished. Needs land cover; without it the ground stays as is.
#[allow(clippy::too_many_arguments)]
pub fn decorate_region(
    editor: &mut WorldEditor,
    ground: &Ground,
    args: &Args,
    xzbbox: &XZBBox,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) {
    if !ground.has_land_cover() || !ground.body().is_earth() || min_x > max_x || min_z > max_z {
        return;
    }
    let (lat, lon) = args
        .bbox
        .as_ref()
        .map(|b| {
            (
                (b.min().lat() + b.max().lat()) * 0.5,
                (b.min().lng() + b.max().lng()) * 0.5,
            )
        })
        .unwrap_or((45.0, 0.0));
    let snow_y = ground.snow_threshold_y();
    let alpine_from_y = match snow_y {
        i32::MAX | i32::MIN => snow_y,
        t => t - (ALPINE_BAND_METRES * ground.blocks_per_meter()).round() as i32,
    };
    let site = Site {
        ground,
        origin_x: xzbbox.min_x(),
        origin_z: xzbbox.min_z(),
        bounds: (min_x, max_x, min_z, max_z),
        terrain: ground.elevation_enabled,
        flat_y: args.ground_level,
        climate: ground.climate(),
        abs_lat: lat.abs(),
        cactus_country: (-170.0..=-30.0).contains(&lon),
        alpine_from_y,
    };

    // Origins up to one spread outside the bounds still reach in.
    let chunk_min_x = (min_x - MAX_SPREAD) >> 4;
    let chunk_max_x = (max_x + MAX_SPREAD) >> 4;
    let chunk_min_z = (min_z - MAX_SPREAD) >> 4;
    let chunk_max_z = (max_z + MAX_SPREAD) >> 4;
    for chunk_x in chunk_min_x..=chunk_max_x {
        for chunk_z in chunk_min_z..=chunk_max_z {
            for i in 0..ORIGINS_PER_CHUNK {
                let mut rng =
                    crate::deterministic_rng::coord_rng(chunk_x, chunk_z, SALT_ORIGIN ^ (i << 48));
                let ox = (chunk_x << 4) + rng.random_range(0..16);
                let oz = (chunk_z << 4) + rng.random_range(0..16);
                let roll = rng.random_range(0..1000u32);
                let Some(habitat) = site.habitat(ox, oz) else {
                    continue;
                };
                let Some(feature) = pick_feature(features(habitat), roll) else {
                    continue;
                };
                if matches!(feature, Feature::Cactus) && !site.cactus_country(ox, oz) {
                    continue;
                }
                grow_patch(editor, &site, &mut rng, feature, habitat, ox, oz);
            }
        }
    }
}

fn pick_feature(table: &[(Feature, u32)], roll: u32) -> Option<Feature> {
    let mut acc = 0;
    for &(feature, weight) in table {
        acc += weight;
        if roll < acc {
            return Some(feature);
        }
    }
    None
}

/// The option a roll in `0..1000` lands on, by weight.
fn pick_weighted<T: Copy>(options: &[(T, u32)], roll_permille: u32) -> T {
    let total: u32 = options.iter().map(|&(_, w)| w).sum();
    let mut roll = roll_permille * total / 1000;
    for &(value, weight) in options {
        if roll < weight {
            return value;
        }
        roll -= weight;
    }
    options[options.len() - 1].0
}

/// One patch: `tries` offsets around the origin, each summed from two dice.
fn grow_patch(
    editor: &mut WorldEditor,
    site: &Site,
    rng: &mut ChaCha8Rng,
    feature: Feature,
    habitat: Habitat,
    ox: i32,
    oz: i32,
) {
    let (tries, spread) = feature.shape();
    // Most flower patches are small; now and then one grows into a full drift.
    let tries = match feature {
        Feature::Flowers(_) | Feature::TallFlowers(_) if rng.random_range(0..100) >= 12 => {
            tries * 2 / 5
        }
        _ => tries,
    };
    // Species for a flower patch: the drift's main one, plus one of the patch's own.
    let (main, second) = match feature {
        Feature::Flowers(palette) => {
            let mut drift = crate::deterministic_rng::coord_rng(
                ox.div_euclid(DRIFT_CELL),
                oz.div_euclid(DRIFT_CELL),
                SALT_DRIFT,
            );
            (
                pick_weighted(palette, drift.random_range(0..1000)),
                pick_weighted(palette, rng.random_range(0..1000)),
            )
        }
        _ => (GRASS, GRASS),
    };
    for _ in 0..tries {
        // Every try draws the same numbers whether or not it lands in this pass's
        // bounds, so a patch comes out the same from each tile it reaches into.
        let x = ox + rng.random_range(0..=spread) - rng.random_range(0..=spread);
        let z = oz + rng.random_range(0..=spread) - rng.random_range(0..=spread);
        let pick = rng.random_range(0..100u32);
        let variant = rng.random_range(0..1000u32);
        if !site.contains(x, z) || editor.surface_is_sealed(x, z) {
            continue;
        }
        let y = site.ground_y(editor, x, z);
        let stays_in_habitat = match feature {
            // Pads float on the pools, cane grows on whatever bank the water has.
            Feature::LilyPads => matches!(site.cover(x, z), LC_WETLAND | LC_MANGROVES | LC_WATER),
            Feature::SugarCane => site.habitat(x, z).is_some() || site.cover(x, z) == LC_BARE,
            _ => site.habitat(x, z).is_some_and(|h| h.same_ground(habitat)),
        };
        if !stays_in_habitat {
            continue;
        }
        match feature {
            Feature::Flowers(_) => {
                let flower = if pick < 85 { main } else { second };
                place_plant(editor, x, y, z, SOIL, flower);
            }
            Feature::TallFlowers(palette) => {
                place_tall_plant(editor, x, y, z, SOIL, pick_weighted(palette, variant));
            }
            Feature::TallGrass => {
                place_tall_plant(editor, x, y, z, SOIL, (TALL_GRASS_BOTTOM, TALL_GRASS_TOP));
            }
            Feature::Ferns => {
                if pick < 30 {
                    place_tall_plant(editor, x, y, z, SOIL, (LARGE_FERN_LOWER, LARGE_FERN_UPPER));
                } else {
                    place_plant(editor, x, y, z, SOIL, FERN);
                }
            }
            Feature::Mushrooms => {
                // Only under a canopy or overhang, where the game lets them live.
                if editor.highest_block_between(x, z, y + 3, y + 24).is_some() {
                    let mushroom = if pick < 70 {
                        BROWN_MUSHROOM
                    } else {
                        RED_MUSHROOM
                    };
                    place_plant(editor, x, y, z, SHADE_SOIL, mushroom);
                }
            }
            Feature::SugarCane => {
                if beside_water(editor, x, y, z) {
                    let height = 1 + (pick % 3) as i32;
                    place_stack(editor, x, y, z, CANE_SOIL, SUGAR_CANE, height, false);
                }
            }
            Feature::Pumpkins => place_plant(editor, x, y, z, &[GRASS_BLOCK], PUMPKIN),
            Feature::SweetBerries => place_plant(editor, x, y, z, SOIL, SWEET_BERRY_BUSH),
            Feature::DeadBushes => place_plant(editor, x, y, z, DRY_SOIL, DEAD_BUSH),
            Feature::Cactus => {
                let height = match pick % 4 {
                    0 => 1,
                    3 => 3,
                    _ => 2,
                };
                place_stack(editor, x, y, z, &[SAND], CACTUS, height, true);
            }
            Feature::LilyPads => {
                if editor.check_for_block_absolute(x, y, z, Some(&[WATER]), None)
                    && !editor.block_exists_absolute(x, y + 1, z)
                {
                    editor.set_block_absolute(LILY_PAD, x, y + 1, z, None, None);
                }
            }
            Feature::Moss => place_plant(editor, x, y, z, SOIL, MOSS_CARPET),
        }
    }
}

/// A free spot for a plant above `ground_y`: nothing there, or short grass it may
/// take over, and no wood right above it.
fn open_above(editor: &WorldEditor, x: i32, ground_y: i32, z: i32) -> bool {
    (!editor.block_exists_absolute(x, ground_y + 1, z)
        || editor.check_for_block_absolute(x, ground_y + 1, z, Some(&[GRASS]), None))
        && !editor.check_for_block_absolute(x, ground_y + 2, z, Some(WOOD), None)
}

fn place_plant(
    editor: &mut WorldEditor,
    x: i32,
    ground_y: i32,
    z: i32,
    soil: &[Block],
    plant: Block,
) {
    if editor.check_for_block_absolute(x, ground_y, z, Some(soil), None)
        && open_above(editor, x, ground_y, z)
    {
        editor.set_block_absolute(plant, x, ground_y + 1, z, Some(&[GRASS]), None);
    }
}

fn place_tall_plant(
    editor: &mut WorldEditor,
    x: i32,
    ground_y: i32,
    z: i32,
    soil: &[Block],
    (lower, upper): (Block, Block),
) {
    if editor.check_for_block_absolute(x, ground_y, z, Some(soil), None)
        && open_above(editor, x, ground_y, z)
        && !editor.block_exists_absolute(x, ground_y + 2, z)
    {
        editor.set_block_absolute(lower, x, ground_y + 1, z, Some(&[GRASS]), None);
        editor.set_block_absolute(upper, x, ground_y + 2, z, None, None);
    }
}

/// Stacks `height` of `plant`, stopping at the first occupied block. A cactus
/// also needs every side clear, or the game breaks it on the next update.
#[allow(clippy::too_many_arguments)]
fn place_stack(
    editor: &mut WorldEditor,
    x: i32,
    ground_y: i32,
    z: i32,
    soil: &[Block],
    plant: Block,
    height: i32,
    sides_clear: bool,
) {
    if !editor.check_for_block_absolute(x, ground_y, z, Some(soil), None)
        || !open_above(editor, x, ground_y, z)
    {
        return;
    }
    for y in ground_y + 1..=ground_y + height {
        let occupied = y > ground_y + 1 && editor.block_exists_absolute(x, y, z);
        let crowded = sides_clear
            && [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|&(dx, dz)| editor.block_exists_absolute(x + dx, y, z + dz));
        if occupied || crowded {
            break;
        }
        editor.set_block_absolute(plant, x, y, z, Some(&[GRASS]), None);
    }
}

/// Sugar cane needs water on a side of the block it stands on.
fn beside_water(editor: &WorldEditor, x: i32, ground_y: i32, z: i32) -> bool {
    [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dz)| {
        editor.check_for_block_absolute(x + dx, ground_y, z + dz, Some(&[WATER]), None)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn habitats_follow_cover_and_climate() {
        let t = Climate::Temperate;
        assert_eq!(
            habitat(LC_GRASSLAND, t, 48.0, false, None),
            Some(Habitat::Meadow)
        );
        assert_eq!(
            habitat(LC_GRASSLAND, t, 48.0, true, None),
            Some(Habitat::Alpine)
        );
        assert_eq!(
            habitat(LC_TREE_COVER, t, 48.0, false, None),
            Some(Habitat::Forest)
        );
        assert_eq!(
            habitat(LC_TREE_COVER, t, 62.0, false, None),
            Some(Habitat::Taiga)
        );
        assert_eq!(
            habitat(LC_TREE_COVER, t, 5.0, false, None),
            Some(Habitat::Jungle)
        );
        assert_eq!(
            habitat(LC_GRASSLAND, Climate::HotDesert, 25.0, false, None),
            Some(Habitat::Desert)
        );
        assert_eq!(
            habitat(LC_WETLAND, t, 48.0, false, None),
            Some(Habitat::Wetland)
        );
        assert_eq!(
            habitat(crate::land_cover::LC_BUILT_UP, t, 48.0, false, None),
            None
        );
        assert_eq!(
            habitat(crate::land_cover::LC_CROPLAND, t, 48.0, false, None),
            None
        );
    }

    #[test]
    fn ecoregions_refine_the_habitat() {
        use crate::ecoregion::lookup;
        let t = Climate::Temperate;
        // Humid Pampas, Eastern Mediterranean and the Serengeti.
        let pampas = lookup(576);
        assert_eq!(
            habitat(LC_GRASSLAND, t, 34.6, false, pampas),
            Some(Habitat::Prairie)
        );
        assert_eq!(
            habitat(LC_GRASSLAND, Climate::ColdSteppe, 34.6, false, pampas),
            Some(Habitat::Steppe)
        );
        assert_eq!(
            habitat(LC_SHRUBLAND, t, 32.0, false, lookup(791)),
            Some(Habitat::Maquis)
        );
        assert_eq!(
            habitat(LC_GRASSLAND, t, 2.8, false, lookup(54)),
            Some(Habitat::Savanna)
        );
        // City lawns in the Everglades ecoregion stay meadow; ESA marks the real marsh.
        assert_eq!(
            habitat(LC_GRASSLAND, t, 26.2, false, lookup(581)),
            Some(Habitat::Meadow)
        );
        // Ethiopian highland forest is no jungle, and alpine ground stays alpine.
        assert_eq!(
            habitat(LC_TREE_COVER, t, 9.0, false, lookup(79)),
            Some(Habitat::Forest)
        );
        assert_eq!(
            habitat(LC_GRASSLAND, t, 34.6, true, pampas),
            Some(Habitat::Alpine)
        );
        // Arid climates and unmapped cover keep the climate reading.
        assert_eq!(
            habitat(LC_GRASSLAND, Climate::HotDesert, 25.0, false, lookup(791)),
            Some(Habitat::Desert)
        );
        assert_eq!(
            habitat(crate::land_cover::LC_BUILT_UP, t, 48.0, false, pampas),
            None
        );
    }

    #[test]
    fn feature_tables_leave_room_for_nothing() {
        for h in [
            Habitat::Meadow,
            Habitat::Alpine,
            Habitat::Forest,
            Habitat::Taiga,
            Habitat::Jungle,
            Habitat::Shrub,
            Habitat::Steppe,
            Habitat::Desert,
            Habitat::Tundra,
            Habitat::Wetland,
            Habitat::Prairie,
            Habitat::Maquis,
            Habitat::Savanna,
        ] {
            let total: u32 = features(h).iter().map(|&(_, w)| w).sum();
            assert!(total < 1000, "{h:?} sums to {total}");
        }
    }

    #[test]
    fn patches_come_out_the_same_whatever_the_tiling() {
        use crate::coordinate_system::geographic::LLBBox;
        use crate::ground::test_support::ground_with_land_cover_and_elevation;
        use crate::land_cover::{LandCoverData, LC_WETLAND};
        use clap::Parser;
        use std::sync::Arc;

        let side = 96usize;
        let grid: Vec<Vec<u8>> = (0..side)
            .map(|z| {
                (0..side)
                    .map(|x| match (x < 40, z < 50) {
                        (true, _) => LC_GRASSLAND,
                        (false, true) => LC_TREE_COVER,
                        (false, false) => LC_WETLAND,
                    })
                    .collect()
            })
            .collect();
        let lc = LandCoverData {
            grid,
            water_distance: vec![vec![0u8; side]; side],
            water_blend_cache: once_cell::sync::OnceCell::new(),
            width: side,
            height: side,
            cells_per_meter: 1.0,
        };
        let ground = Arc::new(ground_with_land_cover_and_elevation(lc, side, side));
        let max = side as i32 - 1;
        let xzbbox = XZBBox::rect_from_min_max(0, 0, max, max).unwrap();
        let llbbox = LLBBox::new(48.0, 11.0, 48.01, 11.01).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().to_str().unwrap();
        let args = Args::parse_from([
            "arnis",
            "--output-dir",
            out,
            "--bbox",
            "48.0,11.0,48.01,11.01",
        ]);

        let run = |tiles: &[(i32, i32)]| -> Vec<Option<Block>> {
            let mut editor = WorldEditor::new(tmp.path().to_path_buf(), &xzbbox, llbbox);
            editor.set_ground(ground.clone());
            for x in 0..=max {
                for z in 0..=max {
                    editor.set_block_absolute(GRASS_BLOCK, x, 0, z, None, None);
                }
            }
            for &(min_x, max_x) in tiles {
                decorate_region(&mut editor, &ground, &args, &xzbbox, min_x, max_x, 0, max);
            }
            (0..=max)
                .flat_map(|x| (0..=max).flat_map(move |z| (1..=2).map(move |y| (x, y, z))))
                .map(|(x, y, z)| editor.get_block_absolute(x, y, z))
                .collect()
        };

        let whole = run(&[(0, max)]);
        let split = run(&[(0, 45), (46, max)]);
        assert!(
            whole.iter().filter(|b| b.is_some()).count() > 100,
            "patches should have grown"
        );
        assert!(whole == split, "a tile seam changed the plants");
    }

    #[test]
    fn every_stacked_plant_part_is_a_loose_plant() {
        for upper in STACKED_PLANT_PARTS {
            assert!(LOOSE_PLANTS.contains(upper));
        }
    }
}
