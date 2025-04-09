use crate::args::Args;
use crate::block_definitions::{BEDROCK, DIRT, GRASS_BLOCK, SNOW_BLOCK, STONE};
use crate::cartesian::XZPoint;
use crate::{element_processing::*, osm_parser};
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;
use crate::progress::emit_gui_progress_update;
use crate::world_editor::WorldEditor;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use fastnbt::Value;
use flate2::read::GzDecoder;
use std::io::Read;
use std::path::PathBuf;
use std::{fs, io::Write};

pub const MIN_Y: i32 = -64;

pub fn generate_world(
    elements: Vec<ProcessedElement>,
    args: &Args,
    scale_factor_x: f64,
    scale_factor_z: f64,
) -> Result<(), String> {
    // Set spawn point     not sure where to put this
    set_spawn_point(&args);


    let region_dir: String = format!("{}/region", args.path);
    let mut editor: WorldEditor = WorldEditor::new(&region_dir, scale_factor_x, scale_factor_z);

    println!("{} Processing data...", "[3/5]".bold());
    if args.terrain {
        emit_gui_progress_update(10.0, "Fetching elevation...");
    }
    let ground: Ground = Ground::new(args);

    emit_gui_progress_update(11.0, "Processing terrain...");

    // Process data
    let elements_count: usize = elements.len();
    let process_pb: ProgressBar = ProgressBar::new(elements_count as u64);
    process_pb.set_style(ProgressStyle::default_bar()
        .template("{spinner:.green} [{elapsed_precise}] [{bar:45.white/black}] {pos}/{len} elements ({eta}) {msg}")
        .unwrap()
        .progress_chars("█▓░"));

    let progress_increment_prcs: f64 = 49.0 / elements_count as f64;
    let mut current_progress_prcs: f64 = 11.0;
    let mut last_emitted_progress: f64 = current_progress_prcs;

    for element in &elements {
        process_pb.inc(1);
        current_progress_prcs += progress_increment_prcs;
        if (current_progress_prcs - last_emitted_progress).abs() > 0.25 {
            emit_gui_progress_update(current_progress_prcs, "");
            last_emitted_progress = current_progress_prcs;
        }

        if args.debug {
            process_pb.set_message(format!(
                "(Element ID: {} / Type: {})",
                element.id(),
                element.kind()
            ));
        } else {
            process_pb.set_message("");
        }

        match element {
            ProcessedElement::Way(way) => {
                if way.tags.contains_key("building") || way.tags.contains_key("building:part") {
                    buildings::generate_buildings(&mut editor, way, &ground, args, None);
                } else if way.tags.contains_key("highway") {
                    highways::generate_highways(&mut editor, element, &ground, args);
                } else if way.tags.contains_key("landuse") {
                    landuse::generate_landuse(&mut editor, way, &ground, args);
                } else if way.tags.contains_key("natural") {
                    natural::generate_natural(&mut editor, element, &ground, args);
                } else if way.tags.contains_key("amenity") {
                    amenities::generate_amenities(&mut editor, element, &ground, args);
                } else if way.tags.contains_key("leisure") {
                    leisure::generate_leisure(&mut editor, way, &ground, args);
                } else if way.tags.contains_key("barrier") {
                    barriers::generate_barriers(&mut editor, element, &ground);
                } else if way.tags.contains_key("waterway") {
                    waterways::generate_waterways(&mut editor, way, &ground);
                } else if way.tags.contains_key("bridge") {
                    //bridges::generate_bridges(&mut editor, way, ground_level); // TODO FIX
                } else if way.tags.contains_key("railway") {
                    railways::generate_railways(&mut editor, way, &ground);
                } else if way.tags.contains_key("aeroway") || way.tags.contains_key("area:aeroway")
                {
                    highways::generate_aeroway(&mut editor, way, &ground);
                } else if way.tags.get("service") == Some(&"siding".to_string()) {
                    highways::generate_siding(&mut editor, way, &ground);
                }
            }
            ProcessedElement::Node(node) => {
                if node.tags.contains_key("door") || node.tags.contains_key("entrance") {
                    doors::generate_doors(&mut editor, node, &ground);
                } else if node.tags.contains_key("natural")
                    && node.tags.get("natural") == Some(&"tree".to_string())
                {
                    natural::generate_natural(&mut editor, element, &ground, args);
                } else if node.tags.contains_key("amenity") {
                    amenities::generate_amenities(&mut editor, element, &ground, args);
                } else if node.tags.contains_key("barrier") {
                    barriers::generate_barriers(&mut editor, element, &ground);
                } else if node.tags.contains_key("highway") {
                    highways::generate_highways(&mut editor, element, &ground, args);
                } else if node.tags.contains_key("tourism") {
                    tourisms::generate_tourisms(&mut editor, node, &ground);
                }
            }
            ProcessedElement::Relation(rel) => {
                if rel.tags.contains_key("building") || rel.tags.contains_key("building:part") {
                    buildings::generate_building_from_relation(&mut editor, rel, &ground, args);
                } else if rel.tags.contains_key("water") {
                    water_areas::generate_water_areas(&mut editor, rel, &ground);
                } else if rel.tags.get("leisure") == Some(&"park".to_string()) {
                    leisure::generate_leisure_from_relation(&mut editor, rel, &ground, args);
                }
            }
        }
    }

    process_pb.finish();

    // Generate ground layer
    let total_blocks: u64 = (scale_factor_x as i32 + 1) as u64 * (scale_factor_z as i32 + 1) as u64;
    let desired_updates: u64 = 1500;
    let batch_size: u64 = (total_blocks / desired_updates).max(1);

    let mut block_counter: u64 = 0;

    println!("{} Generating ground...", "[4/5]".bold());
    emit_gui_progress_update(60.0, "Generating ground...");

    let ground_pb: ProgressBar = ProgressBar::new(total_blocks);
    ground_pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:45}] {pos}/{len} blocks ({eta})")
            .unwrap()
            .progress_chars("█▓░"),
    );

    let mut gui_progress_grnd: f64 = 60.0;
    let mut last_emitted_progress: f64 = gui_progress_grnd;
    let total_iterations_grnd: f64 = (scale_factor_x + 1.0) * (scale_factor_z + 1.0);
    let progress_increment_grnd: f64 = 30.0 / total_iterations_grnd;

    let groundlayer_block = if args.winter { SNOW_BLOCK } else { GRASS_BLOCK };

    // Differentiate between terrain and non-terrain generation
    if ground.elevation_enabled {
        // Pre-calculate ground levels for all points
        let mut ground_levels: Vec<Vec<i32>> = Vec::with_capacity(scale_factor_x as usize + 1);
        for x in 0..=(scale_factor_x as i32) {
            let mut row = Vec::with_capacity(scale_factor_z as usize + 1);
            for z in 0..=(scale_factor_z as i32) {
                row.push(ground.level(XZPoint::new(x, z)));
            }
            ground_levels.push(row);
        }

        // Process blocks in larger batches
        for x in 0..=(scale_factor_x as i32) {
            for z in 0..=(scale_factor_z as i32) {
                let ground_level = ground_levels[x as usize][z as usize];

                // Find the highest block in this column
                let max_y = (MIN_Y..ground_level)
                    .find(|y: &i32| editor.block_at(x, *y, z))
                    .unwrap_or(ground_level)
                    .min(ground_level);

                // Set blocks in a single batch
                editor.set_block(groundlayer_block, x, max_y, z, None, None);
                editor.set_block(DIRT, x, max_y - 1, z, None, None);
                editor.set_block(DIRT, x, max_y - 2, z, None, None);

                // Fill underground with stone
                if args.fillground {
                    editor.fill_blocks(STONE, x, MIN_Y + 1, z, x, max_y - 2, z, None, None);
                    editor.set_block(BEDROCK, x, MIN_Y, z, None, Some(&[BEDROCK]));
                }

                block_counter += 1;
                if block_counter % batch_size == 0 {
                    ground_pb.inc(batch_size);
                }

                gui_progress_grnd += progress_increment_grnd;
                if (gui_progress_grnd - last_emitted_progress).abs() > 0.25 {
                    emit_gui_progress_update(gui_progress_grnd, "");
                    last_emitted_progress = gui_progress_grnd;
                }
            }
        }

        // Set blocks at spawn location
        for x in 0..=20 {
            for z in 0..=20 {
                editor.set_block(groundlayer_block, x, -62, z, None, None);
            }
        }
    } else {
        for x in 0..=(scale_factor_x as i32) {
            for z in 0..=(scale_factor_z as i32) {
                let ground_level = ground.level(XZPoint::new(x, z));
                editor.set_block(groundlayer_block, x, ground_level, z, None, None);
                editor.set_block(DIRT, x, ground_level - 1, z, None, None);

                block_counter += 1;
                if block_counter % batch_size == 0 {
                    ground_pb.inc(batch_size);
                }

                gui_progress_grnd += progress_increment_grnd;
                if (gui_progress_grnd - last_emitted_progress).abs() > 0.25 {
                    emit_gui_progress_update(gui_progress_grnd, "");
                    last_emitted_progress = gui_progress_grnd;
                }
            }
        }
    }

    // Set sign for player orientation
    /*editor.set_sign(
        "↑".to_string(),
        "Generated World".to_string(),
        "This direction".to_string(),
        "".to_string(),
        9,
        -61,
        9,
        6,
    );*/

    ground_pb.inc(block_counter % batch_size);
    ground_pb.finish();

    // Save world
    editor.save();

    emit_gui_progress_update(100.0, "Done! World generation completed.");
    println!("{}", "Done! World generation completed.".green().bold());
    Ok(())
}

