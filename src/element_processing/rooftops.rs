//! Flat roofs: the membrane they are covered in, the coping on their parapets
//! and the plant that stands on them. A New York roof is fitted out in rows
//! and blocks on the building's own grid: a stair
//! bulkhead and the water tank towards the ends, condensers and vents in lines
//! between, skylights down the middle, all kept a couple of blocks off the
//! parapet, rather than single blocks scattered over it. The region decides the
//! inventory: wooden water tanks are a North American fixture, European roofs
//! carry less, and more often gravel, moss or solar panels.

use crate::block_definitions::*;
use crate::deterministic_rng::element_rng;
use crate::element_processing::buildings::{
    cached_prop_block, facing_for_normal, roof_edge_distances, BuildingCategory, BuildingRegion,
    MaterialRegion, RoofDistanceGrid,
};
use crate::element_processing::connected_blocks::place_connected;
use crate::osm_parser::ArchEra;
use crate::world_editor::WorldEditor;
use rand::Rng;

/// What a flat roof is covered in: a field block with low-frequency patches of
/// a second tone and the odd rectangular repair, like a membrane laid in sheets
/// and patched over the years. The patches follow world coordinates, so the
/// parts of one building continue one roof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RoofMembrane {
    base: Block,
    patch: Block,
    repair: Block,
    /// Strip along the parapet: the bare margin of a green roof.
    margin: Option<Block>,
    seed: u64,
}

impl RoofMembrane {
    /// One block over the whole roof, for a mapped roof colour or material.
    pub(crate) fn uniform(block: Block) -> Self {
        Self {
            base: block,
            patch: block,
            repair: block,
            margin: None,
            seed: 0,
        }
    }

    pub(crate) fn block_at(&self, x: i32, z: i32) -> Block {
        if self.patch == self.base && self.repair == self.base {
            return self.base;
        }
        // Repairs are whole sheets of a coarse grid; tone patches are blobs.
        if hash2(
            self.seed ^ 0x7E9A_12C5_0000_0001,
            x.div_euclid(4),
            z.div_euclid(3),
        ) % 100
            < 4
        {
            return self.repair;
        }
        if value_noise(self.seed, x, z, 7) > 0.64 {
            self.patch
        } else {
            self.base
        }
    }

    /// The block for a cell `edge` blocks in from the roof edge.
    pub(crate) fn block_at_edge(&self, x: i32, z: i32, edge: i32) -> Block {
        match self.margin {
            Some(margin) if edge <= 1 => margin,
            _ => self.block_at(x, z),
        }
    }

    /// Planted (extensive green roof): it carries little plant.
    pub(crate) fn is_green(&self) -> bool {
        self.base == MOSS_BLOCK
    }

    /// Every block the membranes can lay, for the export mapping tests.
    #[cfg(test)]
    pub(crate) fn palette_blocks() -> Vec<Block> {
        MEMBRANES
            .iter()
            .flat_map(|family| family.iter())
            .flat_map(|&(a, b, c)| [a, b, c])
            .chain([MOSS_BLOCK, GRASS_BLOCK, COARSE_DIRT])
            .collect()
    }
}

/// (field, patch, repair) per membrane family. Light roofs dominate in New
/// York (white and light grey membranes), older
/// stock keeps more dark tar. European roofs are grey membrane, or gravel
/// ballast with moss and dirt where it has weathered (Berlin, Dresden).
/// Elsewhere the roof is mostly the bare slab: dusty sand-grey concrete in
/// the desert, monsoon-stained concrete under the odd blue tarpaulin in the
/// tropics, red-oxide or brick-tile terraces in Mexico, Argentina and
/// Taiwan, green-painted slabs in Korea.
const WHITE: usize = 0;
const LIGHT: usize = 1;
const GREY: usize = 2;
const DARK: usize = 3;
const BALLAST: usize = 4;
const EU_GREY: usize = 5;
const DUSTY: usize = 6;
const STAINED: usize = 7;
const CLAY: usize = 8;
const PAINTED_GREEN: usize = 9;
const MEMBRANES: [&[(Block, Block, Block)]; 10] = [
    &[
        (CALCITE, WHITE_WOOL, SMOOTH_STONE),
        (WHITE_CONCRETE, DIORITE, LIGHT_GRAY_CONCRETE),
        (WHITE_WOOL, CALCITE, POLISHED_ANDESITE),
    ],
    &[
        (SMOOTH_STONE, LIGHT_GRAY_CONCRETE, ANDESITE),
        (LIGHT_GRAY_CONCRETE, SMOOTH_STONE, POLISHED_ANDESITE),
    ],
    &[
        (GRAY_WOOL, GRAY_CONCRETE, POLISHED_BLACKSTONE),
        (POLISHED_ANDESITE, ANDESITE, GRAY_CONCRETE),
    ],
    &[
        (SMOOTH_BASALT, GRAY_WOOL, BLACK_CONCRETE),
        (GRAY_WOOL, SMOOTH_BASALT, POLISHED_BLACKSTONE),
    ],
    &[
        (ANDESITE, MOSS_BLOCK, COARSE_DIRT),
        (ANDESITE, TUFF, GRAY_CONCRETE),
        (GRAY_CONCRETE, COARSE_DIRT, MOSS_BLOCK),
    ],
    &[
        (GRAY_CONCRETE, LIGHT_GRAY_CONCRETE, SMOOTH_STONE),
        (LIGHT_GRAY_CONCRETE, SMOOTH_STONE, GRAY_CONCRETE),
        (SMOOTH_STONE, GRAY_WOOL, LIGHT_GRAY_CONCRETE),
    ],
    &[
        (MUSHROOM_STEM, SMOOTH_SANDSTONE, LIGHT_GRAY_CONCRETE),
        (LIGHT_GRAY_CONCRETE, MUSHROOM_STEM, SMOOTH_SANDSTONE),
        (SMOOTH_SANDSTONE, MUSHROOM_STEM, SMOOTH_STONE),
    ],
    &[
        (LIGHT_GRAY_CONCRETE, GRAY_CONCRETE, BLUE_WOOL),
        (GRAY_CONCRETE, CYAN_TERRACOTTA, LIGHT_GRAY_CONCRETE),
        (SMOOTH_STONE, GRAY_CONCRETE, CYAN_TERRACOTTA),
    ],
    &[
        (TERRACOTTA, RED_TERRACOTTA, LIGHT_GRAY_CONCRETE),
        (RED_TERRACOTTA, TERRACOTTA, SMOOTH_STONE),
    ],
    &[
        (WAXED_OXIDIZED_COPPER, CYAN_TERRACOTTA, LIGHT_GRAY_CONCRETE),
        (
            WAXED_OXIDIZED_COPPER,
            WAXED_OXIDIZED_COPPER,
            CYAN_TERRACOTTA,
        ),
    ],
];

