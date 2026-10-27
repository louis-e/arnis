//! One DH `FullData` row: column blobs, id mapping and block-state strings.

use fastnbt::Value;
use fnv::FnvHashMap;
use std::ops::Range;

use crate::world_editor::common::PaletteItem;

/// Columns per side of a section, at every detail level.
pub(crate) const WIDTH: usize = 64;
pub(crate) const COLUMNS: usize = WIDTH * WIDTH;

/// `EDhApiWorldGenerationStep.LIGHT`: the column is complete, so DH never regenerates it.
pub(crate) const GEN_STEP_LIGHT: u8 = 9;
/// `EDhApiDataCompressionMode.Z_STD_BLOCK`, compressed at the level DH uses.
pub(crate) const COMPRESSION_MODE: u8 = 4;
const ZSTD_LEVEL: i32 = 3;
/// `FullDataSourceV2DTO.DATA_FORMAT.V2_LATEST`.
pub(crate) const DATA_FORMAT_VERSION: u8 = 2;

const BLOCK_STATE_SEPARATOR: &str = "_DH-BSW_";
const AIR_SERIAL: &str = "AIR";
const DEFAULT_BIOME: &str = "minecraft:plains";

/// One run of a column, as `FullDataPointUtil` packs it. Columns list runs top-down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Point {
    /// Region-wide pair id; see [`Registry`].
    pub id: u32,
    pub height: u16,
    /// Relative to the world's floor.
    pub bottom: u16,
    /// Block light in the high nibble, sky light in the low one.
    pub light: u8,
}

/// `FullDataSourceV2.relativePosToIndex`.
#[inline]
pub(crate) fn index(x: usize, z: usize) -> usize {
    x * WIDTH + z
}

