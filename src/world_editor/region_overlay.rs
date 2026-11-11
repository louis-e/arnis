//! Prototype for overlaying sparse Arnis feature chunks onto WorldPainter terrain.
//!
//! The feature region must contain only intended feature blocks; non-air source
//! blocks replace the destination, while source air never erases terrain.

use super::common::{Blockstates, Chunk, PaletteItem, Section};
use fastanvil::Region;
use fastnbt::{LongArray, Value};
use fnv::FnvHashMap;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::Path;

const SECTION_BLOCKS: usize = 16 * 16 * 16;
pub(crate) const SPARSE_GROUND_HEIGHTS_KEY: &str = "ArnisGroundHeights";
pub(crate) const SPARSE_ROAD_SURFACE_HEIGHTS_KEY: &str = "ArnisRoadSurfaceHeights";

/// Overlay each populated feature chunk in `features_path` onto a terrain region.
/// Returns the number of non-air feature blocks written.
pub(crate) fn merge_feature_region(
    terrain_path: &Path,
    features_path: &Path,
) -> Result<usize, String> {
    if terrain_path == features_path {
        return Err("terrain and feature region paths must be different".to_string());
    }

    let terrain_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(terrain_path)
        .map_err(|error| format!("open terrain region {}: {error}", terrain_path.display()))?;
    let features_file = File::open(features_path)
        .map_err(|error| format!("open feature region {}: {error}", features_path.display()))?;
    let mut terrain = Region::from_stream(terrain_file)
        .map_err(|error| format!("read terrain region {}: {error}", terrain_path.display()))?;
    let mut features = Region::from_stream(features_file)
        .map_err(|error| format!("read feature region {}: {error}", features_path.display()))?;

    let mut merged_chunks = Vec::new();
    let mut applied = 0;
    for z in 0..32 {
        for x in 0..32 {
            let Some(feature_bytes) = features
                .read_chunk(x, z)
                .map_err(|error| format!("read feature chunk ({x}, {z}): {error}"))?
            else {
                continue;
            };
            let Some(terrain_bytes) = terrain
                .read_chunk(x, z)
                .map_err(|error| format!("read terrain chunk ({x}, {z}): {error}"))?
            else {
                return Err(format!(
                    "feature chunk ({x}, {z}) has no matching WorldPainter terrain chunk"
                ));
            };

            let (merged, count) = merge_chunk_bytes(&terrain_bytes, &feature_bytes)
                .map_err(|error| format!("merge chunk ({x}, {z}): {error}"))?;
            merged_chunks.push((x, z, merged));
            applied += count;
        }
    }
    for (x, z, merged) in merged_chunks {
        terrain
            .write_chunk(x, z, &merged)
            .map_err(|error| format!("write merged chunk ({x}, {z}): {error}"))?;
    }
    Ok(applied)
}

