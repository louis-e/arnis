//! RESOLVE Ecoregions 2017 (Dinerstein et al., CC BY 4.0), built by `assets/climate/build_grids.py`.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use rayon::prelude::*;

use crate::coordinate_system::cartesian::XZPoint;
use crate::geo_grid::TiledGrid;

static GRID_BYTES: &[u8] = include_bytes!("../assets/climate/ecoregions.grid");
static TABLE: &str = include_str!("../assets/climate/ecoregions.tsv");
static GRID: LazyLock<Option<TiledGrid>> = LazyLock::new(|| TiledGrid::parse(GRID_BYTES));

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EcoBiome {
    MoistTropical,
    DryTropical,
    TropicalConifer,
    TemperateBroadleaf,
    TemperateConifer,
    Boreal,
    TropicalGrassland,
    TemperateGrassland,
    Flooded,
    MontaneGrassland,
    Tundra,
    Mediterranean,
    Desert,
    Mangroves,
}

impl EcoBiome {
    fn from_num(n: &str) -> Option<Self> {
        use EcoBiome::*;
        Some(match n {
            "1" => MoistTropical,
            "2" => DryTropical,
            "3" => TropicalConifer,
            "4" => TemperateBroadleaf,
            "5" => TemperateConifer,
            "6" => Boreal,
            "7" => TropicalGrassland,
            "8" => TemperateGrassland,
            "9" => Flooded,
            "10" => MontaneGrassland,
            "11" => Tundra,
            "12" => Mediterranean,
            "13" => Desert,
            "14" => Mangroves,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Realm {
    Afrotropic,
    Antarctica,
    Australasia,
    Indomalayan,
    Nearctic,
    Neotropic,
    Oceania,
    Palearctic,
}

impl Realm {
    fn from_code(code: &str) -> Option<Self> {
        use Realm::*;
        Some(match code {
            "AT" => Afrotropic,
            "AN" => Antarctica,
            "AA" => Australasia,
            "IM" => Indomalayan,
            "NA" => Nearctic,
            "NT" => Neotropic,
            "OC" => Oceania,
            "PA" => Palearctic,
            _ => return None,
        })
    }

    /// Cacti are native to the Americas only.
    pub fn is_americas(self) -> bool {
        matches!(self, Realm::Nearctic | Realm::Neotropic)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ecoregion {
    pub id: u16,
    pub biome: EcoBiome,
    pub realm: Realm,
}

struct Row {
    biome: EcoBiome,
    realm: Realm,
    pack: &'static str,
    mix: &'static str,
    name: &'static str,
}

static ROWS: LazyLock<Vec<Option<Row>>> = LazyLock::new(|| {
    let mut rows: Vec<Option<Row>> = Vec::new();
    for line in TABLE.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let (Some(id), Some(biome), Some(realm)) = (
            f.first().and_then(|v| v.parse::<usize>().ok()),
            f.get(1).and_then(|v| EcoBiome::from_num(v)),
            f.get(2).and_then(|v| Realm::from_code(v)),
        ) else {
            continue;
        };
        if rows.len() <= id {
            rows.resize_with(id + 1, || None);
        }
        rows[id] = Some(Row {
            biome,
            realm,
            pack: f.get(3).copied().unwrap_or_default(),
            mix: f.get(4).copied().unwrap_or_default(),
            name: f.get(5).copied().unwrap_or_default(),
        });
    }
    rows
});

fn row(id: u16) -> Option<&'static Row> {
    ROWS.get(id as usize)?.as_ref()
}

pub fn lookup(id: u16) -> Option<Ecoregion> {
    row(id).map(|r| Ecoregion {
        id,
        biome: r.biome,
        realm: r.realm,
    })
}

pub fn name(id: u16) -> Option<&'static str> {
    row(id).map(|r| r.name)
}

/// Tree pack and community mix for an ecoregion; `None` where no trees are mapped.
pub fn tree_mix(id: u16) -> Option<(&'static str, &'static str)> {
    row(id)
        .filter(|r| !r.pack.is_empty())
        .map(|r| (r.pack, r.mix))
}

/// Whether palms grow here, natively or as the usual street and garden tree.
pub fn palms_belong(eco: Ecoregion, abs_lat: f64) -> bool {
    use EcoBiome::*;
    match eco.biome {
        MoistTropical | DryTropical | TropicalConifer | TropicalGrassland | Mangroves => true,
        Boreal | MontaneGrassland | Tundra => false,
        Mediterranean => abs_lat <= 45.0,
        // Nikau and cabbage-tree palms reach New Zealand and Victoria.
        TemperateBroadleaf if eco.realm == Realm::Australasia => abs_lat <= 42.0,
        _ => abs_lat <= 35.0,
    }
}

/// Smallest and largest map cell, in blocks.
const MIN_CELL: i32 = 4;
const MAX_CELL: i32 = 32;
/// Cells per side before the cell grows past `MAX_CELL`.
const MAX_CELLS_PER_SIDE: usize = 512;

/// Ecoregion ids over one run's ground, a cell per `cell` x `cell` blocks.
pub struct EcoMap {
    cell: i32,
    origin: (i32, i32),
    w: usize,
    h: usize,
    ids: Vec<u16>,
}

impl EcoMap {
    /// `geo` maps a ground coordinate to (lat, lon); `align` is the world block at ground (0, 0).
    pub fn build(
        world_w: usize,
        world_h: usize,
        align: (i32, i32),
        geo: impl Fn(f64, f64) -> (f64, f64) + Sync,
    ) -> Option<Self> {
        let grid = GRID.as_ref()?;
        if world_w == 0 || world_h == 0 {
            return None;
        }
        // A quarter source cell, as a power of two so neighbouring runs agree on it.
        let (lat0, _) = geo(0.0, 0.0);
        let (lat1, _) = geo(0.0, world_h as f64);
        let source_cells = ((lat0 - lat1).abs() * grid.cells_per_degree()).max(1e-6);
        let per_source = world_h as f64 / source_cells;
        let quarter = ((per_source / 4.0) as i32).clamp(MIN_CELL, MAX_CELL);
        let cell = (1 << (31 - quarter.leading_zeros()))
            .max(world_w.max(world_h).div_ceil(MAX_CELLS_PER_SIDE) as i32);
        let origin = (-align.0.rem_euclid(cell), -align.1.rem_euclid(cell));
        let span = |len: usize, from: i32| ((len as i32 - from + cell - 1) / cell) as usize;
        let (w, h) = (span(world_w, origin.0), span(world_h, origin.1));
        let half = f64::from(cell) / 2.0;
        let cells: Vec<(usize, usize)> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let gx = f64::from(origin.0 + (i % w) as i32 * cell) + half;
                let gz = f64::from(origin.1 + (i / w) as i32 * cell) + half;
                let (lat, lon) = geo(gx, gz);
                warped_cell(grid, lat, lon)
            })
            .collect();
        let tiles: BTreeSet<usize> = cells.iter().map(|&c| grid.tile_of(c)).collect();
        let decoded: HashMap<usize, Vec<u8>> = tiles
            .into_par_iter()
            .filter_map(|t| grid.decode(t).map(|d| (t, d)))
            .collect();
        let ids = cells
            .par_iter()
            .map(|&c| {
                decoded
                    .get(&grid.tile_of(c))
                    .map_or(0, |t| grid.value(t, grid.index_in_tile(c)))
            })
            .collect();
        Some(Self {
            cell,
            origin,
            w,
            h,
            ids,
        })
    }

    fn id_at(&self, x: i32, z: i32) -> u16 {
        let kx = (x - self.origin.0)
            .div_euclid(self.cell)
            .clamp(0, self.w as i32 - 1);
        let kz = (z - self.origin.1)
            .div_euclid(self.cell)
            .clamp(0, self.h as i32 - 1);
        self.ids[kz as usize * self.w + kx as usize]
    }

    pub fn at(&self, coord: XZPoint) -> Option<Ecoregion> {
        lookup(self.id_at(coord.x, coord.z))
    }

    /// The ecoregions present, most cells first.
    pub fn by_area(&self) -> Vec<(u16, usize)> {
        let mut counts: HashMap<u16, usize> = HashMap::new();
        for &id in &self.ids {
            *counts.entry(id).or_default() += 1;
        }
        let mut out: Vec<(u16, usize)> = counts.into_iter().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }

    /// Whether some cell has no ecoregion (open sea, ice).
    pub fn has_gaps(&self) -> bool {
        self.ids.iter().any(|&id| lookup(id).is_none())
    }

    /// This map over a transformed ground; `source` maps new ground coordinates to old.
    pub fn resample(
        &self,
        world_w: usize,
        world_h: usize,
        source: impl Fn(i32, i32) -> (i32, i32) + Sync,
    ) -> Self {
        let cell = self.cell;
        let (w, h) = (
            world_w.div_ceil(cell as usize).max(1),
            world_h.div_ceil(cell as usize).max(1),
        );
        let ids = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, z) = source(
                    (i % w) as i32 * cell + cell / 2,
                    (i / w) as i32 * cell + cell / 2,
                );
                self.id_at(x, z)
            })
            .collect();
        Self {
            cell,
            origin: (0, 0),
            w,
            h,
            ids,
        }
    }
}

