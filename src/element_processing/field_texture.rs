//! Farmland plots: one crop per field, laid out inside its OSM outline; sizes in metres.

use crate::block_definitions::*;
use crate::clipping::is_invented_node_id;
use crate::ground_generation::value_noise_01;
use crate::land_cover::coord_hash;
use crate::osm_parser::ProcessedNode;
use crate::world_editor::WorldEditor;
use std::cell::Cell;

/// Farmland style, matching the GUI's segmented control.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum FieldPreset {
    /// One sheet of mixed crops per field (the previous look)
    Classic,
    /// Narrow strips with the full crop variety
    Smallholding,
    /// Mixed fields of medium width
    #[default]
    Patchwork,
    /// Wide, wheat-led fields
    Prairie,
}

impl FieldPreset {
    /// Parse the GUI's segment value, falling back to the default.
    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "classic" => FieldPreset::Classic,
            "smallholding" => FieldPreset::Smallholding,
            "prairie" => FieldPreset::Prairie,
            _ => FieldPreset::default(),
        }
    }

    fn params(self) -> Option<Params> {
        // Weights in CROPS order: wheat, potatoes, carrots, beetroots, sunflowers, pumpkins, fallow.
        let (strip_m, length_m, weights) = match self {
            FieldPreset::Classic => return None,
            FieldPreset::Smallholding => ((20.0, 45.0), 200.0, [22, 18, 18, 14, 10, 10, 8]),
            FieldPreset::Patchwork => ((45.0, 100.0), 400.0, [40, 15, 15, 8, 10, 5, 7]),
            FieldPreset::Prairie => ((120.0, 250.0), 1000.0, [60, 8, 6, 4, 12, 2, 8]),
        };
        Some(Params {
            strip_m,
            length_m,
            weights,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Crop {
    Wheat,
    Potatoes,
    Carrots,
    Beetroots,
    Sunflowers,
    Pumpkins,
    Fallow,
}

const CROPS: [Crop; 7] = [
    Crop::Wheat,
    Crop::Potatoes,
    Crop::Carrots,
    Crop::Beetroots,
    Crop::Sunflowers,
    Crop::Pumpkins,
    Crop::Fallow,
];

const TILLED: [Crop; 4] = [Crop::Wheat, Crop::Potatoes, Crop::Carrots, Crop::Beetroots];

// Below this a strip or plot stops reading as a field.
const MIN_STRIP_BLOCKS: f64 = 4.0;
const MIN_LENGTH_BLOCKS: f64 = 12.0;
const TEXTURE_NOISE: i32 = 6;

#[derive(Clone, Copy)]
struct Params {
    strip_m: (f64, f64),
    length_m: f64,
    weights: [u32; 7],
}

impl Params {
    fn strip_blocks(&self, scale: f64, roll: u64) -> f64 {
        let t = (roll % 1000) as f64 / 1000.0;
        ((self.strip_m.0 + t * (self.strip_m.1 - self.strip_m.0)) * scale).max(MIN_STRIP_BLOCKS)
    }

    fn plot(&self, key: u64, rows_along_x: bool) -> Plot {
        let total: u32 = self.weights.iter().sum();
        let mut roll = (mix(key) % u64::from(total)) as u32;
        let mut crop = Crop::Wheat;
        for (&w, &c) in self.weights.iter().zip(CROPS.iter()) {
            if roll < w {
                crop = c;
                break;
            }
            roll -= w;
        }
        Plot {
            crop,
            // Mostly ripe, with some fields sown later.
            young: mix(key ^ 0xA9E3).is_multiple_of(4),
            seed: mix(key ^ 0xF10E),
            rows_along_x,
        }
    }
}

fn mix(mut h: u64) -> u64 {
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

fn plot_key(a: u64, i: i64, j: i64) -> u64 {
    mix(a ^ mix((i as u64) ^ mix(j as u64 ^ 0x51ED)))
}

fn sane_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// One field: its crop, growth stage and row direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plot {
    pub crop: Crop,
    young: bool,
    seed: u64,
    // Rows follow the world axis nearest the strip, so they rasterise as straight lines.
    rows_along_x: bool,
}

/// What grows on a plot cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cover {
    Single(Block),
    Tall(Block, Block),
}

/// Plants `cover` on the cell above `ground_y`; a tall plant only where both halves fit.
pub fn place_cover(editor: &mut WorldEditor, cover: Cover, x: i32, ground_y: i32, z: i32) {
    match cover {
        Cover::Single(block) => editor.set_block_absolute(block, x, ground_y + 1, z, None, None),
        Cover::Tall(lower, upper) => {
            if !editor.block_exists_absolute(x, ground_y + 1, z)
                && !editor.block_exists_absolute(x, ground_y + 2, z)
            {
                editor.set_block_absolute(lower, x, ground_y + 1, z, None, None);
                editor.set_block_absolute(upper, x, ground_y + 2, z, None, None);
            }
        }
    }
}

impl Plot {
    fn on_row(&self, x: i32, z: i32) -> bool {
        (if self.rows_along_x { z } else { x }).rem_euclid(2) == 0
    }

    // Untilled plots avoid dirt and bare farmland, which revert once the world is played.
    pub fn surface(&self, x: i32, z: i32) -> Block {
        let n = value_noise_01(x, z, TEXTURE_NOISE);
        match self.crop {
            Crop::Wheat | Crop::Potatoes | Crop::Carrots | Crop::Beetroots => {
                if n < 0.04 {
                    COARSE_DIRT
                } else {
                    FARMLAND
                }
            }
            Crop::Sunflowers => {
                if n < 0.16 {
                    GRASS_BLOCK
                } else {
                    COARSE_DIRT
                }
            }
            Crop::Pumpkins => {
                if n < 0.42 {
                    COARSE_DIRT
                } else {
                    GRASS_BLOCK
                }
            }
            Crop::Fallow => {
                if n < 0.33 {
                    COARSE_DIRT
                } else if n < 0.45 {
                    MUD
                } else if n < 0.70 {
                    GRASS_BLOCK
                } else {
                    COARSE_DIRT
                }
            }
        }
    }

    /// Plant for a cell whose ground is the block `surface()` gave for it.
    pub fn cover(&self, surface: Block, x: i32, z: i32) -> Option<Cover> {
        let roll = coord_hash(x ^ 0x0C20_9A11, z) % 100;
        let single = |b| Some(Cover::Single(b));
        match self.crop {
            Crop::Wheat | Crop::Potatoes | Crop::Carrots | Crop::Beetroots => {
                if surface == FARMLAND {
                    single(self.crop_block(x, z, roll))
                } else {
                    match roll {
                        0..=2 if self.crop == Crop::Wheat => single(HAY_BALE),
                        3..=7 => single(DEAD_BUSH),
                        8..=20 => single(GRASS),
                        _ => None,
                    }
                }
            }
            Crop::Sunflowers => match surface {
                COARSE_DIRT if roll < 85 && self.on_row(x, z) => {
                    Some(Cover::Tall(SUNFLOWER_LOWER, SUNFLOWER_UPPER))
                }
                GRASS_BLOCK if roll < 25 => single(GRASS),
                _ => None,
            },
            Crop::Pumpkins => match (surface, roll) {
                (GRASS_BLOCK, 0..=5) => single(PUMPKIN),
                (GRASS_BLOCK, 6..=25) => single(GRASS),
                (GRASS_BLOCK, 26) => single(DEAD_BUSH),
                (COARSE_DIRT, 0..=7) => single(GRASS),
                _ => None,
            },
            Crop::Fallow => match (surface, roll) {
                (GRASS_BLOCK, 0..=19) => single(GRASS),
                (GRASS_BLOCK, _) | (MUD, _) => None,
                (_, 0..=5) => single(DEAD_BUSH),
                (_, 6..=14) => single(GRASS),
                (_, 15..=16) => single(FERN),
                _ => None,
            },
        }
    }

    fn crop_block(&self, x: i32, z: i32, roll: u64) -> Block {
        // Small patches where another crop came up.
        let crop = if value_noise_01(x + 321, z - 777, 4) < 0.022 {
            TILLED[(self.seed >> 9) as usize % TILLED.len()]
        } else {
            self.crop
        };
        // Some cells of a ripe field lag behind.
        let young = self.young || roll < 15;
        match (crop, young) {
            (Crop::Wheat, false) => WHEAT,
            (Crop::Wheat, true) => WHEAT_YOUNG,
            (Crop::Potatoes, false) => POTATOES,
            (Crop::Potatoes, true) => POTATOES_YOUNG,
            (Crop::Carrots, false) => CARROTS,
            (Crop::Carrots, true) => CARROTS_YOUNG,
            (_, false) => BEETROOTS,
            (_, true) => BEETROOTS_YOUNG,
        }
    }
}

/// Strips along an OSM field outline's long axis, cut across where it runs long.
pub struct FieldLayout {
    params: Params,
    id: u64,
    across: (f64, f64),
    along: (f64, f64),
    across_min: f64,
    along_min: f64,
    strip: f64,
    length: f64,
    n_across: i64,
    n_along: i64,
    rows_along_x: bool,
    // Neighbouring cells nearly always share a strip and a plot.
    last_strip: Cell<Option<(i64, f64)>>,
    last_plot: Cell<Option<((i64, i64), Plot)>>,
}

impl FieldLayout {
    /// None under the classic preset.
    pub fn new(preset: FieldPreset, scale: f64, id: u64, nodes: &[ProcessedNode]) -> Option<Self> {
        let params = preset.params()?;
        let scale = sane_scale(scale);
        // Edge directions folded by 4x, so parallel and perpendicular edges vote together.
        // Edges the clipper made along the generation bounds get no vote.
        let (mut c, mut s) = (0.0f64, 0.0f64);
        for w in nodes.windows(2) {
            if is_invented_node_id(w[0].id) && is_invented_node_id(w[1].id) {
                continue;
            }
            let (dx, dz) = ((w[1].x - w[0].x) as f64, (w[1].z - w[0].z) as f64);
            let len = dx.hypot(dz);
            if len >= 1.0 {
                let a = 4.0 * dz.atan2(dx);
                c += len * a.cos();
                s += len * a.sin();
            }
        }
        let (sin, cos) = (s.atan2(c) / 4.0).sin_cos();
        let (u, v) = ((cos, sin), (-sin, cos));
        let extent = |d: (f64, f64)| {
            nodes.iter().fold((f64::MAX, f64::MIN), |(lo, hi), n| {
                let p = d.0 * n.x as f64 + d.1 * n.z as f64;
                (lo.min(p), hi.max(p))
            })
        };
        let (eu, ev) = (extent(u), extent(v));
        if eu.0 > eu.1 {
            return None;
        }
        let (along, across, ea, ec) = if eu.1 - eu.0 >= ev.1 - ev.0 {
            (u, v, eu, ev)
        } else {
            (v, u, ev, eu)
        };
        let width = params.strip_blocks(scale, mix(id ^ 0x5717_1D7A));
        let length = (params.length_m * scale).max(MIN_LENGTH_BLOCKS);
        let n_across = (((ec.1 - ec.0) / width).round() as i64).max(1);
        let n_along = (((ea.1 - ea.0) / length).round() as i64).max(1);
        Some(Self {
            params,
            id,
            across,
            along,
            across_min: ec.0,
            along_min: ea.0,
            strip: ((ec.1 - ec.0) / n_across as f64).max(1.0),
            length: ((ea.1 - ea.0) / n_along as f64).max(1.0),
            n_across,
            n_along,
            rows_along_x: along.0.abs() >= along.1.abs(),
            last_strip: Cell::new(None),
            last_plot: Cell::new(None),
        })
    }

    pub fn plot_at(&self, x: i32, z: i32) -> Plot {
        let (fx, fz) = (x as f64, z as f64);
        let a = self.across.0 * fx + self.across.1 * fz - self.across_min;
        let l = self.along.0 * fx + self.along.1 * fz - self.along_min;
        let i = ((a / self.strip).floor() as i64).clamp(0, self.n_across - 1);
        let j = if self.n_along > 1 {
            let shift = match self.last_strip.get() {
                Some((strip, shift)) if strip == i => shift,
                _ => {
                    // Each strip is cut at its own offset, so long fields brick, not checker.
                    let t = (mix(self.id ^ (i as u64).wrapping_mul(0x9E37)) % 1000) as f64;
                    let shift = (0.3 + 0.4 * t / 1000.0) * self.length;
                    self.last_strip.set(Some((i, shift)));
                    shift
                }
            };
            (((l + shift) / self.length).floor() as i64).clamp(0, self.n_along)
        } else {
            0
        };
        if let Some((key, plot)) = self.last_plot.get() {
            if key == (i, j) {
                return plot;
            }
        }
        let plot = self.params.plot(plot_key(self.id, i, j), self.rows_along_x);
        self.last_plot.set(Some(((i, j), plot)));
        plot
    }
}

/// World-anchored blocks of parallel strips, for land-cover cropland without an outline.
pub struct CroplandGrid {
    params: Params,
    scale: f64,
    block: i32,
    last_block: Cell<Option<StripBlock>>,
    last_plot: Cell<Option<((u64, i32), Plot)>>,
}

#[derive(Clone, Copy)]
struct StripBlock {
    key: (i32, i32),
    hash: u64,
    rows_along_x: bool,
    width: i32,
    phase: i32,
}

impl CroplandGrid {
    /// None under the classic preset.
    pub fn new(preset: FieldPreset, scale: f64) -> Option<Self> {
        let params = preset.params()?;
        let scale = sane_scale(scale);
        let block = (params.length_m * scale).max(MIN_LENGTH_BLOCKS).round() as i32;
        Some(Self {
            params,
            scale,
            block,
            last_block: Cell::new(None),
            last_plot: Cell::new(None),
        })
    }

    pub fn plot_at(&self, x: i32, z: i32) -> Plot {
        let key = (x.div_euclid(self.block), z.div_euclid(self.block));
        let b = match self.last_block.get() {
            Some(b) if b.key == key => b,
            _ => {
                let hash = plot_key(0xC0FF_EE11, i64::from(key.0), i64::from(key.1));
                let width = self.params.strip_blocks(self.scale, hash >> 8).round() as i32;
                let b = StripBlock {
                    key,
                    hash,
                    rows_along_x: hash & 1 == 0,
                    width,
                    phase: ((hash >> 32) % width as u64) as i32,
                };
                self.last_block.set(Some(b));
                b
            }
        };
        let across = if b.rows_along_x { z } else { x };
        let strip = (across + b.phase).div_euclid(b.width);
        if let Some((k, plot)) = self.last_plot.get() {
            if k == (b.hash, strip) {
                return plot;
            }
        }
        let plot = self
            .params
            .plot(plot_key(b.hash, i64::from(strip), 0), b.rows_along_x);
        self.last_plot.set(Some(((b.hash, strip), plot)));
        plot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    // Closed rectangle `w` across and `l` long, centred at (cx, cz), long side at `deg`.
    fn rect(cx: f64, cz: f64, w: f64, l: f64, deg: f64) -> Vec<ProcessedNode> {
        let (s, c) = deg.to_radians().sin_cos();
        [(-l, -w), (l, -w), (l, w), (-l, w), (-l, -w)]
            .iter()
            .map(|&(a, b)| ProcessedNode {
                id: 0,
                tags: HashMap::new(),
                x: (cx + (a * c - b * s) / 2.0).round() as i32,
                z: (cz + (a * s + b * c) / 2.0).round() as i32,
            })
            .collect()
    }

    fn plot_count(layout: &FieldLayout, nodes: &[ProcessedNode]) -> usize {
        let xs = nodes.iter().map(|n| n.x);
        let zs = nodes.iter().map(|n| n.z);
        let (x0, x1) = (xs.clone().min().unwrap(), xs.max().unwrap());
        let (z0, z1) = (zs.clone().min().unwrap(), zs.max().unwrap());
        let mut seeds = HashSet::new();
        for x in x0..=x1 {
            for z in z0..=z1 {
                seeds.insert(layout.plot_at(x, z).seed);
            }
        }
        seeds.len()
    }

    #[test]
    fn classic_lays_out_nothing() {
        let nodes = rect(0.0, 0.0, 50.0, 90.0, 0.0);
        assert!(FieldLayout::new(FieldPreset::Classic, 1.0, 1, &nodes).is_none());
        assert!(CroplandGrid::new(FieldPreset::Classic, 1.0).is_none());
    }

    #[test]
    fn a_field_narrower_than_a_strip_is_one_plot() {
        let nodes = rect(500.0, -200.0, 30.0, 140.0, 20.0);
        let layout = FieldLayout::new(FieldPreset::Patchwork, 1.0, 7, &nodes).unwrap();
        assert_eq!(plot_count(&layout, &nodes), 1);
    }

    #[test]
    fn a_large_field_splits_into_strips_along_its_long_side() {
        let nodes = rect(0.0, 0.0, 400.0, 900.0, 30.0);
        let layout = FieldLayout::new(FieldPreset::Patchwork, 1.0, 42, &nodes).unwrap();
        assert!(
            layout.n_across >= 4,
            "400 m across should hold several strips"
        );
        assert_eq!(layout.n_along, 2);
        let along = layout
            .along
            .1
            .atan2(layout.along.0)
            .to_degrees()
            .rem_euclid(180.0);
        assert!((along - 30.0).abs() < 1.0, "strips run at {along} degrees");
    }

    #[test]
    fn neighbouring_strips_are_cut_at_different_points() {
        let nodes = rect(0.0, 0.0, 400.0, 1600.0, 0.0);
        let layout = FieldLayout::new(FieldPreset::Patchwork, 1.0, 11, &nodes).unwrap();
        let cuts = |z: i32| {
            (-799..800)
                .filter(|&x| layout.plot_at(x, z).seed != layout.plot_at(x - 1, z).seed)
                .collect::<Vec<_>>()
        };
        let (a, b) = (cuts(-180), cuts(-180 + layout.strip.ceil() as i32));
        assert!(!a.is_empty() && !b.is_empty());
        assert_ne!(a, b, "strips share their cuts");
    }

    #[test]
    fn a_clipped_edge_does_not_turn_the_strips() {
        // A 900 x 400 field at 20 degrees, cut by the generation bounds along x = 0.
        let node = |id, x, z| ProcessedNode {
            id,
            tags: HashMap::new(),
            x,
            z,
        };
        let cut = 1u64 << 62;
        let nodes = [
            node(1, -354, -342),
            node(cut, 0, -213),
            node(cut + 1, 0, 213),
            node(2, -491, 34),
            node(1, -354, -342),
        ];
        let layout = FieldLayout::new(FieldPreset::Patchwork, 1.0, 5, &nodes).unwrap();
        let along = layout
            .along
            .1
            .atan2(layout.along.0)
            .to_degrees()
            .rem_euclid(90.0);
        assert!(
            (along - 20.0).abs() < 1.0,
            "strips turned to {along} degrees"
        );
    }

    #[test]
    fn plot_sizes_follow_the_map_scale() {
        // The same 300 x 800 m field at two map scales splits into the same plots.
        let one = rect(0.0, 0.0, 300.0, 800.0, 10.0);
        let two = rect(0.0, 0.0, 600.0, 1600.0, 10.0);
        for preset in [
            FieldPreset::Smallholding,
            FieldPreset::Patchwork,
            FieldPreset::Prairie,
        ] {
            let a = FieldLayout::new(preset, 1.0, 3, &one).unwrap();
            let b = FieldLayout::new(preset, 2.0, 3, &two).unwrap();
            assert_eq!(
                (a.n_across, a.n_along),
                (b.n_across, b.n_along),
                "{preset:?}"
            );
        }
        let a = CroplandGrid::new(FieldPreset::Patchwork, 1.0).unwrap();
        let b = CroplandGrid::new(FieldPreset::Patchwork, 2.0).unwrap();
        assert_eq!(b.block, a.block * 2);
    }

    #[test]
    fn presets_grow_wider_fields() {
        let nodes = rect(0.0, 0.0, 600.0, 1200.0, 0.0);
        let strips = |p| FieldLayout::new(p, 1.0, 9, &nodes).unwrap().n_across;
        assert!(strips(FieldPreset::Smallholding) > strips(FieldPreset::Patchwork));
        assert!(strips(FieldPreset::Patchwork) > strips(FieldPreset::Prairie));
    }

    #[test]
    fn crop_shares_follow_the_preset() {
        let share = |preset: FieldPreset| {
            let p = preset.params().unwrap();
            let wheat = (0..20_000)
                .filter(|&k| p.plot(mix(k), true).crop == Crop::Wheat)
                .count();
            wheat as f64 / 20_000.0
        };
        assert!(share(FieldPreset::Prairie) > 0.5);
        assert!(share(FieldPreset::Smallholding) < 0.3);
    }

    #[test]
    fn every_crop_appears() {
        let p = FieldPreset::Patchwork.params().unwrap();
        let seen: HashSet<_> = (0..5_000).map(|k| p.plot(mix(k), true).crop).collect();
        assert_eq!(seen.len(), CROPS.len());
    }

    #[test]
    fn cropland_strips_are_whole_fields_and_world_anchored() {
        let grid = CroplandGrid::new(FieldPreset::Patchwork, 1.0).unwrap();
        let mut cells: HashMap<u64, usize> = HashMap::new();
        for x in -700..700 {
            for z in -700..700 {
                *cells.entry(grid.plot_at(x, z).seed).or_default() += 1;
            }
        }
        assert!(cells.len() > 20);
        // Only strips cut by the sampled square may be small.
        assert!(cells.values().filter(|&&n| n < 45 * 45).count() < cells.len() / 2);
        let again = CroplandGrid::new(FieldPreset::Patchwork, 1.0).unwrap();
        assert_eq!(grid.plot_at(-3210, 4567), again.plot_at(-3210, 4567));
    }

    #[test]
    fn tilled_and_fallow_blocks_keep_one_byte_sections() {
        let blocks = [
            FARMLAND,
            COARSE_DIRT,
            GRASS_BLOCK,
            MUD,
            WHEAT,
            WHEAT_YOUNG,
            CARROTS,
            CARROTS_YOUNG,
            POTATOES,
            POTATOES_YOUNG,
            BEETROOTS,
            BEETROOTS_YOUNG,
            HAY_BALE,
            DEAD_BUSH,
            GRASS,
            FERN,
        ];
        for b in blocks {
            assert!(b.id() < 256, "{} has id {}", b.name(), b.id());
        }
    }

    #[test]
    fn sunflowers_stand_in_straight_world_rows() {
        let p = Plot {
            crop: Crop::Sunflowers,
            young: false,
            seed: 1,
            rows_along_x: true,
        };
        let tall = |x, z| matches!(p.cover(COARSE_DIRT, x, z), Some(Cover::Tall(..)));
        let planted: usize = (0..200).filter(|&x| tall(x, 10)).count();
        assert!(planted > 150, "row 10 holds only {planted} sunflowers");
        assert!(
            (0..200).all(|x| !tall(x, 11)),
            "a sunflower stands between rows"
        );
    }

    #[test]
    fn tilled_ground_grows_crops_and_bare_ground_none() {
        let mut p = FieldPreset::Patchwork.params().unwrap().plot(5, true);
        p.crop = Crop::Carrots;
        let crops = ["wheat", "carrots", "potatoes", "beetroots"];
        for x in 0..50 {
            match p.cover(FARMLAND, x, 7) {
                Some(Cover::Single(b)) => assert!(crops.contains(&b.name()), "{}", b.name()),
                other => panic!("farmland grew {other:?}"),
            }
        }
        p.crop = Crop::Fallow;
        assert_eq!(p.cover(MUD, 3, 4), None);
    }
}