fn set_spawn_point(args: &Args) -> () {
    
    let world_path = &args.path;
    let mut path_buf =  PathBuf::from(world_path);
    path_buf.push("level.dat");
    let x_coord: i32;
    let z_coord: i32;
    
    // If spawn point argument provided, change spawn point from the origin to the given coordinates
    if args.spawn_point == None {
        x_coord = 0;
        z_coord = 0;
    } else {
        //convert lat/long spawn coordinates to minecraft coordinates
        let spawn_point: Vec<f64> = args
            .spawn_point
            .as_ref()
            .unwrap()
            .split(",")
            .map(|s:&str| s.parse::<f64>().expect("Invalid spawn point coordinate"))
            .collect();

        let bbox: Vec<f64> = args
            .bbox
            .as_ref()
            .expect("Bounding box is required")
            .split(',')
            .map(|s: &str| s.parse::<f64>().expect("Invalid bbox coordinate"))
            .collect::<Vec<f64>>();
            
        let bbox_tuple: (f64, f64, f64, f64) = (bbox[0], bbox[1], bbox[2], bbox[3]);

        let (scale_factor_z, scale_factor_x) = osm_parser::geo_distance(bbox_tuple.1, bbox_tuple.3, bbox_tuple.0, bbox_tuple.2);
        let scale_factor_z: f64 = scale_factor_z.floor() * args.scale;
        let scale_factor_x: f64 = scale_factor_x.floor() * args.scale;
        (x_coord, z_coord) = osm_parser::lat_lon_to_minecraft_coords(spawn_point[1], spawn_point[0], bbox_tuple, scale_factor_z, scale_factor_x);
    }

    // Grab and decompress level.dat
    let level_file = fs::File::open(&path_buf).expect("Failed to open level.bat");
    let mut decoder = GzDecoder::new(level_file);
    let mut decompressed_data = vec![];
    decoder
        .read_to_end(&mut decompressed_data)
        .expect("Failed to decompress level.dat");

    // Parse the decompressed NBT data
    let mut level_data: Value = fastnbt::from_bytes(&decompressed_data)
        .expect("Failed to parse level.dat");

    if let Value::Compound(ref mut root) = level_data {
        if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
            //Set spawn point
            if let Value::Long(ref mut spawn_x) = data.get_mut("SpawnX").unwrap() {
                *spawn_x = x_coord as i64;
            }
            if let Value::Long(ref mut spawn_z) = data.get_mut("SpawnZ").unwrap() {
                *spawn_z = z_coord as i64;
            }
            if let Value::Long(ref mut spawn_y) = data.get_mut("SpawnY").unwrap() {
                *spawn_y = -61;
            }
            // Update player position and rotation
            if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
                if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
                    if let Value::Double(ref mut x) = pos.get_mut(0).unwrap() {
                        *x = x_coord as f64;
                    }
                    if let Value::Double(ref mut y) = pos.get_mut(1).unwrap() {
                        *y = -61.0;
                    }
                    if let Value::Double(ref mut z) = pos.get_mut(2).unwrap() {
                        *z = z_coord as f64;
                    }
                }

                if let Some(Value::List(ref mut rot)) = player.get_mut("Rotation") {
                    if let Value::Float(ref mut x) = rot.get_mut(0).unwrap() {
                        *x = -45.0;
                    }
                }
            }
        }
    }
    // Serialize the updated NBT data back to bytes
    let serialized_level_data: Vec<u8> = fastnbt::to_bytes(&level_data)
        .expect("Failed to serialize updated level.dat");

    // Compress the serialized data back to gzip
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized_level_data)
        .expect("Failed to compress updated level.dat");
    let compressed_level_data = encoder
        .finish()
        .expect("Failed to finalize compression for level.dat");

    // Write the level.dat file
    fs::write(&path_buf, compressed_level_data)
        .expect("Failed to create level.dat file");
    println!("({},{})",x_coord,z_coord);
}