fn warped_cell(grid: &TiledGrid, lat: f64, lon: f64) -> (usize, usize) {
    let (col, row) = grid.position(lat, lon);
    let (dc, dr) = warp(col, row);
    grid.cell(col + dc, row + dr)
}

/// Geographic noise offset in source cells, so borders wander instead of tracing raster steps.
fn warp(col: f64, row: f64) -> (f64, f64) {
    const SUB: f64 = 64.0;
    let (x, z) = ((col * SUB).floor() as i32, (row * SUB).floor() as i32);
    let octave = |salt: u32, period: i32, amp: f64| {
        (crate::ground_generation::value_noise_salted(x, z, period, salt) - 0.5) * 2.0 * amp
    };
    (
        octave(0xEC0_0001, 384, 0.9) + octave(0xEC0_0002, 96, 0.35) + octave(0xEC0_0005, 16, 0.12),
        octave(0xEC0_0003, 384, 0.9) + octave(0xEC0_0004, 96, 0.35) + octave(0xEC0_0006, 16, 0.12),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(lat: f64, lon: f64) -> Option<Ecoregion> {
        let map = EcoMap::build(64, 64, (0, 0), |_, _| (lat, lon))?;
        map.at(XZPoint::new(32, 32))
    }

    #[test]
    fn table_covers_every_ecoregion() {
        let rows = ROWS.iter().flatten().count();
        assert_eq!(rows, 846);
        let pampas = lookup(576).unwrap();
        assert_eq!(pampas.biome, EcoBiome::TemperateGrassland);
        assert_eq!(pampas.realm, Realm::Neotropic);
        assert_eq!(name(576), Some("Humid Pampas"));
        assert_eq!(tree_mix(576).map(|m| m.0), Some("sam"));
        // Antarctica has no trees to map.
        assert!(tree_mix(117).is_none());
    }

    #[test]
    fn real_places_resolve() {
        let cases = [
            (-34.60, -58.38, "Humid Pampas"),
            (52.52, 13.40, "Central European mixed forests"),
            (
                32.08,
                34.78,
                "Eastern Mediterranean conifer-broadleaf forests",
            ),
            (19.43, -99.13, "Central Mexican matorral"),
            (35.68, 139.69, "Taiheiyo evergreen forests"),
        ];
        for (lat, lon, want) in cases {
            let eco = at(lat, lon).unwrap_or_else(|| panic!("{want}: no ecoregion"));
            assert_eq!(name(eco.id), Some(want));
        }
        // Coastal cells were filled, the open sea was not.
        assert!(at(64.15, -21.94).is_some(), "Reykjavik");
        assert!(at(0.0, -30.0).is_none(), "mid-Atlantic");
    }

    #[test]
    fn warp_stays_within_a_cell_or_so() {
        for i in 0..2000 {
            let (dc, dr) = warp(f64::from(i) * 0.37, f64::from(i) * 0.61);
            assert!(dc.abs() <= 1.4 && dr.abs() <= 1.4);
        }
    }

    #[test]
    fn map_cells_follow_the_scale_and_align_across_runs() {
        // About 1850 blocks per source cell (1:1 scale): the cell caps at 32 blocks.
        let per_block = 1.0 / 60.0 / 1850.0;
        let geo = |_: f64, gz: f64| (10.0 - gz * per_block, 20.0);
        let map = EcoMap::build(1000, 1000, (70, -7), geo).unwrap();
        assert_eq!(map.cell, 32);
        assert_eq!(map.origin, (-6, -25));
        // At 1:100 a source cell is 18.5 blocks, so the cell shrinks with it.
        let geo = |_: f64, gz: f64| (10.0 - gz * per_block * 100.0, 20.0);
        assert_eq!(EcoMap::build(1000, 1000, (0, 0), geo).unwrap().cell, 4);
    }

    #[test]
    fn palms_follow_biome_and_latitude() {
        let eco = |biome, realm| Ecoregion {
            id: 1,
            biome,
            realm,
        };
        let med = eco(EcoBiome::Mediterranean, Realm::Palearctic);
        assert!(palms_belong(med, 41.9), "Rome");
        let temperate = eco(EcoBiome::TemperateBroadleaf, Realm::Nearctic);
        assert!(!palms_belong(temperate, 40.7), "New York");
        let nz = eco(EcoBiome::TemperateBroadleaf, Realm::Australasia);
        assert!(palms_belong(nz, 36.8), "Auckland");
        let tibet = eco(EcoBiome::MontaneGrassland, Realm::Palearctic);
        assert!(!palms_belong(tibet, 29.6), "Lhasa");
    }
}
