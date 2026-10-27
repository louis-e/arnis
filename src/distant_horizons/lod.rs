//! DH sections built from finished Java chunks. Level `n` spans `64 << n` blocks,
//! so a region holds levels 0..=3 exactly; level 3 is flagged for DH to propagate.
//! Chunks arrive in Morton order, so one section per level is resident at a time.

use super::encode::{index, Encoder, Point, Registry, AIR_KIND, COLUMNS, GEN_STEP_LIGHT, WIDTH};
use super::DhWriter;
use crate::world_editor::common::Section;

pub(crate) const TOP_LEVEL: usize = 3;

/// Full sky light, no block light: the light above everything a chunk holds.
const OPEN_SKY: u8 = 0x0F;

struct Level {
    x: i32,
    z: i32,
    filled: bool,
    columns: Vec<Vec<Point>>,
    gen_steps: Vec<u8>,
}

impl Level {
    fn new() -> Self {
        Self {
            x: 0,
            z: 0,
            filled: false,
            columns: vec![Vec::new(); COLUMNS],
            gen_steps: vec![0; COLUMNS],
        }
    }
}

/// The run a column is currently extending while it is walked top-down.
#[derive(Clone, Copy)]
struct Run {
    kind: u32,
    /// Exclusive top Y.
    top: i32,
    light: u8,
    /// The run above was a decoration, so this one ends after a single block.
    split: bool,
}

pub(crate) struct RegionLod<'a> {
    writer: &'a DhWriter,
    registry: Registry,
    levels: [Level; TOP_LEVEL + 1],
    encoder: Encoder,
    floor_y: i32,
    ceiling_y: i32,
    palette: Vec<u32>,
    voxels: Vec<u32>,
    section_at: Vec<u32>,
    pub(crate) sections_written: u64,
}

impl<'a> RegionLod<'a> {
    /// DH measures bottoms from `floor_y`; `ceiling_y` is one past the world's top.
    pub(crate) fn new(writer: &'a DhWriter, floor_y: i32, ceiling_y: i32) -> Self {
        Self {
            writer,
            registry: Registry::new(),
            levels: std::array::from_fn(|_| Level::new()),
            encoder: Encoder::new(),
            floor_y,
            ceiling_y,
            palette: Vec::new(),
            voxels: vec![AIR_KIND; 4096],
            section_at: Vec::new(),
            sections_written: 0,
        }
    }