pub(crate) fn validate_ground_metadata(features_path: &Path) -> Result<(), String> {
    let features_file = File::open(features_path)
        .map_err(|error| format!("open feature region {}: {error}", features_path.display()))?;
    let mut features = Region::from_stream(features_file)
        .map_err(|error| format!("read feature region {}: {error}", features_path.display()))?;
    for z in 0..32 {
        for x in 0..32 {
            let Some(bytes) = features
                .read_chunk(x, z)
                .map_err(|error| format!("read feature chunk ({x}, {z}): {error}"))?
            else {
                continue;
            };
            let chunk: Chunk = fastnbt::from_bytes(&bytes)
                .map_err(|error| format!("decode feature chunk ({x}, {z}): {error}"))?;
            match chunk.other.get(SPARSE_GROUND_HEIGHTS_KEY) {
                Some(Value::IntArray(heights)) if heights.len() == 256 => {}
                Some(Value::IntArray(heights)) => {
                    return Err(format!(
                        "feature chunk ({}, {}) has {} ground heights; expected 256",
                        chunk.x_pos,
                        chunk.z_pos,
                        heights.len()
                    ));
                }
                Some(_) => {
                    return Err(format!(
                        "feature chunk ({}, {}) has invalid ground height data",
                        chunk.x_pos, chunk.z_pos
                    ));
                }
                None => {
                    return Err(format!(
                        "feature chunk ({}, {}) is missing ground height data",
                        chunk.x_pos, chunk.z_pos
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn ground_surface_at(
    world_path: &Path,
    block_x: i32,
    block_z: i32,
) -> Result<i32, String> {
    let chunk_x = block_x.div_euclid(16);
    let chunk_z = block_z.div_euclid(16);
    let region_x = chunk_x.div_euclid(32);
    let region_z = chunk_z.div_euclid(32);
    let region_path = world_path
        .join("region")
        .join(format!("r.{region_x}.{region_z}.mca"));
    let file = File::open(&region_path)
        .map_err(|error| format!("open spawn region {}: {error}", region_path.display()))?;
    let mut region = Region::from_stream(file)
        .map_err(|error| format!("read spawn region {}: {error}", region_path.display()))?;
    let local_chunk_x = chunk_x.rem_euclid(32) as usize;
    let local_chunk_z = chunk_z.rem_euclid(32) as usize;
    let bytes = region
        .read_chunk(local_chunk_x, local_chunk_z)
        .map_err(|error| format!("read spawn chunk ({chunk_x}, {chunk_z}): {error}"))?
        .ok_or_else(|| format!("WorldPainter export has no spawn chunk ({chunk_x}, {chunk_z})"))?;
    let chunk: Chunk = fastnbt::from_bytes(&bytes)
        .map_err(|error| format!("decode spawn chunk ({chunk_x}, {chunk_z}): {error}"))?;
    if (chunk.x_pos, chunk.z_pos) != (chunk_x, chunk_z) {
        return Err(format!(
            "spawn chunk coordinates differ: expected ({chunk_x}, {chunk_z}), got ({}, {})",
            chunk.x_pos, chunk.z_pos
        ));
    }
    let column = block_z.rem_euclid(16) as usize * 16 + block_x.rem_euclid(16) as usize;
    ground_surface_by_column(&chunk.sections)?[column]
        .ok_or_else(|| format!("WorldPainter spawn column ({block_x}, {block_z}) has no ground"))
}

fn merge_chunk_bytes(
    terrain_bytes: &[u8],
    feature_bytes: &[u8],
) -> Result<(Vec<u8>, usize), String> {
    let mut terrain: Chunk = fastnbt::from_bytes(terrain_bytes)
        .map_err(|error| format!("decode WorldPainter chunk: {error}"))?;
    let features: Chunk = fastnbt::from_bytes(feature_bytes)
        .map_err(|error| format!("decode Arnis feature chunk: {error}"))?;
    if (terrain.x_pos, terrain.z_pos) != (features.x_pos, features.z_pos) {
        return Err(format!(
            "chunk coordinates differ: terrain ({}, {}), features ({}, {})",
            terrain.x_pos, terrain.z_pos, features.x_pos, features.z_pos
        ));
    }

    let source_ground = match features.other.get(SPARSE_GROUND_HEIGHTS_KEY) {
        Some(Value::IntArray(heights)) if heights.len() == 256 => {
            Some(heights.iter().copied().collect::<Vec<_>>())
        }
        Some(Value::IntArray(heights)) => {
            return Err(format!(
                "feature ground data has {} columns; expected 256",
                heights.len()
            ));
        }
        Some(_) => return Err("feature ground data is not an integer array".to_string()),
        None => None,
    };
    let road_surface_heights = match features.other.get(SPARSE_ROAD_SURFACE_HEIGHTS_KEY) {
        Some(Value::IntArray(heights)) if heights.len() == 256 => {
            heights.iter().copied().collect::<Vec<_>>()
        }
        Some(Value::IntArray(heights)) => {
            return Err(format!(
                "feature road surface data has {} columns; expected 256",
                heights.len()
            ));
        }
        Some(_) => return Err("feature road surface data is not an integer array".to_string()),
        None => vec![i32::MIN; 256],
    };
    let mut shifts = [0; 256];
    if let Some(source_ground) = &source_ground {
        let target_ground = ground_surface_by_column(&terrain.sections)?;
        for column in 0..256 {
            let Some(target_y) = target_ground[column] else {
                return Err(format!(
                    "WorldPainter chunk ({}, {}) has no ground surface in column ({}, {})",
                    terrain.x_pos,
                    terrain.z_pos,
                    column % 16,
                    column / 16
                ));
            };
            shifts[column] = target_y - source_ground[column];
        }
    }

    for terrain_section in &mut terrain.sections {
        terrain_section.other.remove("SkyLight");
        terrain_section.other.remove("BlockLight");
    }

    let mut edits = BTreeMap::<i8, Vec<(usize, PaletteItem)>>::new();
    let mut feature_positions = std::collections::HashSet::new();
    for feature_section in &features.sections {
        let feature_blocks = decode_palette(&feature_section.block_states)?;
        for (index, &feature_palette_index) in feature_blocks.iter().enumerate() {
            let feature_state = feature_section
                .block_states
                .palette
                .get(feature_palette_index)
                .ok_or_else(|| {
                    format!("feature palette index {feature_palette_index} is invalid")
                })?;
            if is_air(&feature_state.name) {
                continue;
            }

            let column = index % 256;
            let local_y = index / 256;
            let output_y = i32::from(feature_section.y) * 16 + local_y as i32 + shifts[column];
            feature_positions.insert((column, output_y));
            let output_section_y = output_y.div_euclid(16);
            let output_section_y = i8::try_from(output_section_y).map_err(|_| {
                format!("aligned feature block Y {output_y} is outside the supported chunk range")
            })?;
            let output_index = output_y.rem_euclid(16) as usize * 256 + column;
            edits
                .entry(output_section_y)
                .or_default()
                .push((output_index, feature_state.clone()));
        }
    }

    let air = PaletteItem {
        name: "minecraft:air".to_string(),
        properties: None,
    };
    for (column, &source_road_y) in road_surface_heights.iter().enumerate() {
        if source_road_y == i32::MIN {
            continue;
        }
        let road_y = source_road_y + shifts[column];
        for plant_y in road_y + 1..=road_y + 2 {
            if feature_positions.contains(&(column, plant_y)) {
                continue;
            }
            let section_y = i8::try_from(plant_y.div_euclid(16)).map_err(|_| {
                format!("aligned road plant Y {plant_y} is outside the supported chunk range")
            })?;
            let local_y = plant_y.rem_euclid(16) as usize;
            let Some(section) = terrain
                .sections
                .iter()
                .find(|section| section.y == section_y)
            else {
                continue;
            };
            let blocks = decode_palette(&section.block_states)?;
            let block_index = local_y * 256 + column;
            let palette_index = blocks[block_index];
            if is_road_plant(&section.block_states.palette[palette_index].name) {
                edits
                    .entry(section_y)
                    .or_default()
                    .push((block_index, air.clone()));
            }
        }
    }

    let mut applied = 0;
    for (section_y, section_edits) in edits {
        let terrain_index = terrain
            .sections
            .iter()
            .position(|section| section.y == section_y);
        let terrain_section = match terrain_index {
            Some(index) => &mut terrain.sections[index],
            None => {
                let biomes = terrain
                    .sections
                    .iter()
                    .min_by_key(|section| (i16::from(section.y) - i16::from(section_y)).abs())
                    .and_then(|section| section.other.get("biomes").cloned());
                terrain.sections.push(air_section(section_y, biomes));
                terrain
                    .sections
                    .last_mut()
                    .expect("section was just inserted")
            }
        };
        let mut terrain_blocks = decode_palette(&terrain_section.block_states)?;

        for (index, feature_state) in section_edits {
            let target_palette_index = terrain_section
                .block_states
                .palette
                .iter()
                .position(|state| same_state(state, &feature_state))
                .unwrap_or_else(|| {
                    terrain_section.block_states.palette.push(feature_state);
                    terrain_section.block_states.palette.len() - 1
                });
            terrain_blocks[index] = target_palette_index;
            applied += 1;
        }

        encode_palette(&mut terrain_section.block_states, &terrain_blocks)?;
    }

    terrain.sections.sort_by_key(|section| section.y);
    terrain.is_light_on = 0;
    let (min_section_y, max_section_y) = super::world_section_range();
    let total_height = ((i32::from(max_section_y) + 1) - i32::from(min_section_y)) * 16;
    terrain.other.insert(
        "Heightmaps".to_string(),
        super::java::compute_heightmaps(&terrain.sections, min_section_y, total_height),
    );
    merge_chunk_list(
        &mut terrain.other,
        &features.other,
        "block_entities",
        true,
        &shifts,
        features.x_pos,
        features.z_pos,
    )
    .map_err(|error| format!("merge feature block entities: {error}"))?;
    merge_chunk_list(
        &mut terrain.other,
        &features.other,
        "entities",
        false,
        &shifts,
        features.x_pos,
        features.z_pos,
    )
    .map_err(|error| format!("merge feature entities: {error}"))?;
    let bytes =
        fastnbt::to_bytes(&terrain).map_err(|error| format!("encode merged chunk: {error}"))?;
    Ok((bytes, applied))
}

fn merge_chunk_list(
    destination: &mut FnvHashMap<String, Value>,
    source: &FnvHashMap<String, Value>,
    key: &str,
    replace_same_block_entity: bool,
    shifts: &[i32; 256],
    chunk_x: i32,
    chunk_z: i32,
) -> Result<(), String> {
    let Some(source_value) = source.get(key) else {
        return Ok(());
    };
    let Value::List(source_values) = source_value else {
        return Err(format!("source chunk field {key} is not a list"));
    };
    let destination_value = destination
        .entry(key.to_string())
        .or_insert_with(|| Value::List(Vec::new()));
    let Value::List(destination_values) = destination_value else {
        return Err(format!("terrain chunk field {key} is not a list"));
    };

    for source_value in source_values {
        let mut shifted_value = source_value.clone();
        shift_feature_position(&mut shifted_value, key, shifts, chunk_x, chunk_z);
        if replace_same_block_entity {
            if let Some(position) = block_entity_position(&shifted_value) {
                if let Some(index) = destination_values
                    .iter()
                    .position(|value| block_entity_position(value) == Some(position))
                {
                    destination_values[index] = shifted_value;
                    continue;
                }
            }
        }
        if !destination_values.contains(&shifted_value) {
            destination_values.push(shifted_value);
        }
    }
    Ok(())
}

fn shift_feature_position(
    value: &mut Value,
    key: &str,
    shifts: &[i32; 256],
    chunk_x: i32,
    chunk_z: i32,
) {
    let Value::Compound(compound) = value else {
        return;
    };
    if key == "block_entities" {
        let column = match (compound.get("x"), compound.get("z")) {
            (Some(Value::Int(x)), Some(Value::Int(z))) => {
                (*z - chunk_z * 16).rem_euclid(16) as usize * 16
                    + (*x - chunk_x * 16).rem_euclid(16) as usize
            }
            _ => return,
        };
        if let Some(Value::Int(y)) = compound.get_mut("y") {
            *y += shifts[column];
        }
    } else if key == "entities" {
        let Some(Value::List(position)) = compound.get_mut("Pos") else {
            return;
        };
        let column = match (position.first(), position.get(2)) {
            (Some(Value::Double(x)), Some(Value::Double(z))) => {
                (z.floor() as i32 - chunk_z * 16).rem_euclid(16) as usize * 16
                    + (x.floor() as i32 - chunk_x * 16).rem_euclid(16) as usize
            }
            _ => return,
        };
        if let Some(Value::Double(y)) = position.get_mut(1) {
            *y += f64::from(shifts[column]);
        }
    }
}

fn ground_surface_by_column(sections: &[Section]) -> Result<[Option<i32>; 256], String> {
    let mut ground = [None; 256];
    let mut ordered: Vec<_> = sections.iter().collect();
    ordered.sort_by_key(|section| std::cmp::Reverse(section.y));
    for section in ordered {
        let blocks = decode_palette(&section.block_states)?;
        for local_y in (0..16).rev() {
            for column in 0..256 {
                if ground[column].is_some() {
                    continue;
                }
                let palette_index = blocks[local_y * 256 + column];
                let name = &section.block_states.palette[palette_index].name;
                if !is_air(name) && !is_surface_vegetation(name) {
                    ground[column] = Some(i32::from(section.y) * 16 + local_y as i32);
                }
            }
        }
    }
    Ok(ground)
}

fn is_surface_vegetation(name: &str) -> bool {
    is_road_plant(name)
        || matches!(
            name,
            "minecraft:grass"
                | "minecraft:tall_grass"
                | "minecraft:short_grass"
                | "minecraft:fern"
                | "minecraft:large_fern"
                | "minecraft:dead_bush"
                | "minecraft:sweet_berry_bush"
                | "minecraft:vine"
                | "minecraft:hanging_roots"
                | "minecraft:glow_lichen"
                | "minecraft:lichen"
                | "minecraft:snow"
                | "minecraft:snow_layer"
                | "minecraft:seagrass"
                | "minecraft:tall_seagrass"
                | "minecraft:kelp"
                | "minecraft:kelp_plant"
        )
        || [
            "_leaves", "_sapling", "_flower", "_tulip", "_log", "_wood", "_stem", "_hyphae",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

fn is_road_plant(name: &str) -> bool {
    matches!(
        name,
        "minecraft:short_grass"
            | "minecraft:grass"
            | "minecraft:tall_grass"
            | "minecraft:fern"
            | "minecraft:large_fern"
            | "minecraft:dead_bush"
            | "minecraft:vine"
            | "minecraft:hanging_roots"
            | "minecraft:glow_lichen"
            | "minecraft:lichen"
            | "minecraft:snow"
            | "minecraft:snow_layer"
            | "minecraft:seagrass"
            | "minecraft:tall_seagrass"
            | "minecraft:kelp"
            | "minecraft:kelp_plant"
            | "minecraft:poppy"
            | "minecraft:dandelion"
            | "minecraft:blue_orchid"
            | "minecraft:allium"
            | "minecraft:azure_bluet"
            | "minecraft:red_tulip"
            | "minecraft:orange_tulip"
            | "minecraft:white_tulip"
            | "minecraft:pink_tulip"
            | "minecraft:oxeye_daisy"
            | "minecraft:cornflower"
            | "minecraft:lily_of_the_valley"
            | "minecraft:sunflower"
            | "minecraft:lilac"
            | "minecraft:rose_bush"
            | "minecraft:peony"
            | "minecraft:brown_mushroom"
            | "minecraft:red_mushroom"
            | "minecraft:moss_carpet"
            | "minecraft:sweet_berry_bush"
    )
}

fn block_entity_position(value: &Value) -> Option<(i32, i32, i32)> {
    let Value::Compound(compound) = value else {
        return None;
    };
    let (Some(Value::Int(x)), Some(Value::Int(y)), Some(Value::Int(z))) =
        (compound.get("x"), compound.get("y"), compound.get("z"))
    else {
        return None;
    };
    Some((*x, *y, *z))
}

fn air_section(y: i8, biomes: Option<Value>) -> Section {
    let other = biomes
        .map(|biomes| FnvHashMap::from_iter([("biomes".to_string(), biomes)]))
        .unwrap_or_default();
    Section {
        block_states: Blockstates {
            palette: vec![PaletteItem {
                name: "minecraft:air".to_string(),
                properties: None,
            }],
            data: None,
            other: FnvHashMap::default(),
        },
        y,
        other,
    }
}

fn decode_palette(block_states: &Blockstates) -> Result<Vec<usize>, String> {
    if block_states.palette.is_empty() {
        return Err("block-state palette is empty".to_string());
    }
    let Some(data) = &block_states.data else {
        return Ok(vec![0; SECTION_BLOCKS]);
    };

    let bits = bits_per_block(block_states.palette.len());
    if bits > 32 {
        return Err(format!(
            "block-state palette is too large: {}",
            block_states.palette.len()
        ));
    }
    let values_per_long = 64 / bits;
    let expected_longs = SECTION_BLOCKS.div_ceil(values_per_long);
    if data.len() < expected_longs {
        return Err(format!(
            "block-state data has {} longs; expected at least {expected_longs}",
            data.len()
        ));
    }
    let mask = (1_u64 << bits) - 1;
    let mut blocks = Vec::with_capacity(SECTION_BLOCKS);
    for index in 0..SECTION_BLOCKS {
        let long_index = index / values_per_long;
        let bit_offset = (index % values_per_long) * bits;
        let palette_index = ((data[long_index] as u64 >> bit_offset) & mask) as usize;
        if palette_index >= block_states.palette.len() {
            return Err(format!(
                "block-state palette index {palette_index} is invalid"
            ));
        }
        blocks.push(palette_index);
    }
    Ok(blocks)
}

fn encode_palette(block_states: &mut Blockstates, blocks: &[usize]) -> Result<(), String> {
    if blocks.len() != SECTION_BLOCKS || block_states.palette.is_empty() {
        return Err("cannot encode an incomplete block-state section".to_string());
    }
    if block_states.palette.len() == 1 {
        block_states.data = None;
        return Ok(());
    }

    let bits = bits_per_block(block_states.palette.len());
    if bits > 32 {
        return Err(format!(
            "block-state palette is too large: {}",
            block_states.palette.len()
        ));
    }
    let values_per_long = 64 / bits;
    let mask = (1_u64 << bits) - 1;
    let mut data = vec![0_u64; SECTION_BLOCKS.div_ceil(values_per_long)];
    for (index, &palette_index) in blocks.iter().enumerate() {
        if palette_index >= block_states.palette.len() || palette_index as u64 > mask {
            return Err(format!(
                "block-state palette index {palette_index} is invalid"
            ));
        }
        let long_index = index / values_per_long;
        let bit_offset = (index % values_per_long) * bits;
        data[long_index] |= (palette_index as u64) << bit_offset;
    }
    block_states.data = Some(LongArray::new(
        data.into_iter().map(|value| value as i64).collect(),
    ));
    Ok(())
}

fn bits_per_block(palette_len: usize) -> usize {
    (usize::BITS as usize - palette_len.saturating_sub(1).leading_zeros() as usize).max(4)
}

fn is_air(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "air"
            | "cave_air"
            | "void_air"
    )
}

fn same_state(left: &PaletteItem, right: &PaletteItem) -> bool {
    left.name == right.name && left.properties == right.properties
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastnbt::Value;
    use std::collections::HashMap;
    use std::fs::OpenOptions;

    fn palette_item(name: &str) -> PaletteItem {
        PaletteItem {
            name: format!("minecraft:{name}"),
            properties: None,
        }
    }

    fn section(y: i8, palette: Vec<PaletteItem>, blocks: &[usize]) -> Section {
        let mut block_states = Blockstates {
            palette,
            data: None,
            other: FnvHashMap::default(),
        };
        encode_palette(&mut block_states, blocks).unwrap();
        Section {
            block_states,
            y,
            other: FnvHashMap::from_iter([("biomes".to_string(), Value::Compound(HashMap::new()))]),
        }
    }

    fn chunk(x: i32, z: i32, sections: Vec<Section>) -> Chunk {
        Chunk {
            sections,
            x_pos: x,
            z_pos: z,
            is_light_on: 0,
            other: FnvHashMap::from_iter([(
                "Heightmaps".to_string(),
                Value::Compound(HashMap::from([(
                    "WORLD_SURFACE".to_string(),
                    Value::LongArray(LongArray::new(vec![123])),
                )])),
            )]),
        }
    }

    fn write_region(path: &Path, chunk: &Chunk, slot_x: usize) {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .unwrap();
        let mut region = Region::create(file).unwrap();
        let bytes = fastnbt::to_bytes(chunk).unwrap();
        region.write_chunk(slot_x, 0, &bytes).unwrap();
    }

    #[test]
    fn spawn_surface_ignores_surface_vegetation() {
        let temp = tempfile::tempdir().unwrap();
        let region_dir = temp.path().join("region");
        std::fs::create_dir_all(&region_dir).unwrap();
        let mut blocks = vec![0; SECTION_BLOCKS];
        blocks[3 * 256] = 1;
        blocks[4 * 256] = 2;
        let terrain = chunk(
            0,
            0,
            vec![section(
                4,
                vec![
                    palette_item("air"),
                    palette_item("stone"),
                    palette_item("grass"),
                ],
                &blocks,
            )],
        );
        write_region(&region_dir.join("r.0.0.mca"), &terrain, 0);

        assert_eq!(ground_surface_at(temp.path(), 0, 0).unwrap(), 67);
    }

    fn state_at(section: &Section, x: usize, y: usize, z: usize) -> String {
        let blocks = decode_palette(&section.block_states).unwrap();
        let index = y * 256 + z * 16 + x;
        section.block_states.palette[blocks[index]].name.clone()
    }

    #[test]
    fn sparse_feature_region_preserves_terrain_and_overlays_blocks() {
        let temp = tempfile::tempdir().unwrap();
        let terrain_path = temp.path().join("terrain.mca");
        let features_path = temp.path().join("features.mca");

        let mut terrain_blocks = vec![0; SECTION_BLOCKS];
        terrain_blocks[0] = 1;
        terrain_blocks[9 * 16 + 9] = 1;
        let mut terrain = chunk(
            0,
            0,
            vec![section(
                4,
                vec![palette_item("air"), palette_item("grass_block")],
                &terrain_blocks,
            )],
        );
        terrain.is_light_on = 1;
        terrain.sections[0].other.insert(
            "SkyLight".to_string(),
            Value::ByteArray(fastnbt::ByteArray::new(vec![15; 2048])),
        );
        terrain.sections[0].other.insert(
            "BlockLight".to_string(),
            Value::ByteArray(fastnbt::ByteArray::new(vec![0; 2048])),
        );

        let mut feature_blocks = vec![0; SECTION_BLOCKS];
        feature_blocks[1 * 256 + 8 * 16 + 8] = 1;
        feature_blocks[1 * 256 + 8 * 16 + 9] = 2;
        let features = chunk(
            0,
            0,
            vec![section(
                4,
                vec![
                    palette_item("air"),
                    palette_item("smooth_stone"),
                    palette_item("white_concrete"),
                ],
                &feature_blocks,
            )],
        );
        write_region(&terrain_path, &terrain, 0);
        write_region(&features_path, &features, 0);

        assert_eq!(
            merge_feature_region(&terrain_path, &features_path).unwrap(),
            2
        );

        let file = File::open(&terrain_path).unwrap();
        let mut region = Region::from_stream(file).unwrap();
        let bytes = region.read_chunk(0, 0).unwrap().unwrap();
        let merged: Chunk = fastnbt::from_bytes(&bytes).unwrap();
        let merged_section = &merged.sections[0];
        assert_eq!(state_at(merged_section, 8, 1, 8), "minecraft:smooth_stone");
        assert_eq!(
            state_at(merged_section, 9, 1, 8),
            "minecraft:white_concrete"
        );
        assert_eq!(state_at(merged_section, 0, 0, 0), "minecraft:grass_block");
        assert_eq!(state_at(merged_section, 9, 0, 9), "minecraft:grass_block");
        let Some(Value::Compound(heightmaps)) = merged.other.get("Heightmaps") else {
            panic!("merged chunk has no heightmaps");
        };
        let Some(Value::LongArray(world_surface)) = heightmaps.get("WORLD_SURFACE") else {
            panic!("merged chunk has no world surface heightmap");
        };
        assert_eq!(world_surface.len(), 37);
        let heightmap_index = 8 * 16 + 8;
        let heightmap_value = ((world_surface[heightmap_index / 7] as u64
            >> ((heightmap_index % 7) * 9))
            & 0x1ff) as i32;
        assert_eq!(heightmap_value, 130);
        assert_ne!(
            merged.other.get("Heightmaps"),
            terrain.other.get("Heightmaps")
        );
        assert_eq!(merged.is_light_on, 0);
        assert!(!merged_section.other.contains_key("SkyLight"));
        assert!(!merged_section.other.contains_key("BlockLight"));
        assert_eq!(
            merged_section.other.get("biomes"),
            terrain.sections[0].other.get("biomes")
        );
    }

    #[test]
    fn sparse_feature_blocks_align_to_worldpainter_ground_height() {
        let terrain_blocks = vec![1; SECTION_BLOCKS];
        let terrain = chunk(
            0,
            0,
            vec![section(4, vec![palette_item("stone")], &terrain_blocks)],
        );
        let mut feature_blocks = vec![0; SECTION_BLOCKS];
        feature_blocks[1 * 256 + 1 * 16 + 1] = 1;
        let mut features = chunk(
            0,
            0,
            vec![section(
                4,
                vec![palette_item("air"), palette_item("smooth_stone")],
                &feature_blocks,
            )],
        );
        features.other.insert(
            SPARSE_GROUND_HEIGHTS_KEY.to_string(),
            Value::IntArray(fastnbt::IntArray::new(vec![60; 256])),
        );

        let terrain_bytes = fastnbt::to_bytes(&terrain).unwrap();
        let feature_bytes = fastnbt::to_bytes(&features).unwrap();
        let (merged_bytes, applied) = merge_chunk_bytes(&terrain_bytes, &feature_bytes).unwrap();
        let merged: Chunk = fastnbt::from_bytes(&merged_bytes).unwrap();
        let aligned_section = merged
            .sections
            .iter()
            .find(|section| section.y == 5)
            .unwrap();
        let ground_section = merged
            .sections
            .iter()
            .find(|section| section.y == 4)
            .unwrap();

        assert_eq!(applied, 1);
        assert_eq!(state_at(aligned_section, 1, 4, 1), "minecraft:smooth_stone");
        assert_eq!(state_at(ground_section, 1, 1, 1), "minecraft:stone");
    }

    #[test]
    fn road_overlay_clears_worldpainter_plants_but_preserves_arnis_blocks() {
        let mut terrain_blocks = vec![0; SECTION_BLOCKS];
        terrain_blocks[..256].fill(1);
        terrain_blocks[0] = 1;
        terrain_blocks[256] = 2;
        terrain_blocks[512] = 3;
        terrain_blocks[1] = 1;
        terrain_blocks[257] = 2;
        let terrain = chunk(
            0,
            0,
            vec![section(
                4,
                vec![
                    palette_item("air"),
                    palette_item("stone"),
                    palette_item("tall_grass"),
                    palette_item("poppy"),
                ],
                &terrain_blocks,
            )],
        );

        let mut feature_blocks = vec![0; SECTION_BLOCKS];
        feature_blocks[12 * 256] = 1;
        feature_blocks[13 * 256] = 2;
        let mut features = chunk(
            0,
            0,
            vec![section(
                3,
                vec![
                    palette_item("air"),
                    palette_item("smooth_stone"),
                    palette_item("white_concrete"),
                ],
                &feature_blocks,
            )],
        );
        features.other.insert(
            SPARSE_GROUND_HEIGHTS_KEY.to_string(),
            Value::IntArray(fastnbt::IntArray::new(vec![60; 256])),
        );
        let mut road_heights = vec![i32::MIN; 256];
        road_heights[0] = 60;
        features.other.insert(
            SPARSE_ROAD_SURFACE_HEIGHTS_KEY.to_string(),
            Value::IntArray(fastnbt::IntArray::new(road_heights)),
        );

        let terrain_bytes = fastnbt::to_bytes(&terrain).unwrap();
        let feature_bytes = fastnbt::to_bytes(&features).unwrap();
        let (merged_bytes, applied) = merge_chunk_bytes(&terrain_bytes, &feature_bytes).unwrap();
        let merged: Chunk = fastnbt::from_bytes(&merged_bytes).unwrap();
        let surface = merged
            .sections
            .iter()
            .find(|section| section.y == 4)
            .unwrap();

        assert_eq!(applied, 3);
        assert_eq!(state_at(surface, 0, 0, 0), "minecraft:smooth_stone");
        assert_eq!(state_at(surface, 0, 1, 0), "minecraft:white_concrete");
        assert_eq!(state_at(surface, 0, 2, 0), "minecraft:air");
        assert_eq!(state_at(surface, 1, 1, 0), "minecraft:tall_grass");
    }

    #[test]
    #[ignore = "set ARNIS_SPARSE_FEATURE_REGION to a generated sparse region file"]
    fn merges_generated_sparse_region_into_controlled_terrain() {
        let features_path = std::env::var_os("ARNIS_SPARSE_FEATURE_REGION")
            .expect("ARNIS_SPARSE_FEATURE_REGION must name a generated .mca file");
        let features_path = Path::new(&features_path);
        let temp = tempfile::tempdir().unwrap();
        let terrain_path = temp.path().join("terrain.mca");
        let terrain_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&terrain_path)
            .unwrap();
        let features_file = File::open(features_path).unwrap();
        let mut features = Region::from_stream(features_file).unwrap();
        let mut terrain = Region::create(terrain_file).unwrap();
        let mut marker = None;
        let mut source_block_count = 0;
        let mut source_chunk_count = 0;

        for z in 0..32 {
            for x in 0..32 {
                let Some(bytes) = features.read_chunk(x, z).unwrap() else {
                    continue;
                };
                let source: Chunk = fastnbt::from_bytes(&bytes).unwrap();
                let mut terrain_sections = Vec::with_capacity(source.sections.len());

                for source_section in &source.sections {
                    let source_blocks = decode_palette(&source_section.block_states).unwrap();
                    let mut terrain_blocks = vec![0; SECTION_BLOCKS];
                    let mut palette = vec![palette_item("stone")];
                    if marker.is_none() {
                        'find_air: for (index, &palette_index) in source_blocks.iter().enumerate() {
                            let state = &source_section.block_states.palette[palette_index];
                            if is_air(&state.name) {
                                palette.push(palette_item("gold_block"));
                                terrain_blocks[index] = 1;
                                marker =
                                    Some((source.x_pos, source.z_pos, source_section.y, index));
                                break 'find_air;
                            }
                        }
                    }

                    let mut terrain_section = section(source_section.y, palette, &terrain_blocks);
                    if let Some(biomes) = source_section.other.get("biomes") {
                        terrain_section
                            .other
                            .insert("biomes".to_string(), biomes.clone());
                    }
                    terrain_section.other.insert(
                        "SkyLight".to_string(),
                        Value::ByteArray(fastnbt::ByteArray::new(vec![15; 2048])),
                    );
                    terrain_section.other.insert(
                        "BlockLight".to_string(),
                        Value::ByteArray(fastnbt::ByteArray::new(vec![0; 2048])),
                    );
                    source_block_count += source_blocks
                        .iter()
                        .filter(|&&palette_index| {
                            !is_air(&source_section.block_states.palette[palette_index].name)
                        })
                        .count();
                    terrain_sections.push(terrain_section);
                }

                let mut terrain_chunk = chunk(source.x_pos, source.z_pos, terrain_sections);
                terrain_chunk.is_light_on = 1;
                terrain
                    .write_chunk(x, z, &fastnbt::to_bytes(&terrain_chunk).unwrap())
                    .unwrap();
                source_chunk_count += 1;
            }
        }
        drop(terrain);
        drop(features);

        assert!(
            source_chunk_count > 0,
            "sparse region has no populated chunks"
        );
        assert!(
            source_block_count > 0,
            "sparse region has no feature blocks"
        );
        let (marker_x, marker_z, marker_y, marker_index) =
            marker.expect("sparse region has no air block for terrain preservation check");
        assert_eq!(
            merge_feature_region(&terrain_path, features_path).unwrap(),
            source_block_count
        );

        let terrain_file = File::open(&terrain_path).unwrap();
        let features_file = File::open(features_path).unwrap();
        let mut terrain = Region::from_stream(terrain_file).unwrap();
        let mut features = Region::from_stream(features_file).unwrap();
        let mut verified_feature_blocks = 0;

        for z in 0..32 {
            for x in 0..32 {
                let Some(feature_bytes) = features.read_chunk(x, z).unwrap() else {
                    continue;
                };
                let merged_bytes = terrain.read_chunk(x, z).unwrap().unwrap();
                let source: Chunk = fastnbt::from_bytes(&feature_bytes).unwrap();
                let merged: Chunk = fastnbt::from_bytes(&merged_bytes).unwrap();
                assert_eq!(merged.is_light_on, 0);
                assert!(matches!(
                    merged.other.get("Heightmaps"),
                    Some(Value::Compound(heightmaps))
                        if matches!(heightmaps.get("WORLD_SURFACE"), Some(Value::LongArray(_)))
                ));

                for source_section in &source.sections {
                    let merged_section = merged
                        .sections
                        .iter()
                        .find(|section| section.y == source_section.y)
                        .unwrap();
                    assert!(!merged_section.other.contains_key("SkyLight"));
                    assert!(!merged_section.other.contains_key("BlockLight"));
                    let source_blocks = decode_palette(&source_section.block_states).unwrap();
                    let merged_blocks = decode_palette(&merged_section.block_states).unwrap();
                    for (index, &palette_index) in source_blocks.iter().enumerate() {
                        let source_state = &source_section.block_states.palette[palette_index];
                        if is_air(&source_state.name) {
                            continue;
                        }
                        let merged_state =
                            &merged_section.block_states.palette[merged_blocks[index]];
                        assert!(same_state(source_state, merged_state));
                        verified_feature_blocks += 1;
                    }
                }

                for key in ["block_entities", "entities"] {
                    if let Some(Value::List(source_values)) = source.other.get(key) {
                        let Some(Value::List(merged_values)) = merged.other.get(key) else {
                            panic!("merged chunk lost feature {key}");
                        };
                        for value in source_values {
                            assert!(merged_values.contains(value), "merged chunk lost {key}");
                        }
                    }
                }
            }
        }
        assert_eq!(verified_feature_blocks, source_block_count);
        println!(
            "Merged {source_chunk_count} generated feature chunks and verified {verified_feature_blocks} feature blocks"
        );

        let chunk_x = marker_x.rem_euclid(32) as usize;
        let chunk_z = marker_z.rem_euclid(32) as usize;
        let marker_chunk = terrain.read_chunk(chunk_x, chunk_z).unwrap().unwrap();
        let marker_chunk: Chunk = fastnbt::from_bytes(&marker_chunk).unwrap();
        let marker_section = marker_chunk
            .sections
            .iter()
            .find(|section| section.y == marker_y)
            .unwrap();
        assert_eq!(
            state_at(
                marker_section,
                marker_index % 16,
                marker_index / 256,
                (marker_index / 16) % 16
            ),
            "minecraft:gold_block"
        );
    }

    #[test]
    #[ignore = "set ARNIS_SPARSE_FEATURE_REGION and ARNIS_WORLDPAINTER_REGION to real .mca files"]
    fn merges_generated_sparse_region_into_worldpainter_terrain_copy() {
        let features_path = std::env::var_os("ARNIS_SPARSE_FEATURE_REGION")
            .expect("ARNIS_SPARSE_FEATURE_REGION must name a generated .mca file");
        let terrain_source_path = std::env::var_os("ARNIS_WORLDPAINTER_REGION")
            .expect("ARNIS_WORLDPAINTER_REGION must name a WorldPainter .mca file");
        let features_path = Path::new(&features_path);
        let terrain_source_path = Path::new(&terrain_source_path);
        let temp = tempfile::tempdir().unwrap();
        let terrain_path = std::env::var_os("ARNIS_MERGED_REGION_OUTPUT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| temp.path().join("worldpainter-copy.mca"));
        assert_ne!(
            terrain_path, terrain_source_path,
            "merged output must not overwrite the source WorldPainter region"
        );
        assert_ne!(
            terrain_path, features_path,
            "merged output must not overwrite the Arnis feature region"
        );
        std::fs::copy(terrain_source_path, &terrain_path).unwrap();
        let features_file = File::open(features_path).unwrap();
        let mut features = Region::from_stream(features_file).unwrap();
        let terrain_file = File::open(&terrain_path).unwrap();
        let mut terrain = Region::from_stream(terrain_file).unwrap();
        let mut source_chunk_count = 0;
        let mut source_block_count = 0;
        let mut data_version_pairs = HashMap::new();
        let mut terrain_data_versions = HashMap::new();

        for z in 0..32 {
            for x in 0..32 {
                let Some(feature_bytes) = features.read_chunk(x, z).unwrap() else {
                    continue;
                };
                let terrain_bytes = terrain.read_chunk(x, z).unwrap().unwrap_or_else(|| {
                    panic!("WorldPainter region is missing feature chunk ({x}, {z})")
                });
                let source: Chunk = fastnbt::from_bytes(&feature_bytes).unwrap();
                let destination: Chunk = fastnbt::from_bytes(&terrain_bytes).unwrap();
                let version = |chunk: &Chunk| match chunk.other.get("DataVersion") {
                    Some(Value::Int(version)) => Some(*version),
                    _ => None,
                };
                *data_version_pairs
                    .entry((version(&source), version(&destination)))
                    .or_insert(0) += 1;
                terrain_data_versions.insert((x, z), version(&destination));
                source_chunk_count += 1;
                source_block_count += source
                    .sections
                    .iter()
                    .map(|section| {
                        let blocks = decode_palette(&section.block_states).unwrap();
                        blocks
                            .iter()
                            .filter(|&&palette_index| {
                                !is_air(&section.block_states.palette[palette_index].name)
                            })
                            .count()
                    })
                    .sum::<usize>();
            }
        }
        drop(terrain);
        drop(features);

        assert!(
            source_chunk_count > 0,
            "sparse region has no populated chunks"
        );
        assert!(
            source_block_count > 0,
            "sparse region has no feature blocks"
        );
        let world_path = terrain_source_path.parent().unwrap().parent().unwrap();
        let world_data_version = crate::world_utils::level_data_version(world_path);
        println!(
            "World DataVersion {world_data_version:?}; feature/terrain chunk DataVersion counts: {data_version_pairs:?}"
        );
        assert_eq!(
            merge_feature_region(&terrain_path, features_path).unwrap(),
            source_block_count
        );

        let terrain_file = File::open(&terrain_path).unwrap();
        let features_file = File::open(features_path).unwrap();
        let mut terrain = Region::from_stream(terrain_file).unwrap();
        let mut features = Region::from_stream(features_file).unwrap();
        let mut verified_feature_blocks = 0;
        for z in 0..32 {
            for x in 0..32 {
                let Some(feature_bytes) = features.read_chunk(x, z).unwrap() else {
                    continue;
                };
                let merged_bytes = terrain.read_chunk(x, z).unwrap().unwrap();
                let source: Chunk = fastnbt::from_bytes(&feature_bytes).unwrap();
                let merged: Chunk = fastnbt::from_bytes(&merged_bytes).unwrap();
                let merged_data_version = match merged.other.get("DataVersion") {
                    Some(Value::Int(version)) => Some(*version),
                    _ => None,
                };
                assert_eq!(
                    merged_data_version,
                    terrain_data_versions.get(&(x, z)).copied().flatten(),
                    "merge changed terrain chunk DataVersion at ({x}, {z})"
                );
                assert_eq!(merged.is_light_on, 0);
                assert!(matches!(
                    merged.other.get("Heightmaps"),
                    Some(Value::Compound(heightmaps))
                        if matches!(heightmaps.get("WORLD_SURFACE"), Some(Value::LongArray(_)))
                ));
                for source_section in &source.sections {
                    let merged_section = merged
                        .sections
                        .iter()
                        .find(|section| section.y == source_section.y)
                        .unwrap();
                    assert!(!merged_section.other.contains_key("SkyLight"));
                    assert!(!merged_section.other.contains_key("BlockLight"));
                    let source_blocks = decode_palette(&source_section.block_states).unwrap();
                    let merged_blocks = decode_palette(&merged_section.block_states).unwrap();
                    for (index, &palette_index) in source_blocks.iter().enumerate() {
                        let source_state = &source_section.block_states.palette[palette_index];
                        if is_air(&source_state.name) {
                            continue;
                        }
                        assert!(same_state(
                            source_state,
                            &merged_section.block_states.palette[merged_blocks[index]]
                        ));
                        verified_feature_blocks += 1;
                    }
                }
                for key in ["block_entities", "entities"] {
                    if let Some(Value::List(source_values)) = source.other.get(key) {
                        let Some(Value::List(merged_values)) = merged.other.get(key) else {
                            panic!("merged chunk lost feature {key}");
                        };
                        for value in source_values {
                            assert!(merged_values.contains(value), "merged chunk lost {key}");
                        }
                    }
                }
            }
        }
        assert_eq!(verified_feature_blocks, source_block_count);
        println!(
            "Merged {source_chunk_count} generated feature chunks and verified {verified_feature_blocks} blocks against WorldPainter terrain"
        );
        if std::env::var_os("ARNIS_MERGED_REGION_OUTPUT").is_some() {
            let merged_world_path = terrain_path.parent().unwrap().parent().unwrap();
            assert_eq!(
                crate::world_utils::level_data_version(merged_world_path),
                world_data_version,
                "merge changed level.dat DataVersion"
            );
            println!("Preserved merged test region at {}", terrain_path.display());
        }
    }

    #[test]
    fn feature_entities_and_block_entities_survive_overlay() {
        let temp = tempfile::tempdir().unwrap();
        let terrain_path = temp.path().join("terrain.mca");
        let features_path = temp.path().join("features.mca");
        let blocks = vec![0; SECTION_BLOCKS];
        let mut terrain = chunk(0, 0, vec![section(4, vec![palette_item("stone")], &blocks)]);
        let old_sign = Value::Compound(HashMap::from([
            (
                "id".to_string(),
                Value::String("minecraft:sign".to_string()),
            ),
            ("x".to_string(), Value::Int(1)),
            ("y".to_string(), Value::Int(65)),
            ("z".to_string(), Value::Int(1)),
            ("front_text".to_string(), Value::String("old".to_string())),
        ]));
        terrain
            .other
            .insert("block_entities".to_string(), Value::List(vec![old_sign]));
        let terrain_entity = Value::Compound(HashMap::from([(
            "UUID".to_string(),
            Value::String("terrain-entity".to_string()),
        )]));
        terrain.other.insert(
            "entities".to_string(),
            Value::List(vec![terrain_entity.clone()]),
        );

        let mut feature_blocks = vec![0; SECTION_BLOCKS];
        feature_blocks[1 * 256 + 1 * 16 + 1] = 1;
        let mut features = chunk(
            0,
            0,
            vec![section(
                4,
                vec![palette_item("air"), palette_item("barrel")],
                &feature_blocks,
            )],
        );
        let replacement_sign = Value::Compound(HashMap::from([
            (
                "id".to_string(),
                Value::String("minecraft:barrel".to_string()),
            ),
            ("x".to_string(), Value::Int(1)),
            ("y".to_string(), Value::Int(65)),
            ("z".to_string(), Value::Int(1)),
            ("front_text".to_string(), Value::String("new".to_string())),
        ]));
        features.other.insert(
            "block_entities".to_string(),
            Value::List(vec![replacement_sign.clone()]),
        );
        let feature_entity = Value::Compound(HashMap::from([(
            "UUID".to_string(),
            Value::String("feature-entity".to_string()),
        )]));
        features.other.insert(
            "entities".to_string(),
            Value::List(vec![feature_entity.clone()]),
        );
        write_region(&terrain_path, &terrain, 0);
        write_region(&features_path, &features, 0);

        merge_feature_region(&terrain_path, &features_path).unwrap();

        let file = File::open(&terrain_path).unwrap();
        let mut region = Region::from_stream(file).unwrap();
        let bytes = region.read_chunk(0, 0).unwrap().unwrap();
        let merged: Chunk = fastnbt::from_bytes(&bytes).unwrap();
        let Some(Value::List(block_entities)) = merged.other.get("block_entities") else {
            panic!("merged block entities are missing");
        };
        assert_eq!(block_entities, &vec![replacement_sign]);
        let Some(Value::List(entities)) = merged.other.get("entities") else {
            panic!("merged entities are missing");
        };
        assert_eq!(entities.len(), 2);
        assert!(entities.contains(&terrain_entity));
        assert!(entities.contains(&feature_entity));
    }

    #[test]
    fn feature_sections_outside_terrain_keep_the_feature_biome() {
        let temp = tempfile::tempdir().unwrap();
        let terrain_path = temp.path().join("terrain.mca");
        let features_path = temp.path().join("features.mca");
        let terrain_blocks = vec![0; SECTION_BLOCKS];
        let mut feature_blocks = vec![0; SECTION_BLOCKS];
        feature_blocks[1] = 1;
        write_region(
            &terrain_path,
            &chunk(
                0,
                0,
                vec![section(4, vec![palette_item("stone")], &terrain_blocks)],
            ),
            0,
        );
        write_region(
            &features_path,
            &chunk(
                0,
                0,
                vec![section(
                    5,
                    vec![palette_item("air"), palette_item("oak_log")],
                    &feature_blocks,
                )],
            ),
            0,
        );

        assert_eq!(
            merge_feature_region(&terrain_path, &features_path).unwrap(),
            1
        );

        let file = File::open(&terrain_path).unwrap();
        let mut region = Region::from_stream(file).unwrap();
        let bytes = region.read_chunk(0, 0).unwrap().unwrap();
        let merged: Chunk = fastnbt::from_bytes(&bytes).unwrap();
        let added_section = merged
            .sections
            .iter()
            .find(|section| section.y == 5)
            .unwrap();
        assert_eq!(state_at(added_section, 1, 0, 0), "minecraft:oak_log");
        assert_eq!(
            added_section.other.get("biomes"),
            Some(&Value::Compound(HashMap::new()))
        );
    }

    #[test]
    fn rejects_feature_chunks_without_matching_terrain() {
        let temp = tempfile::tempdir().unwrap();
        let terrain_path = temp.path().join("terrain.mca");
        let features_path = temp.path().join("features.mca");
        let blocks = vec![0; SECTION_BLOCKS];
        write_region(
            &terrain_path,
            &chunk(0, 0, vec![section(4, vec![palette_item("stone")], &blocks)]),
            0,
        );
        write_region(
            &features_path,
            &chunk(1, 0, vec![section(4, vec![palette_item("stone")], &blocks)]),
            1,
        );

        let error = merge_feature_region(&terrain_path, &features_path).unwrap_err();
        assert!(error.contains("no matching WorldPainter terrain chunk"));
    }

    #[test]
    fn validates_all_chunks_before_writing_any_overlays() {
        let temp = tempfile::tempdir().unwrap();
        let terrain_path = temp.path().join("terrain.mca");
        let features_path = temp.path().join("features.mca");
        let terrain_blocks = vec![0; SECTION_BLOCKS];
        let mut feature_blocks = vec![0; SECTION_BLOCKS];
        feature_blocks[0] = 1;
        write_region(
            &terrain_path,
            &chunk(
                0,
                0,
                vec![section(4, vec![palette_item("stone")], &terrain_blocks)],
            ),
            0,
        );

        let feature_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&features_path)
            .unwrap();
        let mut feature_region = Region::create(feature_file).unwrap();
        for (slot_x, chunk_x) in [(0, 0), (1, 1)] {
            let feature_chunk = chunk(
                chunk_x,
                0,
                vec![section(
                    4,
                    vec![palette_item("air"), palette_item("gold_block")],
                    &feature_blocks,
                )],
            );
            feature_region
                .write_chunk(slot_x, 0, &fastnbt::to_bytes(&feature_chunk).unwrap())
                .unwrap();
        }

        assert!(merge_feature_region(&terrain_path, &features_path).is_err());

        let terrain_file = File::open(&terrain_path).unwrap();
        let mut terrain_region = Region::from_stream(terrain_file).unwrap();
        let bytes = terrain_region.read_chunk(0, 0).unwrap().unwrap();
        let unchanged: Chunk = fastnbt::from_bytes(&bytes).unwrap();
        assert_eq!(state_at(&unchanged.sections[0], 0, 0, 0), "minecraft:stone");
    }
}