/// Membrane families by share in percent, for the regions whose flat roofs
/// are mostly the bare or painted slab. None keeps the European and North
/// American rules.
fn regional_membrane_shares(materials: MaterialRegion) -> Option<&'static [(usize, u32)]> {
    use MaterialRegion as R;
    Some(match materials {
        // Soviet roofs are black roll bitumen.
        R::EastEurope => &[(DARK, 45), (GREY, 35), (LIGHT, 20)],
        R::CentralAsia => &[(DARK, 30), (GREY, 30), (DUSTY, 25), (LIGHT, 15)],
        R::MiddleEast => &[(DUSTY, 60), (LIGHT, 20), (WHITE, 15), (GREY, 5)],
        R::SouthAsia => &[(STAINED, 55), (LIGHT, 20), (GREY, 17), (CLAY, 8)],
        R::SoutheastAsia => &[(STAINED, 35), (GREY, 27), (LIGHT, 28), (CLAY, 10)],
        R::EastAsia => &[(LIGHT, 40), (GREY, 35), (STAINED, 15), (CLAY, 10)],
        R::Korea => &[(PAINTED_GREEN, 45), (GREY, 30), (LIGHT, 25)],
        R::Japan => &[(LIGHT, 50), (GREY, 40), (WHITE, 10)],
        R::SubSaharanAfrica => &[(LIGHT, 30), (GREY, 30), (DUSTY, 25), (STAINED, 15)],
        R::LatinAmerica => &[(CLAY, 30), (WHITE, 25), (LIGHT, 27), (GREY, 18)],
        R::SouthernCone => &[(CLAY, 20), (LIGHT, 35), (GREY, 30), (WHITE, 15)],
        R::Australasia => &[(LIGHT, 35), (GREY, 35), (WHITE, 30)],
        _ => return None,
    })
}

/// Membrane for a flat roof the tags leave open, by region and building. Drawn
/// from the shared style seed, so the parts of one building share one roof.
pub(crate) fn pick_membrane(
    region: BuildingRegion,
    materials: MaterialRegion,
    category: BuildingCategory,
    era: ArchEra,
    building_height: i32,
    style_seed: u64,
) -> RoofMembrane {
    let mut rng = element_rng(style_seed ^ 0x6D65_6D62_7261_6E65);
    let roll = rng.random_range(0u32..100);
    let industrial = matches!(
        category,
        BuildingCategory::Industrial | BuildingCategory::Warehouse
    );
    let regional = regional_membrane_shares(materials).filter(|_| !industrial);
    let family = if let Some(shares) = regional {
        let mut left = roll;
        shares
            .iter()
            .find(|&&(_, share)| {
                let hit = left < share;
                left = left.saturating_sub(share);
                hit
            })
            .map_or(GREY, |&(family, _)| family)
    } else if region == BuildingRegion::Europe {
        // Extensive green roofs are a modern European habit (a quarter of
        // the post-1990 roofs), on blocks low enough to be planted, never on
        // sheds and halls.
        let green_share = match era {
            _ if building_height > 30
                || matches!(
                    category,
                    BuildingCategory::Industrial
                        | BuildingCategory::Warehouse
                        | BuildingCategory::Garage
                        | BuildingCategory::Shed
                        | BuildingCategory::Greenhouse
                ) =>
            {
                0.0
            }
            ArchEra::Contemporary => 0.25,
            ArchEra::Unknown => 0.06,
            ArchEra::PostWarPanel => 0.06,
            _ => 0.03,
        };
        if rng.random_bool(green_share) {
            return RoofMembrane {
                // Moss and sedum in two greens, with the odd bare sheet of
                // substrate.
                base: MOSS_BLOCK,
                patch: GRASS_BLOCK,
                repair: COARSE_DIRT,
                margin: Some(COARSE_DIRT),
                seed: style_seed,
            };
        }
        match roll {
            0..50 => EU_GREY,
            50..80 => BALLAST,
            _ => GREY,
        }
    } else {
        let (white, light, grey) = if industrial {
            (50, 85, 95)
        } else {
            match era {
                ArchEra::HistoricOrnate | ArchEra::TraditionalPreWar => (20, 50, 75),
                ArchEra::Contemporary => (45, 85, 97),
                _ => (30, 65, 85),
            }
        };
        if roll < white {
            WHITE
        } else if roll < light {
            LIGHT
        } else if roll < grey {
            GREY
        } else {
            DARK
        }
    };
    let options = MEMBRANES[family];
    let (base, patch, repair) = options[rng.random_range(0..options.len())];
    RoofMembrane {
        base,
        patch,
        repair,
        margin: None,
        seed: style_seed,
    }
}

/// Coping for a parapet in `wall`: light stone over brick, quartz or diorite
/// over white stone, the wall's own family elsewhere, the accent's slab on glass.
pub(crate) fn coping_for(wall: Block, accent: Block, seed: u64) -> Block {
    let pick = |options: &[Block]| {
        options[(hash2(seed ^ 0xC0B1_4600_0000_0001, 0, 0) % options.len() as u64) as usize]
    };
    match wall {
        BRICK | MUD_BRICKS | TERRACOTTA | RED_TERRACOTTA | ORANGE_TERRACOTTA | BROWN_TERRACOTTA
        | GRANITE | POLISHED_GRANITE | BROWN_CONCRETE | NETHER_BRICK | RED_NETHER_BRICKS => {
            pick(&[
                SMOOTH_STONE_SLAB,
                SMOOTH_STONE_SLAB,
                STONE_BRICK_SLAB,
                QUARTZ_SLAB_TOP,
                CUT_SANDSTONE_SLAB,
            ])
        }
        QUARTZ_BLOCK | QUARTZ_BRICKS | SMOOTH_QUARTZ | WHITE_CONCRETE | WHITE_TERRACOTTA
        | CALCITE | DIORITE | POLISHED_DIORITE => {
            pick(&[SMOOTH_QUARTZ_SLAB, POLISHED_DIORITE_SLAB, QUARTZ_SLAB_TOP])
        }
        SANDSTONE | SMOOTH_SANDSTONE | END_STONE_BRICKS | YELLOW_TERRACOTTA => {
            pick(&[CUT_SANDSTONE_SLAB, SMOOTH_STONE_SLAB])
        }
        STONE_BRICKS
        | STONE
        | ANDESITE
        | POLISHED_ANDESITE
        | SMOOTH_STONE
        | LIGHT_GRAY_CONCRETE
        | COBBLESTONE
        | TUFF
        | LIGHT_GRAY_TERRACOTTA => {
            pick(&[SMOOTH_STONE_SLAB, STONE_BRICK_SLAB, POLISHED_ANDESITE_SLAB])
        }
        BLACKSTONE
        | POLISHED_BLACKSTONE
        | POLISHED_BLACKSTONE_BRICKS
        | DEEPSLATE
        | DEEPSLATE_BRICKS
        | POLISHED_DEEPSLATE
        | GRAY_CONCRETE
        | BLACK_CONCRETE
        | GRAY_TERRACOTTA
        | CYAN_TERRACOTTA
        | BLUE_TERRACOTTA => pick(&[
            POLISHED_BLACKSTONE_SLAB,
            SMOOTH_STONE_SLAB,
            DEEPSLATE_TILE_SLAB,
        ]),
        _ if is_glass(wall) && is_glass(accent) => SMOOTH_STONE_SLAB,
        _ if is_glass(wall) => get_slab_block_for_material(accent),
        _ => get_slab_block_for_material(wall),
    }
}

fn is_glass(block: Block) -> bool {
    block.name().contains("glass")
}