    /// Feeds a chunk as `LodDataBuilder` reads one: runs from the ceiling down to the
    /// lowest non-air section, each lit by the block above it.
    pub(crate) fn ingest_chunk(
        &mut self,
        chunk_x: i32,
        chunk_z: i32,
        sections: &[Section],
        span: (i32, i32),
        lighting: Option<&[(Vec<i8>, Vec<i8>)]>,
        biome_names: &[&'static str; 16],
    ) {
        let (span_min, span_max) = span;
        let biomes = biome_names.map(|name| self.registry.biome(name));

        self.section_at.clear();
        self.section_at
            .resize((span_max - span_min + 1).max(0) as usize, u32::MAX);
        for (i, section) in sections.iter().enumerate() {
            let y = section.y as i32;
            if (span_min..=span_max).contains(&y) {
                self.section_at[(y - span_min) as usize] = i as u32;
            }
        }
        let bottom = sections
            .iter()
            .filter(|s| {
                s.block_states
                    .palette
                    .iter()
                    .any(|p| !super::encode::is_air(&p.name))
            })
            .map(|s| s.y as i32)
            .min()
            .unwrap_or(span_min)
            .clamp(span_min, span_max);

        let level = &mut self.levels[0];
        level.x = chunk_x >> 2;
        level.z = chunk_z >> 2;
        level.filled = true;
        let origin_x = ((chunk_x & 3) * 16) as usize;
        let origin_z = ((chunk_z & 3) * 16) as usize;
        for x in 0..16 {
            for z in 0..16 {
                let i = index(origin_x + x, origin_z + z);
                level.columns[i].clear();
                level.gen_steps[i] = GEN_STEP_LIGHT;
            }
        }

        let mut runs = [Run {
            kind: AIR_KIND,
            top: self.ceiling_y,
            light: OPEN_SKY,
            split: false,
        }; 256];

        for section_y in (bottom..=span_max).rev() {
            let slot = self.section_at[(section_y - span_min) as usize];
            let section = (slot != u32::MAX).then(|| &sections[slot as usize]);
            let uniform = self.decode(section);

            for z in 0..16usize {
                for x in 0..16usize {
                    let run = &mut runs[z * 16 + x];
                    let column = &mut self.levels[0].columns[index(origin_x + x, origin_z + z)];
                    let biome = biomes[(z >> 2) * 4 + (x >> 2)];
                    for local_y in (0..16usize).rev() {
                        let kind =
                            uniform.unwrap_or_else(|| self.voxels[(local_y << 8) | (z << 4) | x]);
                        if kind == run.kind && !run.split {
                            if uniform.is_some() {
                                break;
                            }
                            continue;
                        }
                        let y = section_y * 16 + local_y as i32;
                        close_run(&mut self.registry, column, run, biome, y + 1, self.floor_y);
                        run.split = self.registry.kind_info(run.kind).decoration;
                        run.kind = kind;
                        run.top = y + 1;
                        run.light = light_at(lighting, span_min, x, y + 1, z);
                    }
                }
            }
        }

        for z in 0..16usize {
            for x in 0..16usize {
                let column = &mut self.levels[0].columns[index(origin_x + x, origin_z + z)];
                let biome = biomes[(z >> 2) * 4 + (x >> 2)];
                close_run(
                    &mut self.registry,
                    column,
                    &runs[z * 16 + x],
                    biome,
                    bottom * 16,
                    self.floor_y,
                );
            }
        }
    }

    /// Called after the four chunks of Morton column `column` have been fed.
    pub(crate) fn end_column(&mut self, column: usize) {
        for level in 0..=TOP_LEVEL {
            if (column + 1).is_multiple_of(4 << (2 * level)) {
                self.flush(level);
            }
        }
    }

    /// Flushes whatever is left, in case a caller stops short of a full region.
    pub(crate) fn finish(&mut self) {
        for level in 0..=TOP_LEVEL {
            self.flush(level);
        }
    }

    /// The section's kind if it is uniform, else `None` with `voxels` unpacked.
    fn decode(&mut self, section: Option<&Section>) -> Option<u32> {
        let Some(section) = section else {
            return Some(AIR_KIND);
        };
        let states = &section.block_states;
        self.palette.clear();
        for item in &states.palette {
            let kind = self.registry.kind(item);
            self.palette.push(kind);
        }
        let first = self.palette.first().copied().unwrap_or(AIR_KIND);
        let Some(data) = states.data.as_ref().filter(|_| self.palette.len() > 1) else {
            return Some(first);
        };
        if self.palette.iter().all(|&kind| kind == first) {
            return Some(first);
        }

        let mut bits = 4usize;
        while (1usize << bits) < self.palette.len() {
            bits += 1;
        }
        let per_long = 64 / bits;
        let mask = (1u64 << bits) - 1;
        let (mut long_index, mut slot) = (0usize, 0usize);
        for voxel in self.voxels.iter_mut() {
            let entry = data
                .get(long_index)
                .map_or(0, |&word| ((word as u64 >> (slot * bits)) & mask) as usize);
            *voxel = self.palette.get(entry).copied().unwrap_or(AIR_KIND);
            slot += 1;
            if slot == per_long {
                slot = 0;
                long_index += 1;
            }
        }
        None
    }

    fn flush(&mut self, level: usize) {
        if !self.levels[level].filled {
            return;
        }
        let section = &self.levels[level];
        match self
            .encoder
            .encode(&section.columns, &section.gen_steps, &self.registry)
        {
            Ok(row) => {
                self.writer.put_section(
                    level as u8,
                    section.x,
                    section.z,
                    &row,
                    level == TOP_LEVEL,
                );
                self.sections_written += 1;
            }
            Err(e) => self.writer.record_error(e.to_string()),
        }
        if level < TOP_LEVEL {
            self.downsample(level);
        }

        let section = &mut self.levels[level];
        section.columns.iter_mut().for_each(Vec::clear);
        section.gen_steps.fill(0);
        section.filled = false;
    }

    /// `updateFromOneBelowDetailLevel`: each 2x2 columns become one in the parent's quadrant.
    fn downsample(&mut self, level: usize) {
        let (lower, upper) = self.levels.split_at_mut(level + 1);
        let child = &lower[level];
        let parent = &mut upper[0];
        parent.x = child.x >> 1;
        parent.z = child.z >> 1;
        parent.filled = true;
        let offset_x = (child.x & 1) as usize * WIDTH / 2;
        let offset_z = (child.z & 1) as usize * WIDTH / 2;

        for x in (0..WIDTH).step_by(2) {
            for z in (0..WIDTH).step_by(2) {
                let quad = [
                    index(x, z),
                    index(x, z + 1),
                    index(x + 1, z),
                    index(x + 1, z + 1),
                ];
                let target = index(offset_x + x / 2, offset_z + z / 2);
                parent.gen_steps[target] =
                    quad.iter().map(|&i| child.gen_steps[i]).min().unwrap_or(0);
                merge_columns(
                    quad.map(|i| child.columns[i].as_slice()),
                    &self.registry,
                    &mut parent.columns[target],
                );
            }
        }
    }
}

/// Ends `run` at `bottom` and appends it to the column, unless it is empty.
fn close_run(
    registry: &mut Registry,
    column: &mut Vec<Point>,
    run: &Run,
    biome: u16,
    bottom: i32,
    floor_y: i32,
) {
    if run.top <= bottom {
        return;
    }
    column.push(Point {
        id: registry.pair(biome, run.kind),
        height: (run.top - bottom) as u16,
        bottom: (bottom - floor_y) as u16,
        light: run.light,
    });
}

/// Packed light at a block: open sky above the span, dark below it or without arrays.
fn light_at(
    lighting: Option<&[(Vec<i8>, Vec<i8>)]>,
    span_min: i32,
    x: usize,
    y: i32,
    z: usize,
) -> u8 {
    let Some(lighting) = lighting else {
        return OPEN_SKY;
    };
    let Ok(slot) = usize::try_from((y >> 4) - span_min) else {
        return 0;
    };
    let Some((sky, block)) = lighting.get(slot) else {
        return OPEN_SKY;
    };
    let i = (((y & 15) as usize) << 8) | (z << 4) | x;
    let nibble = |plane: &[i8]| {
        plane
            .get(i >> 1)
            .map_or(0, |&b| ((b as u8) >> ((i & 1) * 4)) & 0x0F)
    };
    (nibble(block) << 4) | nibble(sky)
}

/// `mergeInputTwoByTwoDataColumn`: per slice, the most common non-air block and its light.
fn merge_columns(columns: [&[Point]; 4], registry: &Registry, out: &mut Vec<Point>) {
    // Equal columns only coalesce equal neighbours; flat ground mostly lands here.
    if columns[1..].iter().all(|c| *c == columns[0]) {
        out.clear();
        for &point in columns[0] {
            match out.last_mut() {
                Some(run) if run.id == point.id && run.light == point.light => {
                    run.height += point.height;
                    run.bottom = point.bottom;
                }
                _ => out.push(point),
            }
        }
        return;
    }
    merge_slices(columns, registry, out);
}

/// DH's slices visited in one sweep instead of sorting every boundary. A slice holds
/// no boundary, so its bottom samples the runs DH samples at its middle.
fn merge_slices(columns: [&[Point]; 4], registry: &Registry, out: &mut Vec<Point>) {
    out.clear();
    let top = |p: &Point| p.bottom as i32 + p.height as i32;
    let Some(mut low) = columns
        .iter()
        .filter_map(|c| c.last())
        .map(|p| p.bottom as i32)
        .min()
    else {
        return;
    };
    let ceiling = columns
        .iter()
        .filter_map(|c| c.first())
        .map(top)
        .max()
        .unwrap_or(low);

    // Columns are top-down, so the sweep up starts at each one's last run.
    let mut cursors = columns.map(|c| c.len() as isize - 1);
    let mut current: Option<Point> = None;
    while low < ceiling {
        let mut high = ceiling;
        let mut ids = [0u32; 4];
        let mut lights = [0u8; 4];
        for (i, column) in columns.iter().enumerate() {
            while cursors[i] >= 0 && top(&column[cursors[i] as usize]) <= low {
                cursors[i] -= 1;
            }
            let Ok(at) = usize::try_from(cursors[i]) else {
                continue;
            };
            let point = column[at];
            if low >= point.bottom as i32 {
                ids[i] = point.id;
                lights[i] = point.light;
                high = high.min(top(&point));
            } else {
                high = high.min(point.bottom as i32);
            }
        }

        let height = high - low;
        let id = most_common(ids, registry);
        let light = (average(lights.map(|l| l >> 4), ids, id) << 4)
            | average(lights.map(|l| l & 0x0F), ids, id);
        match current.as_mut() {
            Some(run) if run.id == id && run.light == light => run.height += height as u16,
            _ => {
                out.extend(current);
                current = Some(Point {
                    id,
                    height: height as u16,
                    bottom: low as u16,
                    light,
                });
            }
        }
        low = high;
    }
    out.extend(current);
    out.reverse();
}

/// `determineMostCommonValueInColumnSlice`: air never counts, ties go to the earliest column.
fn most_common(ids: [u32; 4], registry: &Registry) -> u32 {
    let mut counts = [0u8; 4];
    for &id in &ids {
        if registry.is_air(id) {
            continue;
        }
        let first = ids.iter().position(|&other| other == id).unwrap_or(3);
        counts[first] += 1;
    }
    let max = counts.iter().copied().max().unwrap_or(0);
    ids[counts.iter().position(|&c| c == max).unwrap_or(0)]
}

/// Averaged over the winner's columns only, so water light doesn't brighten the sand below.
fn average(values: [u8; 4], ids: [u32; 4], winner: u32) -> u8 {
    let (sum, count) = (0..4)
        .filter(|&i| ids[i] == winner)
        .fold((0u32, 0u32), |(sum, count), i| {
            (sum + values[i] as u32, count + 1)
        });
    sum.checked_div(count).unwrap_or(0) as u8
}

#[cfg(test)]
mod tests {
    use super::super::encode::tests::{decompress, read_columns, read_mapping};
    use super::*;
    use crate::world_editor::common::{Blockstates, PaletteItem};
    use fastnbt::LongArray;
    use fnv::FnvHashMap;