fn write_varint(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn zigzag(n: i32) -> u32 {
    ((n << 1) ^ (n >> 31)) as u32
}

pub(crate) fn is_air(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

/// Non-solid, see-through blocks; DH gives the block under one its own run so it tints only that.
fn is_decoration(name: &str) -> bool {
    let Some(path) = name.strip_prefix("minecraft:") else {
        return false;
    };
    matches!(
        path,
        "snow"
            | "short_grass"
            | "grass"
            | "tall_grass"
            | "fern"
            | "large_fern"
            | "dead_bush"
            | "dandelion"
            | "poppy"
            | "blue_orchid"
            | "allium"
            | "azure_bluet"
            | "oxeye_daisy"
            | "cornflower"
            | "lily_of_the_valley"
            | "wither_rose"
            | "sunflower"
            | "lilac"
            | "rose_bush"
            | "peony"
            | "torchflower"
            | "pink_petals"
            | "wheat"
            | "carrots"
            | "potatoes"
            | "beetroots"
            | "sweet_berry_bush"
            | "vine"
            | "lily_pad"
            | "brown_mushroom"
            | "red_mushroom"
            | "redstone_wire"
            | "lever"
            | "cobweb"
            | "rail"
    ) || [
        "_tulip",
        "_sapling",
        "carpet",
        "torch",
        "_rail",
        "_button",
        "_pressure_plate",
        "_sign",
        "_banner",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
}

/// The block's full property set, sorted, where the state shows at a distance.
fn full_property_set(path: &str) -> Option<&'static [&'static str]> {
    Some(match path {
        "snow" => &["layers"],
        "water" | "lava" => &["level"],
        "wheat" | "carrots" | "potatoes" | "beetroots" | "nether_wart" | "sweet_berry_bush"
        | "cactus" | "sugar_cane" | "kelp" | "torchflower_crop" => &["age"],
        "farmland" => &["moisture"],
        "grass_block" | "podzol" | "mycelium" => &["snowy"],
        "sunflower" | "lilac" | "rose_bush" | "peony" | "tall_grass" | "large_fern"
        | "tall_seagrass" => &["half"],
        "sea_pickle" => &["pickles", "waterlogged"],
        "basalt"
        | "polished_basalt"
        | "hay_block"
        | "bone_block"
        | "quartz_pillar"
        | "purpur_pillar"
        | "deepslate"
        | "bamboo_block"
        | "stripped_bamboo_block"
        | "muddy_mangrove_roots" => &["axis"],
        p if p.ends_with("_log")
            || p.ends_with("_wood")
            || p.ends_with("_hyphae")
            || p.ends_with("froglight")
            || matches!(
                p.trim_start_matches("stripped_"),
                "crimson_stem" | "warped_stem"
            ) =>
        {
            &["axis"]
        }
        _ => return None,
    })
}

/// DH's block-state serial, e.g. `minecraft:snow_STATE_{layers:3}`. DH warns on an
/// incomplete property list and Arnis omits defaults, so anything [`full_property_set`]
/// doesn't cover goes out bare, which DH maps to the default state silently.
fn block_serial_into(item: &PaletteItem, out: &mut String) {
    out.clear();
    if is_air(&item.name) {
        out.push_str(AIR_SERIAL);
        return;
    }
    let path = match item.name.split_once(':') {
        Some(("minecraft", path)) => Some(path),
        Some(_) => None,
        None => {
            out.push_str("minecraft:");
            Some(item.name.as_str())
        }
    };
    out.push_str(&item.name);

    let (Some(names), Some(Value::Compound(props))) =
        (path.and_then(full_property_set), item.properties.as_ref())
    else {
        return;
    };
    if props.len() != names.len() {
        return;
    }
    let mut state = String::from("_STATE_");
    for name in names {
        let Some(Value::String(value)) = props.get(*name) else {
            return;
        };
        state.push('{');
        state.push_str(name);
        state.push(':');
        state.push_str(value);
        state.push('}');
    }
    out.push_str(&state);
}

pub(crate) struct Kind {
    serial: String,
    pub air: bool,
    pub decoration: bool,
}

/// Region-wide (biome, block) pairs that [`Point`] ids refer to. Pair 0 is air, as
/// DH reads a missing sample.
pub(crate) struct Registry {
    kinds: Vec<Kind>,
    kind_ids: FnvHashMap<String, u32>,
    biomes: Vec<&'static str>,
    biome_ids: FnvHashMap<&'static str, u16>,
    pairs: Vec<(u16, u32)>,
    /// Pair id by biome, then kind; `u32::MAX` where the pair doesn't exist yet.
    pair_grid: Vec<Vec<u32>>,
    pair_air: Vec<bool>,
    serial: String,
}

pub(crate) const AIR_KIND: u32 = 0;

impl Registry {
    pub(crate) fn new() -> Self {
        let mut registry = Self {
            kinds: Vec::new(),
            kind_ids: FnvHashMap::default(),
            biomes: Vec::new(),
            biome_ids: FnvHashMap::default(),
            pairs: Vec::new(),
            pair_grid: Vec::new(),
            pair_air: Vec::new(),
            serial: String::new(),
        };
        registry.kinds.push(Kind {
            serial: AIR_SERIAL.to_string(),
            air: true,
            decoration: false,
        });
        registry.kind_ids.insert(AIR_SERIAL.to_string(), AIR_KIND);
        let biome = registry.biome(DEFAULT_BIOME);
        registry.pair(biome, AIR_KIND);
        registry
    }

    /// Keyed by serial, so states DH can't tell apart never duplicate a mapping entry.
    pub(crate) fn kind(&mut self, item: &PaletteItem) -> u32 {
        block_serial_into(item, &mut self.serial);
        if let Some(&id) = self.kind_ids.get(self.serial.as_str()) {
            return id;
        }
        let id = self.kinds.len() as u32;
        self.kinds.push(Kind {
            serial: self.serial.clone(),
            air: is_air(&item.name),
            decoration: is_decoration(&item.name),
        });
        self.kind_ids.insert(self.serial.clone(), id);
        id
    }

    pub(crate) fn kind_info(&self, kind: u32) -> &Kind {
        &self.kinds[kind as usize]
    }

    pub(crate) fn biome(&mut self, name: &'static str) -> u16 {
        *self.biome_ids.entry(name).or_insert_with(|| {
            self.biomes.push(name);
            self.pair_grid.push(Vec::new());
            (self.biomes.len() - 1) as u16
        })
    }

    pub(crate) fn pair(&mut self, biome: u16, kind: u32) -> u32 {
        let row = &mut self.pair_grid[biome as usize];
        if row.len() <= kind as usize {
            row.resize(kind as usize + 1, u32::MAX);
        }
        let slot = &mut row[kind as usize];
        if *slot == u32::MAX {
            *slot = self.pairs.len() as u32;
            self.pairs.push((biome, kind));
            self.pair_air.push(self.kinds[kind as usize].air);
        }
        *slot
    }

    pub(crate) fn is_air(&self, pair: u32) -> bool {
        self.pair_air[pair as usize]
    }

    pub(crate) fn entry(&self, pair: u32) -> (&str, &str) {
        let (biome, kind) = self.pairs[pair as usize];
        (
            self.biomes[biome as usize],
            &self.kinds[kind as usize].serial,
        )
    }
}

/// The compressed column values of one `FullData` row.
pub(crate) struct Row {
    pub data: Vec<u8>,
    pub north: Vec<u8>,
    pub south: Vec<u8>,
    pub east: Vec<u8>,
    pub west: Vec<u8>,
    pub gen_steps: Vec<u8>,
    pub compression_modes: Vec<u8>,
    pub mapping: Vec<u8>,
    pub checksum: i32,
}

impl Row {
    pub(crate) fn len(&self) -> usize {
        [
            &self.data,
            &self.north,
            &self.south,
            &self.east,
            &self.west,
            &self.gen_steps,
            &self.compression_modes,
            &self.mapping,
        ]
        .iter()
        .map(|blob| blob.len())
        .sum()
    }
}

/// Encodes rows, keeping its buffers and zstd context across a region's rows.
pub(crate) struct Encoder {
    compressor: zstd::bulk::Compressor<'static>,
    raw: Vec<u8>,
    /// Region pair id -> row id, `u32::MAX` where the row doesn't use the pair.
    remap: Vec<u32>,
    /// Region pair ids in row id order.
    used: Vec<u32>,
}

impl Encoder {
    pub(crate) fn new() -> Self {
        Self {
            compressor: zstd::bulk::Compressor::new(ZSTD_LEVEL)
                .expect("zstd level 3 is always valid"),
            raw: Vec::new(),
            remap: Vec::new(),
            used: Vec::new(),
        }
    }

    pub(crate) fn encode(
        &mut self,
        columns: &[Vec<Point>],
        gen_steps: &[u8],
        registry: &Registry,
    ) -> std::io::Result<Row> {
        debug_assert_eq!(columns.len(), COLUMNS);
        self.assign_ids(columns, registry);

        // The border columns go out in the four edge blobs instead.
        let inner = 1..WIDTH - 1;
        self.raw.clear();
        write_columns(&mut self.raw, columns, &self.remap, inner.clone(), inner);
        let checksum = fnv32(&self.raw) as i32;
        let data = self.compressor.compress(&self.raw)?;
        let north = self.compress_columns(columns, 0..WIDTH, 0..1)?;
        let south = self.compress_columns(columns, 0..WIDTH, WIDTH - 1..WIDTH)?;
        let east = self.compress_columns(columns, WIDTH - 1..WIDTH, 0..WIDTH)?;
        let west = self.compress_columns(columns, 0..1, 0..WIDTH)?;
        let gen_steps = self.compressor.compress(gen_steps)?;
        let compression_modes = self.compressor.compress(&[0u8; COLUMNS])?;

        // `FullDataPointIdMap.serialize`: a count, then `writeUTF` each (names are ASCII).
        self.raw.clear();
        self.raw
            .extend_from_slice(&(self.used.len() as i32).to_be_bytes());
        for &pair in &self.used {
            let (biome, serial) = registry.entry(pair);
            let len = biome.len() + BLOCK_STATE_SEPARATOR.len() + serial.len();
            self.raw.extend_from_slice(&(len as u16).to_be_bytes());
            self.raw.extend_from_slice(biome.as_bytes());
            self.raw.extend_from_slice(BLOCK_STATE_SEPARATOR.as_bytes());
            self.raw.extend_from_slice(serial.as_bytes());
        }
        let mapping = self.compressor.compress(&self.raw)?;

        for &pair in &self.used {
            self.remap[pair as usize] = u32::MAX;
        }
        Ok(Row {
            data,
            north,
            south,
            east,
            west,
            gen_steps,
            compression_modes,
            mapping,
            checksum,
        })
    }

    fn assign_ids(&mut self, columns: &[Vec<Point>], registry: &Registry) {
        self.remap.resize(registry.pairs.len(), u32::MAX);
        self.used.clear();
        // Row id 0 must be air too: DH reads it for samples a column lacks.
        self.remap[0] = 0;
        self.used.push(0);
        for point in columns.iter().flatten() {
            let slot = &mut self.remap[point.id as usize];
            if *slot == u32::MAX {
                *slot = self.used.len() as u32;
                self.used.push(point.id);
            }
        }
    }

    fn compress_columns(
        &mut self,
        columns: &[Vec<Point>],
        xs: Range<usize>,
        zs: Range<usize>,
    ) -> std::io::Result<Vec<u8>> {
        self.raw.clear();
        write_columns(&mut self.raw, columns, &self.remap, xs, zs);
        self.compressor.compress(&self.raw)
    }
}

/// `writeDataSourceDataArrayToBlobV2` over one rectangle of columns.
fn write_columns(
    out: &mut Vec<u8>,
    columns: &[Vec<Point>],
    remap: &[u32],
    xs: Range<usize>,
    zs: Range<usize>,
) {
    let cells = || {
        xs.clone()
            .flat_map(|x| zs.clone().map(move |z| &columns[index(x, z)]))
    };
    for column in cells() {
        write_varint(out, column.len() as u32);
    }
    let mut previous_bottom = 0i32;
    for point in cells().flatten() {
        let bottom = point.bottom as i32;
        let discontinuous = bottom != previous_bottom - point.height as i32;
        previous_bottom = bottom;
        let lit = point.light != 0;
        write_varint(
            out,
            (remap[point.id as usize] << 2) | ((lit as u32) << 1) | discontinuous as u32,
        );
    }
    for point in cells().flatten() {
        write_varint(out, point.height as u32);
    }
    previous_bottom = 0;
    for point in cells().flatten() {
        let bottom = point.bottom as i32;
        let expected = previous_bottom - point.height as i32;
        if bottom != expected {
            write_varint(out, zigzag(bottom - expected));
        }
        previous_bottom = bottom;
    }
    for point in cells().flatten() {
        if point.light != 0 {
            out.push(point.light);
        }
    }
}

/// DH stores a checksum per row but never reads it back; any stable hash does.
fn fnv32(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5u32, |hash, &b| {
        (hash ^ b as u32).wrapping_mul(0x0100_0193)
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    fn item(name: &str, props: &[(&str, &str)]) -> PaletteItem {
        PaletteItem {
            name: name.to_string(),
            properties: (!props.is_empty()).then(|| {
                Value::Compound(
                    props
                        .iter()
                        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
                        .collect::<HashMap<_, _>>(),
                )
            }),
        }
    }

    fn serial(name: &str, props: &[(&str, &str)]) -> String {
        let mut out = String::new();
        block_serial_into(&item(name, props), &mut out);
        out
    }

    #[test]
    fn serials_match_dh() {
        assert_eq!(serial("minecraft:air", &[]), "AIR");
        assert_eq!(serial("minecraft:cave_air", &[]), "AIR");
        assert_eq!(serial("minecraft:stone", &[]), "minecraft:stone");
        assert_eq!(serial("stone", &[]), "minecraft:stone");
        assert_eq!(
            serial("minecraft:snow", &[("layers", "3")]),
            "minecraft:snow_STATE_{layers:3}"
        );
        assert_eq!(
            serial("minecraft:stripped_oak_log", &[("axis", "x")]),
            "minecraft:stripped_oak_log_STATE_{axis:x}"
        );
        assert_eq!(
            serial(
                "minecraft:sea_pickle",
                &[("waterlogged", "true"), ("pickles", "2")]
            ),
            "minecraft:sea_pickle_STATE_{pickles:2}{waterlogged:true}"
        );
    }

    /// A partial property list would make DH warn and fall back anyway.
    #[test]
    fn partial_states_go_out_bare() {
        assert_eq!(
            serial("minecraft:oak_stairs", &[("facing", "north")]),
            "minecraft:oak_stairs"
        );
        assert_eq!(
            serial("minecraft:oak_leaves", &[("persistent", "true")]),
            "minecraft:oak_leaves"
        );
        assert_eq!(
            serial("minecraft:mushroom_stem", &[("up", "false")]),
            "minecraft:mushroom_stem"
        );
        assert_eq!(
            serial("minecraft:pumpkin_stem", &[("age", "7")]),
            "minecraft:pumpkin_stem"
        );
    }

    #[test]
    fn varints_match_dh() {
        let mut out = Vec::new();
        write_varint(&mut out, 0);
        write_varint(&mut out, 127);
        write_varint(&mut out, 128);
        write_varint(&mut out, 300);
        assert_eq!(out, [0, 127, 0x80, 1, 0xAC, 2]);
        assert_eq!(zigzag(0), 0);
        assert_eq!(zigzag(-1), 1);
        assert_eq!(zigzag(1), 2);
        assert_eq!(zigzag(-64), 127);
    }

    #[test]
    fn decorations_cover_plants_but_not_solids() {
        for name in ["poppy", "snow", "white_carpet", "wall_torch", "oak_sapling"] {
            assert!(is_decoration(&format!("minecraft:{name}")), "{name}");
        }
        for name in ["stone", "glass", "oak_leaves", "water", "snow_block"] {
            assert!(!is_decoration(&format!("minecraft:{name}")), "{name}");
        }
    }

    /// `readBlobToDataSourceDataArrayV2`, transcribed, so the tests check the
    /// encoder against the reader rather than against itself.
    pub(crate) fn read_columns(
        blob: &[u8],
        columns: &mut [Vec<(u32, u16, u16, u8)>],
        xs: Range<usize>,
        zs: Range<usize>,
    ) {
        let mut at = 0usize;
        let mut varint = || {
            let mut value = 0u32;
            let mut shift = 0;
            loop {
                let b = blob[at];
                at += 1;
                value |= ((b & 0x7F) as u32) << shift;
                shift += 7;
                if b & 0x80 == 0 {
                    return value;
                }
            }
        };
        let cells: Vec<usize> = xs
            .flat_map(|x| zs.clone().map(move |z| index(x, z)))
            .collect();
        for &i in &cells {
            columns[i] = vec![(0, 0, 0, 0); varint() as usize];
        }
        let mut flags = HashMap::new();
        for &i in &cells {
            for j in 0..columns[i].len() {
                let encoded = varint();
                columns[i][j].0 = encoded >> 2;
                flags.insert((i, j), encoded & 3);
            }
        }
        for &i in &cells {
            for point in columns[i].iter_mut() {
                point.1 = varint() as u16;
            }
        }
        let mut previous_bottom = 0i32;
        for &i in &cells {
            for j in 0..columns[i].len() {
                let error = if flags[&(i, j)] & 1 != 0 {
                    let z = varint();
                    ((z >> 1) as i32) ^ -((z & 1) as i32)
                } else {
                    0
                };
                let bottom = previous_bottom - columns[i][j].1 as i32 + error;
                columns[i][j].2 = bottom as u16;
                previous_bottom = bottom;
            }
        }
        for &i in &cells {
            for j in 0..columns[i].len() {
                if flags[&(i, j)] & 2 != 0 {
                    columns[i][j].3 = blob[at];
                    at += 1;
                }
            }
        }
        assert_eq!(at, blob.len(), "trailing bytes");
    }

    pub(crate) fn read_mapping(blob: &[u8]) -> Vec<String> {
        let count = i32::from_be_bytes(blob[0..4].try_into().unwrap()) as usize;
        let mut at = 4;
        (0..count)
            .map(|_| {
                let len = u16::from_be_bytes(blob[at..at + 2].try_into().unwrap()) as usize;
                at += 2;
                at += len;
                String::from_utf8(blob[at - len..at].to_vec()).unwrap()
            })
            .collect()
    }

    pub(crate) fn decompress(frame: &[u8]) -> Vec<u8> {
        // DH sizes its buffer from the frame header, so the size has to be in it.
        let size = zstd::zstd_safe::get_frame_content_size(frame)
            .expect("valid frame")
            .expect("content size in the frame header");
        zstd::bulk::decompress(frame, size as usize).unwrap()
    }

    #[test]
    fn rows_round_trip_through_the_reader() {
        let mut registry = Registry::new();
        let grass = registry.kind(&item("minecraft:grass_block", &[]));
        let stone = registry.kind(&item("minecraft:stone", &[]));
        let forest = registry.biome("minecraft:forest");
        let pairs = [
            registry.pair(forest, AIR_KIND),
            registry.pair(forest, grass),
            registry.pair(forest, stone),
        ];

        let mut columns = vec![Vec::new(); COLUMNS];
        let mut gen_steps = vec![0u8; COLUMNS];
        for x in 0..WIDTH {
            for z in 0..WIDTH {
                if (x + z) % 7 == 3 {
                    continue; // absent columns read back empty
                }
                let surface = 70 + ((x * 3 + z) % 9) as u16;
                columns[index(x, z)] = vec![
                    Point {
                        id: pairs[0],
                        height: 384 - surface,
                        bottom: surface,
                        light: 0x0F,
                    },
                    Point {
                        id: pairs[1],
                        height: 1,
                        bottom: surface - 1,
                        light: ((x % 16) as u8) << 4 | 0x0E,
                    },
                    // A gap, so the bottom misses the stacking prediction.
                    Point {
                        id: pairs[2],
                        height: 4,
                        bottom: 0,
                        light: 0,
                    },
                ];
                gen_steps[index(x, z)] = GEN_STEP_LIGHT;
            }
        }

        let row = Encoder::new()
            .encode(&columns, &gen_steps, &registry)
            .unwrap();

        let mut read = vec![Vec::new(); COLUMNS];
        read_columns(&decompress(&row.data), &mut read, 1..63, 1..63);
        read_columns(&decompress(&row.north), &mut read, 0..64, 0..1);
        read_columns(&decompress(&row.south), &mut read, 0..64, 63..64);
        read_columns(&decompress(&row.east), &mut read, 63..64, 0..64);
        read_columns(&decompress(&row.west), &mut read, 0..1, 0..64);

        let mapping = read_mapping(&decompress(&row.mapping));
        assert_eq!(mapping[0], "minecraft:plains_DH-BSW_AIR");
        for (column, decoded) in columns.iter().zip(&read) {
            let expected: Vec<_> = column
                .iter()
                .map(|p| {
                    let (biome, serial) = registry.entry(p.id);
                    (
                        format!("{biome}_DH-BSW_{serial}"),
                        p.height,
                        p.bottom,
                        p.light,
                    )
                })
                .collect();
            let got: Vec<_> = decoded
                .iter()
                .map(|&(id, height, bottom, light)| {
                    (mapping[id as usize].clone(), height, bottom, light)
                })
                .collect();
            assert_eq!(got, expected);
        }

        assert_eq!(decompress(&row.gen_steps), gen_steps);
        assert_eq!(decompress(&row.compression_modes), vec![0u8; COLUMNS]);
    }

    #[test]
    fn mappings_are_per_row_and_start_with_air() {
        let mut registry = Registry::new();
        let stone = registry.kind(&item("minecraft:stone", &[]));
        let dirt = registry.kind(&item("minecraft:dirt", &[]));
        let biome = registry.biome("minecraft:desert");
        registry.pair(biome, stone);
        let dirt_pair = registry.pair(biome, dirt);

        let mut columns = vec![Vec::new(); COLUMNS];
        columns[index(5, 5)] = vec![Point {
            id: dirt_pair,
            height: 3,
            bottom: 0,
            light: 0,
        }];
        let mut encoder = Encoder::new();
        let row = encoder.encode(&columns, &[0; COLUMNS], &registry).unwrap();
        assert_eq!(
            read_mapping(&decompress(&row.mapping)),
            [
                "minecraft:plains_DH-BSW_AIR",
                "minecraft:desert_DH-BSW_minecraft:dirt"
            ]
        );
        // The scratch map is reset, so the next row numbers from scratch.
        assert!(encoder.remap.iter().all(|&id| id == u32::MAX));
    }

    #[test]
    fn identical_serials_share_a_kind() {
        let mut registry = Registry::new();
        let north = registry.kind(&item("minecraft:oak_stairs", &[("facing", "north")]));
        let south = registry.kind(&item("minecraft:oak_stairs", &[("facing", "south")]));
        assert_eq!(north, south);
        assert_eq!(registry.kind(&item("minecraft:void_air", &[])), AIR_KIND);
    }
}
