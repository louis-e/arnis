use crate::block_definitions::*;
use crate::world_editor::WorldEditor;
use std::collections::HashSet;

/// Interior layout for building ground floors (1st layer above floor)
#[rustfmt::skip]
const INTERIOR1_LAYER1: [[char; 23]; 23] = [
    ['1', 'U', ' ', 'W', 'C', ' ', ' ', ' ', 'S', 'S', 'W', 'B', 'T', 'T', 'B', 'W', '7', '8', ' ', ' ', ' ', ' ', 'W',],
    ['2', ' ', ' ', 'W', 'F', ' ', ' ', ' ', 'U', 'U', 'W', 'B', 'T', 'T', 'B', 'W', '7', '8', ' ', ' ', ' ', 'B', 'W',],
    [' ', ' ', ' ', 'W', 'F', ' ', ' ', ' ', ' ', ' ', 'W', 'B', 'T', 'T', 'B', 'W', 'W', 'W', 'D', 'W', 'W', 'W', 'W',],
    ['W', 'W', 'D', 'W', 'L', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'A', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'D', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'W', 'W', 'W', 'D', 'W', 'W', 'W', 'W', 'D', 'W', 'W', ' ', ' ', 'D',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'B', 'B', 'B', ' ', ' ', 'J', 'W', ' ', ' ', ' ', 'B', 'W', 'W', 'W',],
    ['W', 'W', 'W', 'W', 'D', 'W', ' ', ' ', 'W', 'T', 'S', 'S', 'T', ' ', ' ', 'W', 'S', 'S', ' ', 'B', 'W', 'W', 'W',],
    [' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'W', 'T', 'T', 'T', 'T', ' ', ' ', 'W', 'U', 'U', ' ', 'B', 'W', ' ', ' ',],
    [' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'D', 'T', 'T', 'T', 'T', ' ', 'B', 'W', ' ', ' ', ' ', 'B', 'W', ' ', ' ',],
    ['L', ' ', 'A', 'L', 'W', 'W', ' ', ' ', 'W', 'J', 'U', 'U', ' ', ' ', 'B', 'W', 'W', 'D', 'W', 'W', 'W', ' ', ' ',],
    ['W', 'W', 'W', 'W', 'W', 'W', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'D', 'W', 'W', ' ', ' ', 'W', 'C', 'C', 'W', 'W',],
    ['B', 'B', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'D', ' ', ' ', 'W', ' ', ' ', 'W', 'W',],
    [' ', ' ', ' ', 'D', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'D', ' ', ' ', ' ', ' ', ' ', ' ', 'D',],
    [' ', '6', ' ', 'W', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'D', 'W', 'W', 'D', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    ['U', '5', ' ', 'W', ' ', ' ', 'W', 'C', 'F', 'F', ' ', ' ', 'W', ' ', ' ', 'W', 'W', 'D', 'W', 'W', ' ', ' ', 'W',],
    ['W', 'W', 'W', 'W', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', 'W', 'L', ' ', 'W', 'A', ' ', 'B', 'W', ' ', ' ', 'W',],
    ['B', ' ', ' ', ' ', ' ', ' ', 'D', ' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'W', ' ', ' ', 'B', 'W', 'J', ' ', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W', ' ', 'W', 'U', ' ', ' ', 'W', 'B', ' ', 'D',],
    ['J', ' ', ' ', 'C', 'B', 'B', 'W', 'L', 'F', ' ', 'W', 'F', ' ', 'W', 'L', 'W', '7', '8', ' ', 'W', 'B', ' ', 'W',],
    ['B', ' ', ' ', 'B', 'W', 'W', 'W', 'W', 'W', ' ', 'W', 'A', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'C', ' ', 'W',],
    ['B', ' ', ' ', 'B', 'W', ' ', ' ', ' ', 'D', ' ', 'W', 'C', ' ', ' ', 'W', 'W', 'B', 'B', 'B', 'B', 'W', 'D', 'W',],
    ['W', 'W', 'D', 'W', 'C', ' ', ' ', ' ', 'W', 'W', 'W', 'B', 'T', 'T', 'B', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
];

/// Interior layout for building ground floors (2nd layer above floor)
#[rustfmt::skip]
const INTERIOR1_LAYER2: [[char; 23]; 23] = [
    [' ', 'P', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'B', ' ', ' ', 'B', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    [' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', 'P', 'P', 'W', 'B', ' ', ' ', 'B', 'W', ' ', ' ', ' ', ' ', ' ', 'B', 'W',],
    [' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'B', ' ', ' ', 'B', 'W', 'W', 'W', 'D', 'W', 'W', 'W', 'W',],
    ['W', 'W', 'D', 'W', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'D', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'W', 'W', 'W', 'D', 'W', 'W', 'W', 'W', 'D', 'W', 'W', ' ', ' ', 'D',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'B', 'B', 'B', ' ', ' ', ' ', 'W', ' ', ' ', ' ', 'B', 'W', 'W', 'W',],
    ['W', 'W', 'W', 'W', 'D', 'W', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', ' ', 'B', 'W', 'W', 'W',],
    [' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'P', 'P', ' ', 'B', 'W', ' ', ' ',],
    [' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'D', ' ', ' ', ' ', ' ', ' ', 'B', 'W', ' ', ' ', ' ', 'B', 'W', ' ', ' ',],
    [' ', ' ', ' ', ' ', 'W', 'W', ' ', ' ', 'W', ' ', 'P', 'P', ' ', ' ', 'B', 'W', 'W', 'D', 'W', 'W', 'W', ' ', ' ',],
    ['W', 'W', 'W', 'W', 'W', 'W', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'D', 'W', 'W', ' ', ' ', 'W', 'C', 'C', 'W', 'W',],
    ['B', 'B', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'D', ' ', ' ', 'W', ' ', ' ', 'W', 'W',],
    [' ', ' ', ' ', 'D', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'D', ' ', ' ', ' ', ' ', ' ', ' ', 'D',],
    [' ', ' ', ' ', 'W', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'D', 'W', 'W', 'D', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    ['P', ' ', ' ', 'W', ' ', ' ', 'W', 'N', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'W', 'W', 'D', 'W', 'W', ' ', ' ', 'W',],
    ['W', 'W', 'W', 'W', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'W', ' ', ' ', 'B', 'W', ' ', ' ', 'W',],
    ['B', ' ', ' ', ' ', ' ', ' ', 'D', ' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'W', ' ', ' ', 'C', 'W', ' ', ' ', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W', ' ', 'W', 'P', ' ', ' ', 'W', 'B', ' ', 'D',],
    [' ', ' ', ' ', ' ', 'B', 'B', 'W', ' ', ' ', ' ', 'W', ' ', ' ', 'W', 'P', 'W', ' ', ' ', ' ', 'W', 'B', ' ', 'W',],
    ['B', ' ', ' ', 'B', 'W', 'W', 'W', 'W', 'W', ' ', 'W', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'W', ' ', ' ', 'W',],
    ['B', ' ', ' ', 'B', 'W', ' ', ' ', ' ', 'D', ' ', 'W', 'N', ' ', ' ', 'W', 'W', 'B', 'B', 'B', 'B', 'W', 'D', 'W',],
    ['W', 'W', 'D', 'W', ' ', ' ', ' ', ' ', 'W', 'W', 'W', 'B', ' ', ' ', 'B', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
];

/// Interior layout for building level floors (1nd layer above floor)
#[rustfmt::skip]
const INTERIOR2_LAYER1: [[char; 23]; 23] = [
    ['W', 'W', 'W', 'D', 'W', 'W', 'W', 'W', 'W', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'D', 'W', 'W', 'W',],
    ['U', ' ', ' ', ' ', ' ', ' ', 'C', 'W', 'L', ' ', ' ', 'L', 'W', 'A', 'A', 'W', ' ', ' ', ' ', ' ', ' ', 'L', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    [' ', ' ', 'W', 'W', 'W', ' ', ' ', 'W', ' ', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', 'S', 'S', 'S', ' ', 'W',],
    [' ', ' ', 'W', 'F', ' ', ' ', ' ', 'W', 'C', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'J', ' ', 'U', 'U', 'U', ' ', 'D',],
    ['U', ' ', 'W', 'F', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W',],
    ['U', ' ', 'W', 'F', ' ', ' ', ' ', 'D', ' ', ' ', 'T', 'T', 'W', ' ', ' ', ' ', ' ', ' ', 'U', 'W', ' ', 'L', 'W',],
    [' ', ' ', 'W', 'W', 'W', ' ', ' ', 'W', ' ', ' ', 'T', 'J', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'D', 'W', 'W', 'W', ' ', ' ', 'W', 'L', ' ', 'W',],
    ['J', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'C', ' ', ' ', ' ', 'B', 'W', ' ', ' ', 'W', ' ', ' ', 'W',],
    ['W', 'W', 'W', 'W', 'W', 'L', ' ', ' ', ' ', ' ', 'W', 'C', ' ', ' ', ' ', 'B', 'W', ' ', ' ', 'W', 'W', 'D', 'W',],
    [' ', 'A', 'B', 'B', 'W', 'W', 'W', 'W', ' ', ' ', 'W', ' ', ' ', ' ', ' ', 'B', 'W', ' ', ' ', ' ', ' ', ' ', 'W',],
    [' ', ' ', ' ', 'B', 'W', 'L', ' ', ' ', ' ', ' ', 'W', 'L', ' ', ' ', 'B', 'W', 'W', 'B', 'B', 'W', ' ', ' ', 'W',],
    [' ', ' ', ' ', 'B', 'W', ' ', ' ', ' ', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', ' ', ' ', 'D',],
    [' ', ' ', ' ', ' ', 'D', ' ', ' ', 'U', ' ', ' ', ' ', 'D', ' ', ' ', 'F', 'F', 'W', 'A', 'A', 'W', ' ', ' ', 'W',],
    [' ', ' ', ' ', ' ', 'W', ' ', ' ', 'U', ' ', ' ', 'W', 'W', ' ', ' ', ' ', ' ', 'C', ' ', ' ', 'W', ' ', ' ', 'W',],
    ['C', ' ', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', ' ', ' ', ' ', ' ', 'L', ' ', ' ', 'W', 'W', 'D', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'D', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    ['L', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'L', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    ['W', 'W', 'W', 'W', 'W', 'W', ' ', ' ', 'U', 'U', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', ' ', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'U', 'U', ' ', 'W', 'B', ' ', 'U', 'U', 'B', ' ', ' ', ' ', ' ', ' ', 'W',],
    ['S', 'S', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'B', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'B', ' ', 'W',],
    ['U', 'U', ' ', ' ', ' ', 'L', 'B', 'B', 'B', ' ', ' ', 'W', 'B', 'B', 'B', 'B', 'B', 'B', 'B', ' ', 'B', 'D', 'W',],
];

/// Interior layout for building level floors (2nd layer above floor)
#[rustfmt::skip]
const INTERIOR2_LAYER2: [[char; 23]; 23] = [
    ['W', 'W', 'W', 'D', 'W', 'W', 'W', 'W', 'W', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'D', 'W', 'W', 'W',],
    ['P', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'E', ' ', ' ', 'E', 'W', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', 'E', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    [' ', ' ', 'W', 'W', 'W', ' ', ' ', 'W', ' ', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    [' ', ' ', 'W', 'F', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'P', 'P', 'P', ' ', 'D',],
    ['P', ' ', 'W', 'F', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W',],
    ['P', ' ', 'W', 'F', ' ', ' ', ' ', 'D', ' ', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', 'P', 'W', ' ', 'P', 'W',],
    [' ', ' ', 'W', 'W', 'W', ' ', ' ', 'W', ' ', ' ', ' ', ' ', 'W', ' ', ' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'D', 'W', 'W', 'W', ' ', ' ', 'W', ' ', ' ', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'P', ' ', ' ', ' ', 'B', 'W', ' ', ' ', 'W', ' ', ' ', 'W',],
    ['W', 'W', 'W', 'W', 'W', 'E', ' ', ' ', ' ', ' ', 'W', 'P', ' ', ' ', ' ', 'B', 'W', ' ', ' ', 'W', 'W', 'D', 'W',],
    [' ', ' ', 'B', 'B', 'W', 'W', 'W', 'W', ' ', ' ', 'W', ' ', ' ', ' ', ' ', 'B', 'W', ' ', ' ', ' ', ' ', ' ', 'W',],
    [' ', ' ', ' ', 'B', 'W', 'E', ' ', ' ', ' ', ' ', 'W', 'E', ' ', ' ', 'B', 'W', 'W', 'B', 'B', 'W', ' ', ' ', 'W',],
    [' ', ' ', ' ', 'B', 'W', ' ', ' ', ' ', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', ' ', ' ', 'D',],
    [' ', ' ', ' ', ' ', 'D', ' ', ' ', 'P', ' ', ' ', ' ', 'D', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'W', ' ', ' ', 'W',],
    [' ', ' ', ' ', ' ', 'W', ' ', ' ', 'P', ' ', ' ', 'W', 'W', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', ' ', ' ', 'W',],
    [' ', ' ', ' ', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', ' ', ' ', ' ', ' ', 'E', ' ', ' ', 'W', 'W', 'D', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'D', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    ['E', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'E', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W',],
    ['W', 'W', 'W', 'W', 'W', 'W', ' ', ' ', 'P', 'P', ' ', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', 'W', ' ', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'P', 'P', ' ', 'W', 'B', ' ', 'P', 'P', 'B', ' ', ' ', ' ', ' ', ' ', 'W',],
    [' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'W', 'B', ' ', ' ', ' ', ' ', ' ', ' ', ' ', 'B', ' ', 'W',],
    ['P', 'P', ' ', ' ', ' ', 'E', 'B', 'B', 'B', ' ', ' ', 'W', 'B', 'B', 'B', 'B', 'B', 'B', 'B', ' ', 'B', ' ', 'D',],
];

/// Maps interior layout characters to actual block types for different floor layers
#[inline(always)]
pub fn get_interior_block(c: char, is_layer2: bool, wall_block: Block) -> Option<Block> {
    match c {
        ' ' => None,                     // Nothing
        'W' => Some(wall_block),         // Use the building's wall block for interior walls
        'U' => Some(OAK_FENCE),          // Oak Fence
        'S' => Some(OAK_STAIRS),         // Oak Stairs
        'B' => Some(BOOKSHELF),          // Bookshelf
        'C' => Some(CRAFTING_TABLE),     // Crafting Table
        'F' => Some(FURNACE),            // Furnace
        '1' => Some(RED_BED_NORTH_HEAD), // Bed North Head
        '2' => Some(RED_BED_NORTH_FOOT), // Bed North Foot
        '3' => Some(RED_BED_EAST_HEAD),  // Bed East Head
        '4' => Some(RED_BED_EAST_FOOT),  // Bed East Foot
        '5' => Some(RED_BED_SOUTH_HEAD), // Bed South Head
        '6' => Some(RED_BED_SOUTH_FOOT), // Bed South Foot
        '7' => Some(RED_BED_WEST_HEAD),  // Bed West Head
        '8' => Some(RED_BED_WEST_FOOT),  // Bed West Foot
        // 'H' => Some(CHEST),           // Chest
        'L' => Some(CAULDRON),           // Cauldron
        'A' => Some(ANVIL),              // Anvil
        'P' => Some(OAK_PRESSURE_PLATE), // Pressure Plate
        'D' => {
            // Use different door types for different layers
            if is_layer2 {
                Some(DARK_OAK_DOOR_UPPER)
            } else {
                Some(DARK_OAK_DOOR_LOWER)
            }
        }
        'J' => Some(NOTE_BLOCK),    // Note block
        'G' => Some(GLOWSTONE),     // Glowstone
        'N' => Some(BREWING_STAND), // Brewing Stand
        'T' => Some(WHITE_CARPET),  // White Carpet
        'E' => Some(OAK_LEAVES),    // Oak Leaves
        _ => None,                  // Default case for unknown characters
    }
}

/// Generates interior layouts inside buildings at each floor level
#[allow(clippy::too_many_arguments)]
pub fn generate_building_interior(
    editor: &mut WorldEditor,
    floor_area: &[(i32, i32)],
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    start_y_offset: i32,
    building_height: i32,
    wall_block: Block,
    floor_levels: &[i32],
    args: &crate::args::Args,
    element: &crate::osm_parser::ProcessedWay,
    abs_terrain_offset: i32,
) {
    // Skip interior generation for very small buildings
    let width = max_x - min_x + 1;
    let depth = max_z - min_z + 1;

    if width < 8 || depth < 8 {
        return; // Building too small for interior
    }

    // For efficiency, create a HashSet of floor area coordinates
    let floor_area_set: HashSet<(i32, i32)> = floor_area.iter().cloned().collect();

    // Add buffer around edges to avoid placing furniture too close to walls
    let buffer = 2;
    let interior_min_x = min_x + buffer;
    let interior_min_z = min_z + buffer;
    let interior_max_x = max_x - buffer;
    let interior_max_z = max_z - buffer;

    // Generate interiors for each floor
    for (floor_index, &floor_y) in floor_levels.iter().enumerate() {
        // Store wall and door positions for this floor to extend them to the ceiling
        let mut wall_positions = Vec::new();
        let mut door_positions = Vec::new();

        // Determine the floor extension height (ceiling) - either next floor or roof
        let current_floor_ceiling = if floor_index < floor_levels.len() - 1 {
            // For intermediate floors, extend walls up to just below the next floor
            floor_levels[floor_index + 1] - 1
        } else {
            // Last floor ceiling depends on roof generation
            if args.roof
                && element.tags.contains_key("roof:shape")
                && element.tags.get("roof:shape").unwrap() != "flat"
            {
                // When roof generation is enabled with non-flat roofs, stop at building height (no extra ceiling)
                start_y_offset + building_height
            } else {
                // When roof generation is disabled or flat roof, extend to building top + 1 (includes ceiling)
                start_y_offset + building_height + 1
            }
        };

        // Choose the appropriate interior pattern based on floor number
        let (layer1, layer2) = if floor_index == 0 {
            // Ground floor uses INTERIOR1 patterns
            (&INTERIOR1_LAYER1, &INTERIOR1_LAYER2)
        } else {
            // Upper floors use INTERIOR2 patterns
            (&INTERIOR2_LAYER1, &INTERIOR2_LAYER2)
        };

        // Get dimensions for the selected pattern
        let pattern_height = layer1.len() as i32;
        let pattern_width = layer1[0].len() as i32;

        // Calculate Y offset - place interior 1 block above floor level consistently
        let y_offset = 1;

        // Create a seamless repeating pattern across the interior of this floor
        for z in interior_min_z..=interior_max_z {
            for x in interior_min_x..=interior_max_x {
                // Skip if outside the building's floor area
                if !floor_area_set.contains(&(x, z)) {
                    continue;
                }

                // Map the world coordinates to pattern coordinates using modulo
                // This creates a seamless tiling effect across the entire building
                // Add floor_index offset to create variation between floors
                let pattern_x = ((x - interior_min_x + floor_index as i32) % pattern_width
                    + pattern_width)
                    % pattern_width;
                let pattern_z = ((z - interior_min_z + floor_index as i32) % pattern_height
                    + pattern_height)
                    % pattern_height;

                // Access the pattern arrays safely
                let cell1 = layer1[pattern_z as usize][pattern_x as usize];
                let cell2 = layer2[pattern_z as usize][pattern_x as usize];

                // Place first layer blocks
                if let Some(block) = get_interior_block(cell1, false, wall_block) {
                    editor.set_block_absolute(
                        block,
                        x,
                        floor_y + y_offset + abs_terrain_offset,
                        z,
                        None,
                        None,
                    );

                    // If this is a wall in layer 1, add to wall positions to extend later
                    if cell1 == 'W' {
                        wall_positions.push((x, z));
                    }
                    // If this is a door in layer 1, add to door positions to add wall above later
                    else if cell1 == 'D' {
                        door_positions.push((x, z));
                    }
                }

                // Place second layer blocks
                if let Some(block) = get_interior_block(cell2, true, wall_block) {
                    editor.set_block_absolute(
                        block,
                        x,
                        floor_y + y_offset + abs_terrain_offset + 1,
                        z,
                        None,
                        None,
                    );
                }
            }
        }

        // Extend walls all the way to the next floor ceiling or roof
        for (x, z) in &wall_positions {
            for y in (floor_y + y_offset + 2)..=current_floor_ceiling {
                editor.set_block_absolute(wall_block, *x, y + abs_terrain_offset, *z, None, None);
            }
        }

        // Add wall blocks above doors all the way to the ceiling/next floor
        for (x, z) in &door_positions {
            for y in (floor_y + y_offset + 2)..=current_floor_ceiling {
                editor.set_block_absolute(wall_block, *x, y + abs_terrain_offset, *z, None, None);
            }
        }
    }
}