    fn palette_item(name: &str) -> PaletteItem {
        PaletteItem {
            name: format!("minecraft:{name}"),
            properties: None,
        }
    }

    /// A section from a block lookup over (x, y, z), packed the way chunks are.
    fn section(y: i8, palette: &[&str], block: impl Fn(usize, usize, usize) -> usize) -> Section {
        let bits = 4;
        let mut longs = vec![0i64; 4096 / 16];
        for i in 0..4096 {
            let entry = block(i & 15, i >> 8, (i >> 4) & 15) as i64;
            longs[i / 16] |= entry << ((i % 16) * bits);
        }
        Section {
            block_states: Blockstates {
                palette: palette.iter().map(|n| palette_item(n)).collect(),
                data: Some(LongArray::new(longs)),
                other: FnvHashMap::default(),
            },
            y,
            other: FnvHashMap::default(),
        }
    }

    fn uniform(y: i8, name: &str) -> Section {
        Section {
            block_states: Blockstates {
                palette: vec![palette_item(name)],
                data: None,
                other: FnvHashMap::default(),
            },
            y,
            other: FnvHashMap::default(),
        }
    }

    fn writer() -> (tempfile::TempDir, DhWriter) {
        let dir = tempfile::tempdir().unwrap();
        let writer = DhWriter::create(dir.path()).unwrap();
        (dir, writer)
    }