/// A bottom slab. Only the slab ids that default to the top half need the
/// state spelled out; the rest stay plain blocks, which store no properties.
pub(crate) fn bottom_slab(slab: Block) -> BlockWithProperties {
    if slab == OAK_SLAB_TOP || slab == QUARTZ_SLAB_TOP {
        cached_prop_block(slab, &[("type", "bottom")])
    } else {
        BlockWithProperties::simple(slab)
    }
}

/// Integer hash of a seed and a cell (splitmix64 finaliser).
pub(crate) fn hash2(seed: u64, a: i32, b: i32) -> u64 {
    let mut h = seed
        ^ u64::from(a as u32).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ u64::from(b as u32).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

/// Smooth value noise in 0..1 with features about `cell` blocks across.
fn value_noise(seed: u64, x: i32, z: i32, cell: i32) -> f64 {
    let (gx, gz) = (x.div_euclid(cell), z.div_euclid(cell));
    let fx = f64::from(x.rem_euclid(cell)) / f64::from(cell);
    let fz = f64::from(z.rem_euclid(cell)) / f64::from(cell);
    let corner = |i: i32, j: i32| (hash2(seed, gx + i, gz + j) >> 11) as f64 / (1u64 << 53) as f64;
    let ease = |t: f64| t * t * (3.0 - 2.0 * t);
    let (sx, sz) = (ease(fx), ease(fz));
    let near = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * sx;
    let far = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * sx;
    near + (far - near) * sz
}

/// A flat roof being fitted out: how far each cell lies from the roof edge,
/// and which cells a prop already holds.
pub(crate) struct RoofDeck {
    grid: RoofDistanceGrid,
    taken: Vec<bool>,
    /// The largest distance from the edge of any cell.
    deepest: i32,
    /// Absolute Y of the roof surface; props stand on it.
    surface_y: i32,
}

impl RoofDeck {
    pub(crate) fn new(cells: &[(i32, i32)], surface_y: i32) -> Option<Self> {
        if cells.is_empty() {
            return None;
        }
        let grid = roof_edge_distances(cells);
        let taken = vec![false; grid.dist.len()];
        let deepest = grid.dist.iter().copied().max().unwrap_or(-1);
        Some(Self {
            grid,
            taken,
            deepest,
            surface_y,
        })
    }

    fn index(&self, x: i32, z: i32) -> Option<usize> {
        let (lx, lz) = (x - self.grid.min_x, z - self.grid.min_z);
        (lx >= 0 && lz >= 0 && lx < self.grid.width && lz < self.grid.height)
            .then(|| (lz * self.grid.width + lx) as usize)
    }

    /// Blocks in from the roof edge, -1 off the roof.
    pub(crate) fn edge(&self, x: i32, z: i32) -> i32 {
        self.grid.get(x, z)
    }

    fn is_taken(&self, x: i32, z: i32) -> bool {
        self.index(x, z).is_some_and(|i| self.taken[i])
    }

    /// Marks a rectangle as held, so nothing else is placed on or against it.
    pub(crate) fn reserve(&mut self, x0: i32, z0: i32, w: i32, d: i32) {
        for z in z0..z0 + d {
            for x in x0..x0 + w {
                if let Some(i) = self.index(x, z) {
                    self.taken[i] = true;
                }
            }
        }
    }

    /// The rectangle lies `min_edge` or more in from the edge, on free cells,
    /// with a free cell all round it so props never touch.
    fn fits(&self, x0: i32, z0: i32, w: i32, d: i32, min_edge: i32) -> bool {
        for z in z0..z0 + d {
            for x in x0..x0 + w {
                if self.edge(x, z) < min_edge {
                    return false;
                }
            }
        }
        for z in z0 - 1..=z0 + d {
            for x in x0 - 1..=x0 + w {
                if self.is_taken(x, z) {
                    return false;
                }
            }
        }
        true
    }

    /// Origin of the free `w`×`d` rectangle whose centre lies nearest the
    /// target, searching outwards ring by ring up to `reach` blocks.
    fn find_near(
        &self,
        target: (i32, i32),
        w: i32,
        d: i32,
        min_edge: i32,
        reach: i32,
    ) -> Option<(i32, i32)> {
        // Reject what cannot fit anywhere before the search, which grows with
        // the square of `reach`: the inset bounds, or the roof's depth (a
        // `w`×`d` rectangle has a cell `min(w, d) / 2` further in than its rim).
        if w > self.grid.width - 2 * min_edge
            || d > self.grid.height - 2 * min_edge
            || self.deepest < min_edge + (w.min(d) - 1) / 2
        {
            return None;
        }
        let (ox, oz) = (target.0 - w / 2, target.1 - d / 2);
        for r in 0..=reach {
            for (dx, dz) in ring(r) {
                if self.fits(ox + dx, oz + dz, w, d, min_edge) {
                    return Some((ox + dx, oz + dz));
                }
            }
        }
        None
    }

    fn count_at_least(&self, min_edge: i32) -> usize {
        self.grid.dist.iter().filter(|&&d| d >= min_edge).count()
    }

    fn deepest(&self) -> i32 {
        self.deepest
    }

    fn cells_at_least(&self, min_edge: i32) -> Vec<(i32, i32)> {
        let mut cells = Vec::new();
        for lz in 0..self.grid.height {
            for lx in 0..self.grid.width {
                if self.grid.dist[(lz * self.grid.width + lx) as usize] >= min_edge {
                    cells.push((self.grid.min_x + lx, self.grid.min_z + lz));
                }
            }
        }
        cells
    }

    /// The roof's middle: its deepest cell, nearest the bounding box centre.
    fn heart(&self) -> (i32, i32) {
        let (cx2, cz2) = (
            2 * self.grid.min_x + self.grid.width - 1,
            2 * self.grid.min_z + self.grid.height - 1,
        );
        let deepest = self.deepest();
        self.cells_at_least(deepest)
            .into_iter()
            .min_by_key(|&(x, z)| (2 * x - cx2).pow(2) + (2 * z - cz2).pow(2))
            .unwrap_or((cx2 / 2, cz2 / 2))
    }
}

/// Offsets on the square ring `r` blocks out, in a fixed order.
fn ring(r: i32) -> impl Iterator<Item = (i32, i32)> {
    let rows = (-r..=r).flat_map(move |d| [(d, -r), (d, r)]);
    let sides = (-r + 1..r).flat_map(move |d| [(-r, d), (r, d)]);
    rows.chain(sides).take(if r == 0 { 1 } else { usize::MAX })
}

/// What the fit-out needs to know about the building under the roof.
pub(crate) struct RooftopBrief {
    pub region: BuildingRegion,
    pub category: BuildingCategory,
    pub era: ArchEra,
    /// Wall height in blocks.
    pub height: i32,
    pub floor_cycle: i32,
    pub wall: Block,
    pub scale: f64,
    /// Placement seed: the element id.
    pub seed: u64,
    /// Homes rather than offices, shops or plant.
    pub residential: bool,
    /// A whole building rather than one part of it: water tanks and plant
    /// rooms stand on the main roof only.
    pub whole_building: bool,
    /// Little decorative budget: half the small clutter.
    pub sparse: bool,
    /// A planted roof, which carries hardly any plant.
    pub green: bool,
}

impl RooftopBrief {
    fn scaled(&self, v: i32) -> i32 {
        ((f64::from(v) * self.scale).round() as i32).max(1)
    }

    fn storeys(&self) -> i32 {
        self.height / self.floor_cycle.max(3)
    }

    /// Plant-heavy uses: their roofs fill with condensers and air handlers.
    fn plant_heavy(&self) -> bool {
        !self.residential
            && !matches!(
                self.category,
                BuildingCategory::House | BuildingCategory::Historic | BuildingCategory::Farm
            )
    }
}

/// Fits out a flat roof with the plant a building of this kind and region
/// carries, laid in rows along the roof's long axis.
pub(crate) fn fit_out_roof(editor: &mut WorldEditor, deck: &mut RoofDeck, brief: &RooftopBrief) {
    let mut rng = element_rng(brief.seed ^ 0x2F0F_70F5_0000_0001);
    let usable = deck.count_at_least(2);
    if usable == 0 {
        return;
    }
    let na = brief.region == BuildingRegion::NorthAmerica;
    let europe = brief.region == BuildingRegion::Europe;
    // European and other roofs carry less than New York's; a planted roof or a
    // plain building carries less again.
    let mut density = match brief.region {
        BuildingRegion::NorthAmerica => 1.0,
        BuildingRegion::Europe => 0.5,
        BuildingRegion::Other => 0.8,
    };
    if brief.sparse {
        density *= 0.5;
    }
    if brief.green {
        density *= 0.3;
    }
    let count = |per_cells: f64, cap: usize| -> usize {
        ((usable as f64 / per_cells * density).round() as usize).min(cap)
    };

    let heart = deck.heart();
    let along_x = deck.grid.width >= deck.grid.height;
    let axis = if along_x { (1, 0) } else { (0, 1) };
    let across = (axis.1, axis.0);
    let (long, short) = if along_x {
        (deck.grid.width, deck.grid.height)
    } else {
        (deck.grid.height, deck.grid.width)
    };
    let flip = if rng.random_bool(0.5) { 1 } else { -1 };
    let step = |from: (i32, i32), dir: (i32, i32), n: i32| (from.0 + dir.0 * n, from.1 + dir.1 * n);
    let end_a = step(heart, axis, -flip * long / 4);
    let end_b = step(heart, axis, flip * long / 4);
    let side = (short / 4).max(2);
    // A rectangle `len` long along the axis and `wide` across it, as (w, d).
    let oriented = |len: i32, wide: i32| if along_x { (len, wide) } else { (wide, len) };
    let reach = long.max(short) / 2;

    // A plant room crowns a tower and takes in its stair and lift overruns.
    let tower = brief.whole_building && brief.height >= brief.scaled(36) && usable >= 80;
    let penthouse_share = match brief.region {
        BuildingRegion::NorthAmerica => 0.5,
        BuildingRegion::Europe => 0.3,
        BuildingRegion::Other => 0.4,
    };
    let penthouse =
        tower && rng.random_bool(penthouse_share) && place_penthouse(editor, deck, brief);

    // Stair and lift bulkheads from about four storeys, else a roof hatch.
    let mut bulkhead_top: Option<(i32, i32, i32)> = None;
    if !penthouse {
        let bulkhead_share = if na {
            0.9
        } else if europe {
            0.6
        } else {
            0.8
        };
        if brief.storeys() >= 4 && usable >= 24 && rng.random_bool(bulkhead_share) {
            let pair = usable >= 500 && rng.random_bool(0.5);
            let big = usable >= 400;
            let (len, wide) = if big { (5, 4) } else { (4, 3) };
            let (w, d) = oriented(brief.scaled(len), brief.scaled(wide));
            // A door is two blocks tall at any scale, under the cap.
            let h = brief.scaled(if brief.height >= 40 { 4 } else { 3 }).max(2);
            let wall = bulkhead_wall(brief.wall, &mut rng);
            let cap = if rng.random_bool(0.6) {
                POLISHED_BLACKSTONE_SLAB
            } else {
                SMOOTH_STONE_SLAB
            };
            let door = if rng.random_bool(0.5) {
                IRON_DOOR
            } else {
                DARK_OAK_DOOR_LOWER
            };
            for target in [end_a, end_b].into_iter().take(1 + usize::from(pair)) {
                if let Some((x0, z0)) = deck.find_near(target, w, d, 2, reach) {
                    // The door opens towards the middle of the roof.
                    let to_heart = (heart.0 - (x0 + w / 2), heart.1 - (z0 + d / 2));
                    let door_side = if along_x {
                        (if to_heart.0 >= 0 { 1 } else { -1 }, 0)
                    } else {
                        (0, if to_heart.1 >= 0 { 1 } else { -1 })
                    };
                    place_bulkhead(
                        editor,
                        deck,
                        (x0, z0),
                        (w, d, h),
                        wall,
                        cap,
                        door,
                        door_side,
                    );
                    bulkhead_top.get_or_insert((x0 + w / 2, z0 + d / 2, h + 1));
                }
            }
        } else if usable >= 6 && rng.random_bool(0.6) {
            if let Some((x, z)) = deck.find_near(end_a, 1, 1, 2, reach) {
                place_props(
                    editor,
                    cached_prop_block(
                        IRON_TRAPDOOR,
                        &[("facing", "north"), ("half", "bottom"), ("open", "false")],
                    ),
                    x,
                    deck.surface_y + 1,
                    z,
                );
                deck.reserve(x, z, 1, 1);
            }
        }
    }

    // Wooden water tanks: New York's mid-rises from about six storeys up,
    // mostly on pre-war stock, never on the glass towers.
    let tank_share = match brief.era {
        ArchEra::HistoricOrnate | ArchEra::TraditionalPreWar | ArchEra::Unknown => 0.45,
        ArchEra::PostWarPanel => 0.25,
        ArchEra::Contemporary => 0.06,
    };
    let tank_building = !matches!(
        brief.category,
        BuildingCategory::GlassySkyscraper
            | BuildingCategory::GlassCornerSkyscraper
            | BuildingCategory::GridSkyscraper
            | BuildingCategory::ContemporarySkyscraper
            | BuildingCategory::ModernSkyscraper
            | BuildingCategory::Religious
            | BuildingCategory::Hospital
            | BuildingCategory::School
            | BuildingCategory::House
            | BuildingCategory::Greenhouse
    );
    if na
        && brief.whole_building
        && !penthouse
        && tank_building
        && brief.storeys() >= 6
        && brief.height <= brief.scaled(160)
        && usable >= 20
        && rng.random_bool(tank_share)
    {
        // Sizes are picked unscaled and scaled once, so a bigger world scale
        // makes the tank proportionally bigger, not taller.
        let large = usable >= 90;
        let r = brief.scaled(if large { 2 } else { 1 });
        let size = 2 * r + 1;
        let target = if bulkhead_top.is_some() {
            end_b
        } else {
            step(heart, axis, flip * long / 6)
        };
        if let Some((x0, z0)) = deck.find_near(target, size, size, 2, reach) {
            let frame = brief.scaled(if large { 3 } else { 2 }) + rng.random_range(0..2);
            let barrel = brief.scaled(if large { 4 } else { 3 });
            place_water_tank(editor, deck, (x0 + r, z0 + r), r, frame, barrel);
        }
    }

    // Skylights down the middle; industrial halls get rows of them.
    let skylights = if matches!(
        brief.category,
        BuildingCategory::Industrial | BuildingCategory::Warehouse
    ) {
        count(120.0, 8)
    } else if usable >= 60 && rng.random_bool(0.4 * density.min(1.0)) {
        1 + usize::from(usable >= 200 && rng.random_bool(0.5))
    } else {
        0
    };
    let (sw, sd) = oriented(brief.scaled(4), brief.scaled(3));
    for i in 0..skylights as i32 {
        let along = if i % 2 == 0 {
            i / 2 * 6
        } else {
            -(i / 2 + 1) * 6
        };
        let target = step(heart, axis, along);
        if let Some((x0, z0)) = deck.find_near(target, sw, sd, 2, 3) {
            place_skylight(editor, deck, (x0, z0), (sw, sd));
        }
    }

    // Condensers and air handlers in a row along the axis to one side.
    let units = if brief.plant_heavy() {
        count(110.0, 8)
    } else {
        count(260.0, 3)
    };
    if units > 0 {
        let body = [IRON_BLOCK, LIGHT_GRAY_CONCRETE, SMOOTH_STONE][rng.random_range(0..3)];
        let tall = brief.plant_heavy() && rng.random_bool(0.35);
        let (len, wide) = (brief.scaled(3), brief.scaled(2));
        let (w, d) = oriented(len, wide);
        let row_start = step(
            step(heart, across, side),
            axis,
            -((units as i32).min(4) * (len + 1)) / 2,
        );
        lay_row(
            deck,
            row_start,
            axis,
            units,
            (w, d),
            len + 1,
            |deck, origin| {
                place_condenser(editor, deck, origin, (w, d), body, tall, &mut rng);
            },
        );
    }

    // Solar panel rows: modern European roofs, the odd new American one.
    let solar_share = match (brief.region, brief.era) {
        (BuildingRegion::Europe, ArchEra::Contemporary) => 0.45,
        (BuildingRegion::Europe, ArchEra::PostWarPanel | ArchEra::Unknown) => 0.18,
        (BuildingRegion::Europe, _) => 0.05,
        (_, ArchEra::Contemporary) if brief.storeys() <= 5 => 0.2,
        _ => 0.0,
    };
    if usable >= 40 && !brief.green && rng.random_bool(solar_share) {
        let rows = (usable / 60).clamp(2, 6) as i32;
        let len = (long / 3).clamp(3, 8);
        let (w, d) = oriented(len, 1);
        let anchor = step(heart, across, -side);
        for i in 0..rows {
            let target = step(anchor, across, -2 * i);
            if let Some((x0, z0)) = deck.find_near(target, w, d, 2, 2) {
                for z in z0..z0 + d {
                    for x in x0..x0 + w {
                        place(editor, DAYLIGHT_DETECTOR, x, deck.surface_y + 1, z);
                    }
                }
                deck.reserve(x0, z0, w, d);
            }
        }
    }

    // Small single units (exhaust fans, packaged units) in a short row.
    let small = count(if na { 90.0 } else { 160.0 }, 6);
    if small > 0 {
        let row_start = step(step(heart, across, -side), axis, (small as i32) * 2 / 2);
        lay_row(
            deck,
            row_start,
            (-axis.0, -axis.1),
            small,
            (1, 1),
            2,
            |deck, (x, z)| {
                place_small_unit(editor, deck, (x, z), &mut rng);
            },
        );
    }

    // Vent stacks and pipes: a line along the axis, the rest scattered.
    let vents = count(if na { 35.0 } else { 60.0 }, 12).max(usize::from(usable >= 8));
    if vents > 0 {
        let line = vents.div_ceil(2);
        let row_start = step(step(heart, across, -side - 2), axis, -(line as i32) * 3 / 2);
        let placed = lay_row(deck, row_start, axis, line, (1, 1), 3, |deck, (x, z)| {
            place_vent(editor, deck, (x, z), europe, &mut rng);
        });
        let (min_x, min_z) = (deck.grid.min_x, deck.grid.min_z);
        let (w, h) = (deck.grid.width, deck.grid.height);
        for _ in placed..vents {
            let target = (
                min_x + rng.random_range(0..w),
                min_z + rng.random_range(0..h),
            );
            if let Some((x, z)) = deck.find_near(target, 1, 1, 2, 3) {
                place_vent(editor, deck, (x, z), europe, &mut rng);
            }
        }
    }

    // A mast: radio on a tall building, a television aerial on a European home.
    let mast = if brief.height >= brief.scaled(40) && !penthouse {
        rng.random_bool(0.4)
    } else {
        europe && brief.residential && rng.random_bool(0.3)
    };
    if mast {
        let (x, z, y) = match bulkhead_top {
            Some((x, z, top)) => (x, z, deck.surface_y + top + 1),
            None => match deck.find_near(heart, 1, 1, 2, reach) {
                Some((x, z)) => {
                    deck.reserve(x, z, 1, 1);
                    (x, z, deck.surface_y + 1)
                }
                None => return,
            },
        };
        let height = brief.scaled(if brief.height >= 40 { 5 } else { 3 });
        place_mast(editor, x, y, z, height);
    }
}

/// Planted terrace on a residential tower's roof: a wooden deck round the
/// middle, planters along the railing and an aerial.
pub(crate) fn fit_out_terrace(editor: &mut WorldEditor, deck: &mut RoofDeck, brief: &RooftopBrief) {
    let heart = deck.heart();
    let surface = deck.surface_y;
    let y1 = surface + 1;
    for (x, z) in deck.cells_at_least(2) {
        let edge = deck.edge(x, z);
        if edge >= 3
            && (x - heart.0).abs() <= deck.grid.width / 4
            && (z - heart.1).abs() <= deck.grid.height / 4
        {
            // Deck boards over the membrane.
            editor.set_block_absolute(SPRUCE_PLANKS, x, surface, z, None, Some(&[]));
        } else if edge == 2 && hash2(brief.seed, x.div_euclid(3), z.div_euclid(3)).is_multiple_of(3)
        {
            // Planter troughs along the railing.
            place(editor, MOSS_BLOCK, x, y1, z);
            let leaves = if (x + z) % 2 == 0 {
                AZALEA_LEAVES
            } else {
                OAK_LEAVES
            };
            place(editor, leaves, x, y1 + 1, z);
            deck.reserve(x, z, 1, 1);
        }
    }
    if let Some((x, z)) = deck.find_near(heart, 1, 1, 2, 4) {
        place_mast(editor, x, y1, z, brief.scaled(5));
        deck.reserve(x, z, 1, 1);
    }
}

/// Lays up to `count` props in a row from `start` along `dir`, `pitch` apart,
/// nudging each up to two blocks to clear what is there. Returns how many fit.
fn lay_row(
    deck: &mut RoofDeck,
    start: (i32, i32),
    dir: (i32, i32),
    count: usize,
    (w, d): (i32, i32),
    pitch: i32,
    mut place_one: impl FnMut(&mut RoofDeck, (i32, i32)),
) -> usize {
    let mut placed = 0;
    let mut slot = 0;
    // Gaps let a row run round a bulkhead; past a few misses it has left the roof.
    let mut misses = 0;
    while placed < count && misses < 4 {
        let target = (
            start.0 + dir.0 * pitch * slot,
            start.1 + dir.1 * pitch * slot,
        );
        slot += 1;
        match deck.find_near(target, w, d, 2, 2) {
            Some(origin) => {
                place_one(deck, origin);
                placed += 1;
                misses = 0;
            }
            None => misses += 1,
        }
    }
    placed
}

fn place(editor: &mut WorldEditor, block: Block, x: i32, y: i32, z: i32) {
    editor.set_block_absolute(block, x, y, z, None, None);
}

fn place_props(editor: &mut WorldEditor, block: BlockWithProperties, x: i32, y: i32, z: i32) {
    editor.set_block_with_properties_absolute(block, x, y, z, None, None);
}

/// Bulkheads are clad like the building when it is masonry, else rendered grey.
fn bulkhead_wall(wall: Block, rng: &mut impl Rng) -> Block {
    if !is_glass(wall) && rng.random_bool(0.55) {
        wall
    } else {
        [LIGHT_GRAY_CONCRETE, SMOOTH_STONE, WHITE_CONCRETE][rng.random_range(0..3)]
    }
}

/// Stair or lift bulkhead: a hollow box in `wall` under a flat slab cap, a door
/// on the side facing `door_side` and a small window opposite.
#[allow(clippy::too_many_arguments)]
fn place_bulkhead(
    editor: &mut WorldEditor,
    deck: &mut RoofDeck,
    (x0, z0): (i32, i32),
    (w, d, h): (i32, i32, i32),
    wall: Block,
    cap: Block,
    door: Block,
    door_side: (i32, i32),
) {
    let y1 = deck.surface_y + 1;
    let cap = bottom_slab(cap);
    for z in z0..z0 + d {
        for x in x0..x0 + w {
            if x == x0 || x == x0 + w - 1 || z == z0 || z == z0 + d - 1 {
                for y in y1..y1 + h {
                    place(editor, wall, x, y, z);
                }
            }
            place_props(editor, cap.clone(), x, y1 + h, z);
        }
    }
    let middle_of = |(nx, nz): (i32, i32)| match (nx, nz) {
        (1, _) => (x0 + w - 1, z0 + d / 2),
        (-1, _) => (x0, z0 + d / 2),
        (_, 1) => (x0 + w / 2, z0 + d - 1),
        _ => (x0 + w / 2, z0),
    };
    let (dx, dz) = middle_of(door_side);
    let facing = facing_for_normal(door_side.0, door_side.1);
    for (half, y) in [("lower", y1), ("upper", y1 + 1)] {
        editor.set_block_with_properties_absolute(
            cached_prop_block(
                door,
                &[("half", half), ("facing", facing), ("hinge", "left")],
            ),
            dx,
            y,
            dz,
            None,
            Some(&[]),
        );
    }
    if h >= 3 {
        let (wx, wz) = middle_of((-door_side.0, -door_side.1));
        editor.set_block_absolute(GRAY_STAINED_GLASS, wx, y1 + 1, wz, None, Some(&[]));
    }
    deck.reserve(x0, z0, w, d);
    // Keep the landing in front of the door clear.
    deck.reserve(dx + door_side.0, dz + door_side.1, 1, 1);
}

/// Wooden water tank on a frame of dark posts: a round tub of hooped staves
/// (barrels) under a stepped conical spruce cap.
fn place_water_tank(
    editor: &mut WorldEditor,
    deck: &mut RoofDeck,
    (cx, cz): (i32, i32),
    r: i32,
    frame: i32,
    barrel_h: i32,
) {
    let inside = |dx: i32, dz: i32, r: i32| dx * dx + dz * dz <= r * r + r;
    let rim = |dx: i32, dz: i32, r: i32| {
        inside(dx, dz, r)
            && (!inside(dx + 1, dz, r)
                || !inside(dx - 1, dz, r)
                || !inside(dx, dz + 1, r)
                || !inside(dx, dz - 1, r))
    };
    let y1 = deck.surface_y + 1;

    let mut legs = vec![(r, 0), (-r, 0), (0, r), (0, -r)];
    if r == 1 {
        legs = vec![(1, 1), (-1, 1), (1, -1), (-1, -1)];
    } else if r >= 3 {
        let k = (f64::from(r) / std::f64::consts::SQRT_2).round() as i32;
        legs.extend([(k, k), (-k, k), (k, -k), (-k, -k)]);
    }
    for (dx, dz) in legs {
        for y in y1..y1 + frame {
            place(editor, DARK_OAK_FENCE, cx + dx, y, cz + dz);
        }
    }

    let tub = y1 + frame;
    let stave = cached_prop_block(BARREL, &[("facing", "up"), ("open", "false")]);
    for dz in -r..=r {
        for dx in -r..=r {
            if rim(dx, dz, r) {
                for y in tub..tub + barrel_h {
                    place_props(editor, stave.clone(), cx + dx, y, cz + dz);
                }
            } else if inside(dx, dz, r) {
                place(editor, SPRUCE_PLANKS, cx + dx, tub, cz + dz);
            }
        }
    }

    // Conical cap, one ring narrower per course, stairs climbing inwards.
    let top = tub + barrel_h;
    let slab = bottom_slab(SPRUCE_SLAB);
    for k in 0..=r {
        let rr = r - k;
        for dz in -rr..=rr {
            for dx in -rr..=rr {
                if !inside(dx, dz, rr) {
                    continue;
                }
                let (x, y, z) = (cx + dx, top + k, cz + dz);
                if rr == 0 || (rim(dx, dz, rr) && dx.abs() == dz.abs()) {
                    place_props(editor, slab.clone(), x, y, z);
                } else if !rim(dx, dz, rr) {
                    place(editor, SPRUCE_PLANKS, x, y, z);
                } else {
                    let facing = if dx.abs() > dz.abs() {
                        if dx > 0 {
                            "west"
                        } else {
                            "east"
                        }
                    } else if dz > 0 {
                        "north"
                    } else {
                        "south"
                    };
                    place_props(
                        editor,
                        cached_prop_block(SPRUCE_STAIRS, &[("facing", facing)]),
                        x,
                        y,
                        z,
                    );
                }
            }
        }
    }
    deck.reserve(cx - r, cz - r, 2 * r + 1, 2 * r + 1);
}

/// Condenser or air handler: a box with fan grilles on top.
fn place_condenser(
    editor: &mut WorldEditor,
    deck: &mut RoofDeck,
    (x0, z0): (i32, i32),
    (w, d): (i32, i32),
    body: Block,
    tall: bool,
    rng: &mut impl Rng,
) {
    let y1 = deck.surface_y + 1;
    let body_h = if tall { 2 } else { 1 };
    let grille = rng.random_bool(0.6);
    for z in z0..z0 + d {
        for x in x0..x0 + w {
            for y in y1..y1 + body_h {
                place(editor, body, x, y, z);
            }
            if grille {
                place_props(
                    editor,
                    cached_prop_block(
                        IRON_TRAPDOOR,
                        &[("facing", "north"), ("half", "bottom"), ("open", "false")],
                    ),
                    x,
                    y1 + body_h,
                    z,
                );
            } else {
                place(editor, DAYLIGHT_DETECTOR, x, y1 + body_h, z);
            }
        }
    }
    deck.reserve(x0, z0, w, d);
}

/// One-block unit: exhaust fan, packaged unit, drum or grinder-like blower.
fn place_small_unit(
    editor: &mut WorldEditor,
    deck: &mut RoofDeck,
    (x, z): (i32, i32),
    rng: &mut impl Rng,
) {
    let y1 = deck.surface_y + 1;
    let facing = ["north", "south", "east", "west"][rng.random_range(0..4)];
    match rng.random_range(0..6) {
        0 => place_props(
            editor,
            cached_prop_block(BLAST_FURNACE, &[("facing", facing)]),
            x,
            y1,
            z,
        ),
        1 => place_props(
            editor,
            cached_prop_block(SMOKER, &[("facing", facing)]),
            x,
            y1,
            z,
        ),
        2 => place_props(
            editor,
            cached_prop_block(GRINDSTONE, &[("face", "floor"), ("facing", facing)]),
            x,
            y1,
            z,
        ),
        3 => place_props(
            editor,
            cached_prop_block(BARREL, &[("facing", "up"), ("open", "false")]),
            x,
            y1,
            z,
        ),
        4 => {
            place(editor, IRON_BLOCK, x, y1, z);
            place_props(
                editor,
                cached_prop_block(
                    IRON_TRAPDOOR,
                    &[("facing", facing), ("half", "bottom"), ("open", "false")],
                ),
                x,
                y1 + 1,
                z,
            );
        }
        _ => {
            place(editor, LIGHT_GRAY_CONCRETE, x, y1, z);
            place(editor, DAYLIGHT_DETECTOR, x, y1 + 1, z);
        }
    }
    deck.reserve(x, z, 1, 1);
}

/// Vent stack, plumbing vent, mushroom vent or drain cap. European roofs keep
/// to stacks and thin pipes.
fn place_vent(
    editor: &mut WorldEditor,
    deck: &mut RoofDeck,
    (x, z): (i32, i32),
    pipes_only: bool,
    rng: &mut impl Rng,
) {
    let y1 = deck.surface_y + 1;
    match rng.random_range(0..if pipes_only { 3 } else { 6 }) {
        0 => {
            let stack = [ANDESITE_WALL, STONE_BRICK_WALL, COBBLESTONE_WALL][rng.random_range(0..3)];
            for y in y1..y1 + rng.random_range(1..=2) {
                place(editor, stack, x, y, z);
            }
        }
        1 => place(editor, LIGHTNING_ROD, x, y1, z),
        2 => place(editor, END_ROD, x, y1, z),
        3 => {
            place(editor, IRON_BARS, x, y1, z);
            place_props(editor, bottom_slab(SMOOTH_STONE_SLAB), x, y1 + 1, z);
        }
        4 => place(editor, CAULDRON, x, y1, z),
        _ => place(editor, STONE_PRESSURE_PLATE, x, y1, z),
    }
    deck.reserve(x, z, 1, 1);
}

/// Flat skylight: glazing let into the membrane inside a slab curb.
fn place_skylight(
    editor: &mut WorldEditor,
    deck: &mut RoofDeck,
    (x0, z0): (i32, i32),
    (w, d): (i32, i32),
) {
    let curb = bottom_slab(SMOOTH_STONE_SLAB);
    for z in z0..z0 + d {
        for x in x0..x0 + w {
            if x == x0 || x == x0 + w - 1 || z == z0 || z == z0 + d - 1 {
                place_props(editor, curb.clone(), x, deck.surface_y + 1, z);
            } else {
                editor.set_block_absolute(GLASS, x, deck.surface_y, z, None, Some(&[]));
            }
        }
    }
    deck.reserve(x0, z0, w, d);
}

/// Radio mast or aerial: a thin steel pole with a rod on top.
fn place_mast(editor: &mut WorldEditor, x: i32, y: i32, z: i32, height: i32) {
    for dy in 0..height {
        place(editor, IRON_BARS, x, y + dy, z);
    }
    place(editor, LIGHTNING_ROD, x, y + height, z);
}

/// Mechanical penthouse on a tower: a plant room set back from the parapet
/// over 20-45% of the roof, one or two storeys of rendered wall with a band of
/// louvres, under a flat cap. Returns false when the roof has no room for one.
fn place_penthouse(editor: &mut WorldEditor, deck: &mut RoofDeck, brief: &RooftopBrief) -> bool {
    let total = deck.count_at_least(0);
    let deepest = deck.deepest();
    // Set well back, so it reads as plant on the roof rather than one more
    // storey of the building's own outline.
    let mut inset = 3;
    while inset < deepest && deck.count_at_least(inset) * 20 > total * 9 {
        inset += 1;
    }
    let cells = deck.cells_at_least(inset);
    if cells.len() * 100 < total * 20 || cells.len() < 36 {
        return false;
    }
    // Never built over a reservation (a hospital's helipad) or against one.
    if cells.iter().any(|&(x, z)| !deck.fits(x, z, 1, 1, inset)) {
        return false;
    }
    let mut rng = element_rng(brief.seed ^ 0x9E47_0A5E_0000_0001);
    let storeys = if brief.height >= brief.scaled(120) && cells.len() >= 300 && rng.random_bool(0.5)
    {
        2
    } else {
        1
    };
    let h = brief.floor_cycle.max(3) * storeys;
    let wall = if is_glass(brief.wall) || rng.random_bool(0.5) {
        [LIGHT_GRAY_CONCRETE, GRAY_CONCRETE, SMOOTH_STONE][rng.random_range(0..3)]
    } else {
        brief.wall
    };
    let roof = [GRAY_CONCRETE, POLISHED_ANDESITE, SMOOTH_STONE][rng.random_range(0..3)];
    let y1 = deck.surface_y + 1;
    let coping = bottom_slab(SMOOTH_STONE_SLAB);
    for &(x, z) in &cells {
        let open = [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .find(|&(dx, dz)| deck.edge(x + dx, z + dz) < inset);
        if let Some((ox, oz)) = open {
            let u = if ox != 0 { z } else { x };
            // Louvres in three of every six columns, in the courses below the top.
            let louvre_bay = (2..=4).contains(&u.rem_euclid(6));
            for k in 0..h {
                let y = y1 + k;
                if louvre_bay && h >= 4 && (h - 3..h - 1).contains(&k) {
                    place_connected(editor, IRON_BARS, x, y, z);
                    place(editor, BLACK_CONCRETE, x - ox, y, z - oz);
                } else {
                    place(editor, wall, x, y, z);
                }
            }
            place_props(editor, coping.clone(), x, y1 + h + 1, z);
        }
        place(editor, roof, x, y1 + h, z);
    }
    for &(x, z) in &cells {
        deck.reserve(x, z, 1, 1);
    }
    // Aerials on the plant room roof.
    if rng.random_bool(0.6) {
        let heart = deck.heart();
        place_mast(editor, heart.0, y1 + h + 1, heart.1, brief.scaled(6));
    }
    true
}

/// Every block the parapet copings and the prop catalogue place, for the
/// export mapping tests.
#[cfg(test)]
pub(crate) const PROP_BLOCKS: &[Block] = &[
    IRON_TRAPDOOR,
    DAYLIGHT_DETECTOR,
    BARREL,
    SPRUCE_STAIRS,
    SPRUCE_SLAB,
    SPRUCE_PLANKS,
    DARK_OAK_FENCE,
    IRON_BARS,
    IRON_BLOCK,
    END_ROD,
    LIGHTNING_ROD,
    GRINDSTONE,
    BLAST_FURNACE,
    SMOKER,
    IRON_DOOR,
    DARK_OAK_DOOR_LOWER,
    ANDESITE_WALL,
    STONE_BRICK_WALL,
    COBBLESTONE_WALL,
    STONE_PRESSURE_PLATE,
    CAULDRON,
    GLASS,
    GRAY_STAINED_GLASS,
    LIGHT_GRAY_CONCRETE,
    GRAY_CONCRETE,
    SMOOTH_STONE,
    WHITE_CONCRETE,
    POLISHED_ANDESITE,
    BLACK_CONCRETE,
    MOSS_BLOCK,
    AZALEA_LEAVES,
    OAK_LEAVES,
    SMOOTH_STONE_SLAB,
    STONE_BRICK_SLAB,
    QUARTZ_SLAB_TOP,
    CUT_SANDSTONE_SLAB,
    SMOOTH_QUARTZ_SLAB,
    POLISHED_DIORITE_SLAB,
    POLISHED_ANDESITE_SLAB,
    POLISHED_BLACKSTONE_SLAB,
    DEEPSLATE_TILE_SLAB,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_roofs_follow_the_region() {
        let draw = |region, materials, seed| {
            pick_membrane(
                region,
                materials,
                BuildingCategory::Residential,
                ArchEra::Unknown,
                20,
                seed,
            )
        };
        let mut green = 0;
        for seed in 0..200u64 {
            let korea = draw(BuildingRegion::Other, MaterialRegion::Korea, seed);
            green += usize::from(korea.base == WAXED_OXIDIZED_COPPER);
            // Desert roofs are pale slab, never tar or moss.
            let dubai = draw(BuildingRegion::Other, MaterialRegion::MiddleEast, seed);
            assert_ne!(dubai.base, SMOOTH_BASALT);
            assert!(!dubai.is_green());
            // Europe and New York are untouched by the regional shares.
            assert_eq!(
                draw(BuildingRegion::Europe, MaterialRegion::Central, seed),
                draw(BuildingRegion::Europe, MaterialRegion::Other, seed)
            );
            assert_eq!(
                draw(
                    BuildingRegion::NorthAmerica,
                    MaterialRegion::NorthAmerica,
                    seed
                ),
                draw(BuildingRegion::NorthAmerica, MaterialRegion::Other, seed)
            );
        }
        assert!((60..=130).contains(&green), "{green}");
    }

    #[test]
    fn membrane_patches_are_low_frequency() {
        let m = RoofMembrane {
            base: SMOOTH_STONE,
            patch: LIGHT_GRAY_CONCRETE,
            repair: ANDESITE,
            margin: None,
            seed: 99,
        };
        let (mut changes, mut cells, mut patched) = (0, 0, 0);
        for z in 0..60 {
            for x in 0..60 {
                let b = m.block_at(x, z);
                cells += 1;
                patched += usize::from(b != SMOOTH_STONE);
                changes += usize::from(b != m.block_at(x + 1, z));
            }
        }
        // Patches cover a minority of the roof in blobs, not salt noise.
        assert!(
            patched * 100 > cells * 5 && patched * 100 < cells * 40,
            "{patched}/{cells}"
        );
        assert!(
            changes * 100 < cells * 15,
            "{changes} tone changes in {cells} cells"
        );
        assert_eq!(RoofMembrane::uniform(GRAY_WOOL).block_at(5, 7), GRAY_WOOL);
    }

    #[test]
    fn deck_search_keeps_off_the_edge_and_apart() {
        let cells: Vec<(i32, i32)> = (0..16).flat_map(|x| (0..10).map(move |z| (x, z))).collect();
        let mut deck = RoofDeck::new(&cells, 10).unwrap();
        let first = deck.find_near((8, 5), 3, 3, 2, 10).unwrap();
        assert!(deck.fits(first.0, first.1, 3, 3, 2));
        deck.reserve(first.0, first.1, 3, 3);
        let second = deck.find_near((8, 5), 3, 3, 2, 10).unwrap();
        // A free cell between the two, and both two in from the edge.
        let apart = second.0 >= first.0 + 4
            || second.0 + 3 < first.0
            || second.1 >= first.1 + 4
            || second.1 + 3 < first.1;
        assert!(apart, "{first:?} {second:?}");
        for (x0, z0) in [first, second] {
            assert!(x0 >= 2 && z0 >= 2 && x0 + 3 <= 14 && z0 + 3 <= 8);
        }
    }

    #[test]
    fn deck_search_rejects_props_that_cannot_fit() {
        // A long strip only six wide: a 5×4 prop cannot stand two in from
        // its edge, however far the search reaches.
        let strip: Vec<(i32, i32)> = (0..2000)
            .flat_map(|x| (0..6).map(move |z| (x, z)))
            .collect();
        let deck = RoofDeck::new(&strip, 10).unwrap();
        assert_eq!(deck.find_near((1000, 3), 5, 4, 2, 1000), None);
        assert!(deck.find_near((1000, 3), 2, 2, 2, 1000).is_some());
        // A diagonal band: a wide bounding box, but no deep cell.
        let band: Vec<(i32, i32)> = (0..60)
            .flat_map(|x| (0..5).map(move |k| (x, x + k)))
            .collect();
        let deck = RoofDeck::new(&band, 10).unwrap();
        assert_eq!(deck.find_near((30, 32), 3, 3, 2, 60), None);
    }

    #[test]
    fn plant_rooms_keep_off_reserved_cells() {
        use crate::coordinate_system::cartesian::XZBBox;
        use crate::element_processing::building_test_support::test_editor;
        let brief = RooftopBrief {
            region: BuildingRegion::Europe,
            category: BuildingCategory::Hospital,
            era: ArchEra::Unknown,
            height: 60,
            floor_cycle: 4,
            wall: WHITE_CONCRETE,
            scale: 1.0,
            seed: 7,
            residential: false,
            whole_building: true,
            sparse: false,
            green: false,
        };
        let cells: Vec<(i32, i32)> = (0..40).flat_map(|x| (0..40).map(move |z| (x, z))).collect();
        let xzbbox = XZBBox::rect_from_xz_lengths(50.0, 50.0).unwrap();

        let mut editor = test_editor(&xzbbox);
        let mut deck = RoofDeck::new(&cells, 10).unwrap();
        assert!(place_penthouse(&mut editor, &mut deck, &brief));

        // A helipad reserved in the middle turns the plant room down.
        let mut editor = test_editor(&xzbbox);
        let mut deck = RoofDeck::new(&cells, 10).unwrap();
        deck.reserve(17, 17, 7, 7);
        assert!(!place_penthouse(&mut editor, &mut deck, &brief));
        for (x, z) in [(20, 20), (17, 17), (23, 23)] {
            assert!(editor.get_block_absolute(x, 11 + 4, z).is_none());
        }
    }
}