    fn names(lod: &RegionLod, column: &[Point]) -> Vec<(String, u16, u16, u8)> {
        column
            .iter()
            .map(|p| {
                let (_, serial) = lod.registry.entry(p.id);
                (serial.to_string(), p.height, p.bottom, p.light)
            })
            .collect()
    }

    const PLAINS: [&str; 16] = ["minecraft:plains"; 16];

    #[test]
    fn columns_are_runs_from_the_ceiling_down() {
        let (_dir, writer) = writer();
        let mut lod = RegionLod::new(&writer, -64, 320);
        // Bedrock under air under three stone, a grass block and a poppy.
        let sections = [
            uniform(-4, "bedrock"),
            section(
                0,
                &["air", "stone", "grass_block", "poppy"],
                |_, y, _| match y {
                    0..=2 => 1,
                    3 => 2,
                    4 => 3,
                    _ => 0,
                },
            ),
        ];
        // Sky 15 above the poppy, 12 at the poppy, a torch's 9 beside the grass.
        let mut lighting = vec![(Vec::new(), Vec::new()); 24];
        let mut sky = vec![0xFFu8 as i8; 2048];
        let mut block = vec![0i8; 2048];
        let set = |plane: &mut Vec<i8>, x: usize, y: usize, z: usize, v: u8| {
            let i = (y << 8) | (z << 4) | x;
            let b = plane[i >> 1] as u8;
            plane[i >> 1] = if i & 1 == 0 {
                (b & 0xF0) | v
            } else {
                (b & 0x0F) | (v << 4)
            } as i8;
        };
        for y in 0..5 {
            set(&mut sky, 0, y, 0, 0);
        }
        set(&mut sky, 0, 4, 0, 12);
        set(&mut block, 0, 4, 0, 9);
        lighting[4] = (sky, block);

        lod.ingest_chunk(0, 0, &sections, (-4, 19), Some(&lighting), &PLAINS);
        let column = names(&lod, &lod.levels[0].columns[index(0, 0)]);
        let expected: [(&str, u16, u16, u8); 6] = [
            ("AIR", 320 - 5, 5 + 64, 0x0F),
            ("minecraft:poppy", 1, 4 + 64, 0x0F),
            ("minecraft:grass_block", 1, 3 + 64, 0x9C),
            ("minecraft:stone", 3, 64, 0),
            // Unlit air between the terrain and the bedrock floor.
            ("AIR", 48, 16, 0),
            ("minecraft:bedrock", 16, 0, 0),
        ];
        assert_eq!(
            column,
            expected.map(|(s, h, b, l)| (s.to_string(), h, b, l))
        );
        assert_eq!(lod.levels[0].gen_steps[index(0, 0)], GEN_STEP_LIGHT);
    }

    /// A decoration only tints the block right under it, so that block is its
    /// own run even when the same block continues below.
    #[test]
    fn decorations_split_the_block_below() {
        let (_dir, writer) = writer();
        let mut lod = RegionLod::new(&writer, -64, 320);
        let sections = [section(0, &["air", "stone", "snow"], |_, y, _| match y {
            0..=9 => 1,
            10 => 2,
            _ => 0,
        })];
        lod.ingest_chunk(0, 0, &sections, (-4, 19), None, &PLAINS);
        let column: Vec<_> = names(&lod, &lod.levels[0].columns[index(3, 3)])
            .into_iter()
            .map(|(s, h, b, _)| (s, h, b))
            .collect();
        assert_eq!(
            column,
            [
                ("AIR".to_string(), 320 - 11, 11 + 64),
                ("minecraft:snow".to_string(), 1, 10 + 64),
                ("minecraft:stone".to_string(), 1, 9 + 64),
                ("minecraft:stone".to_string(), 9, 64),
            ]
        );
    }

    #[test]
    fn biomes_follow_the_four_by_four_cells() {
        let (_dir, writer) = writer();
        let mut lod = RegionLod::new(&writer, -64, 320);
        let mut biomes = PLAINS;
        biomes[3 * 4 + 1] = "minecraft:desert"; // cell x 4..8, z 12..16
        lod.ingest_chunk(5, 0, &[uniform(0, "sand")], (-4, 19), None, &biomes);
        let biome_of = |x: usize, z: usize| {
            let p = lod.levels[0].columns[index(16 + x, z)][1];
            lod.registry.entry(p.id).0.to_string()
        };
        assert_eq!(biome_of(5, 13), "minecraft:desert");
        assert_eq!(biome_of(3, 13), "minecraft:plains");
        assert_eq!(biome_of(5, 11), "minecraft:plains");
    }

    fn point(registry: &mut Registry, name: &str, height: u16, bottom: u16, light: u8) -> Point {
        let kind = registry.kind(&palette_item(name));
        let biome = registry.biome("minecraft:plains");
        Point {
            id: registry.pair(biome, kind),
            height,
            bottom,
            light,
        }
    }

    #[test]
    fn merging_keeps_the_most_common_solid_block() {
        let mut registry = Registry::new();
        let air = |r: &mut Registry, h, b| point(r, "air", h, b, 0x0F);
        let columns = [
            vec![
                air(&mut registry, 90, 10),
                point(&mut registry, "stone", 10, 0, 0),
            ],
            vec![
                air(&mut registry, 92, 8),
                point(&mut registry, "dirt", 8, 0, 2),
            ],
            vec![
                air(&mut registry, 90, 10),
                point(&mut registry, "stone", 10, 0, 4),
            ],
            vec![air(&mut registry, 100, 0)],
        ];
        let mut out = Vec::new();
        merge_columns(
            columns.each_ref().map(|c| c.as_slice()),
            &registry,
            &mut out,
        );
        let stone = columns[0][1].id;
        let dirt = columns[1][1].id;
        // Slices 0..8 and 8..10: stone twice beats dirt once, and air never
        // counts, so the hole in column 3 doesn't open a hole in the merge.
        assert_eq!(
            out,
            [
                Point {
                    id: columns[0][0].id,
                    height: 90,
                    bottom: 10,
                    light: 0x0F
                },
                Point {
                    id: stone,
                    height: 10,
                    bottom: 0,
                    light: 2
                },
            ]
        );
        // A tie goes to the earliest column.
        let ids = [dirt, stone, stone, dirt];
        assert_eq!(most_common(ids, &registry), dirt);
    }

    #[test]
    fn equal_columns_merge_like_the_slow_path() {
        let mut registry = Registry::new();
        // The snow split leaves two stone runs that the merge joins again.
        let column = [
            point(&mut registry, "air", 300, 20, 0x0F),
            point(&mut registry, "snow", 1, 19, 0x0F),
            point(&mut registry, "stone", 1, 18, 0x0E),
            point(&mut registry, "stone", 8, 10, 0x0E),
            point(&mut registry, "dirt", 10, 0, 0),
        ];
        let (mut fast, mut slow) = (Vec::new(), Vec::new());
        merge_columns([&column; 4], &registry, &mut fast);
        merge_slices([&column; 4], &registry, &mut slow);
        assert_eq!(fast, slow);
        assert_eq!(fast.len(), 4);
        assert_eq!((fast[2].height, fast[2].bottom), (9, 10));
    }

    /// DH's merge as written: sort every boundary, sample each slice's middle.
    fn dh_merge(columns: [&[Point]; 4], registry: &Registry) -> Vec<Point> {
        let mut transitions: Vec<i32> = columns
            .iter()
            .copied()
            .flatten()
            .flat_map(|p| [p.bottom as i32, p.bottom as i32 + p.height as i32])
            .collect();
        transitions.sort_unstable();
        transitions.dedup();
        let mut cursors = columns.map(|c| c.len() as isize - 1);
        let mut out: Vec<Point> = Vec::new();
        for slice in transitions.windows(2) {
            let (low, height) = (slice[0], slice[1] - slice[0]);
            let sample = low + height / 2;
            let (mut ids, mut lights) = ([0u32; 4], [0u8; 4]);
            for (i, column) in columns.iter().enumerate() {
                while cursors[i] >= 0 {
                    let p = column[cursors[i] as usize];
                    if sample >= p.bottom as i32 + p.height as i32 {
                        cursors[i] -= 1;
                        continue;
                    }
                    if sample >= p.bottom as i32 {
                        ids[i] = p.id;
                        lights[i] = p.light;
                    }
                    break;
                }
            }
            let id = most_common(ids, registry);
            let light = (average(lights.map(|l| l >> 4), ids, id) << 4)
                | average(lights.map(|l| l & 0x0F), ids, id);
            match out.last_mut() {
                Some(run) if run.id == id && run.light == light => run.height += height as u16,
                _ => out.push(Point {
                    id,
                    height: height as u16,
                    bottom: low as u16,
                    light,
                }),
            }
        }
        out.reverse();
        out
    }

    #[test]
    fn the_sweep_merges_like_dh() {
        let mut registry = Registry::new();
        let kinds = ["air", "stone", "dirt", "water", "oak_leaves"]
            .map(|name| point(&mut registry, name, 1, 0, 0).id);
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % n
        };
        for _ in 0..2000 {
            let columns: [Vec<Point>; 4] = std::array::from_fn(|_| {
                if next(6) == 0 {
                    return Vec::new();
                }
                // Contiguous runs from a random floor up to a shared ceiling.
                let mut y = next(40) as u16;
                let mut runs = Vec::new();
                while y < 100 {
                    let height = (1 + next(12) as u16).min(100 - y);
                    runs.push(Point {
                        id: kinds[next(5) as usize],
                        height,
                        bottom: y,
                        light: next(3) as u8 * 7,
                    });
                    y += height;
                }
                runs.reverse();
                runs
            });
            let refs = columns.each_ref().map(|c| c.as_slice());
            let mut out = Vec::new();
            merge_slices(refs, &registry, &mut out);
            assert_eq!(out, dh_merge(refs, &registry), "{columns:?}");
        }
    }

    #[test]
    fn a_column_missing_from_the_merge_reads_as_air() {
        let mut registry = Registry::new();
        let stone = point(&mut registry, "stone", 4, 0, 0);
        let mut out = Vec::new();
        merge_columns([&[stone], &[], &[], &[]], &registry, &mut out);
        assert_eq!(out, [stone]);
        merge_columns([&[], &[], &[], &[]], &registry, &mut out);
        assert!(out.is_empty());
    }

    /// A full region in Morton order writes 64 + 16 + 4 + 1 rows, and only
    /// level 3 asks DH to propagate further.
    #[test]
    fn a_region_writes_every_level_once() {
        let dir = tempfile::tempdir().unwrap();
        let writer = DhWriter::create(dir.path()).unwrap();
        let mut lod = RegionLod::new(&writer, -64, 320);
        let sections = [uniform(-4, "bedrock"), uniform(-3, "stone")];
        for column in 0..256usize {
            let (sx, sz) = crate::world_editor::java::morton_16(column);
            for dz in 0..2 {
                for dx in 0..2 {
                    let chunk_x = 32 + sx * 2 + dx;
                    let chunk_z = -32 + sz * 2 + dz;
                    lod.ingest_chunk(chunk_x, chunk_z, &sections, (-4, 19), None, &PLAINS);
                }
            }
            lod.end_column(column);
        }
        lod.finish();
        assert_eq!(lod.sections_written, 85);
        drop(lod);
        writer.finish().unwrap();

        let db = rusqlite::Connection::open(dir.path().join("data").join(super::super::DB_NAME))
            .unwrap();
        let rows: Vec<(u8, i32, i32, bool)> = db
            .prepare("SELECT DetailLevel, PosX, PosZ, ApplyToParent FROM FullData ORDER BY DetailLevel, PosX, PosZ")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let count = |level| rows.iter().filter(|r| r.0 == level).count();
        assert_eq!([count(0), count(1), count(2), count(3)], [64, 16, 4, 1]);
        assert_eq!(
            rows.iter().find(|r| r.0 == 0).map(|r| (r.1, r.2)),
            Some((8, -8))
        );
        assert_eq!(rows.last().map(|r| (r.1, r.2, r.3)), Some((1, -1, true)));
        assert!(rows.iter().filter(|r| r.0 < 3).all(|r| !r.3));

        // The level-3 row is the region merged down: bedrock under stone,
        // every column complete.
        let (data, gen_steps, mapping): (Vec<u8>, Vec<u8>, Vec<u8>) = db
            .query_row(
                "SELECT Data, ColumnGenerationStep, Mapping FROM FullData WHERE DetailLevel = 3",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert!(decompress(&gen_steps).iter().all(|&s| s == GEN_STEP_LIGHT));
        let mapping = read_mapping(&decompress(&mapping));
        let mut columns = vec![Vec::new(); COLUMNS];
        read_columns(&decompress(&data), &mut columns, 1..63, 1..63);
        let serials: Vec<_> = columns[index(20, 20)]
            .iter()
            .map(|&(id, height, bottom, _)| (mapping[id as usize].clone(), height, bottom))
            .collect();
        let expected: [(&str, u16, u16); 3] = [
            ("minecraft:plains_DH-BSW_AIR", 352, 32),
            ("minecraft:plains_DH-BSW_minecraft:stone", 16, 16),
            ("minecraft:plains_DH-BSW_minecraft:bedrock", 16, 0),
        ];
        assert_eq!(serials, expected.map(|(s, h, b)| (s.to_string(), h, b)));
    }
}
