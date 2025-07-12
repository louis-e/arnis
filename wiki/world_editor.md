# WorldEditor Technical Documentation

This document provides a complete technical reference for the `WorldEditor` system in Arnis - the core component responsible for Minecraft world manipulation and block placement.

## Overview

The `WorldEditor` serves as the interface between OSM elements and Minecraft's world format. It handles all aspects of block placement, world structure navigation, and region file management. The system abstracts away the complexity of Minecraft's region file format (Anvil) and provides a simplified API for element processors to place blocks.

## Architecture

The system follows a hierarchical structure that mirrors Minecraft's own world organization:

```
WorldToModify
├── RegionToModify (32×32 chunks)
│   └── ChunkToModify (16×16×384 blocks)
│       └── SectionToModify (16×16×16 blocks)
```

### Key Components

#### 1. SectionToModify

The smallest unit of data storage, representing a 16×16×16 block volume:

- **Storage**: Uses a fixed-size array of 4096 elements to store block data
- **Interface**:
  - `get_block(x, y, z) -> Option<Block>` - Retrieves a block at local coordinates
  - `set_block(x, y, z, block)` - Sets a block at local coordinates
  - `to_section(y) -> Section` - Converts to Minecraft's serializable format

#### 2. ChunkToModify

Represents a vertical column of blocks in the world (16×16×384):

- **Storage**: HashMap of section Y-indices to `SectionToModify` instances
- **Interface**:
  - `get_block(x, y, z)` - Gets a block using global Y and local X/Z coordinates
  - `set_block(x, y, z, block)` - Sets a block using global Y and local X/Z coordinates
  - `sections() -> Iterator<Section>` - Provides an iterator over serializable sections

#### 3. RegionToModify

Groups 32×32 chunks together:

- **Storage**: HashMap of `(chunk_x, chunk_z)` tuples to `ChunkToModify` instances
- **Interface**:
  - `get_or_create_chunk(x, z)` - Gets or creates a chunk at specified coordinates
  - `get_chunk(x, z)` - Retrieves a chunk if it exists

#### 4. WorldToModify

Top-level structure representing the entire world:

- **Storage**: HashMap of `(region_x, region_z)` tuples to `RegionToModify` instances
- **Interface**: 
  - `get_or_create_region(x, z)` - Gets or creates a region at specified coordinates
  - `get_region(x, z)` - Retrieves a region if it exists
  - `get_block(x, y, z)` - Gets a block at global coordinates
  - `set_block(x, y, z, block)` - Sets a block at global coordinates

## WorldEditor API

The `WorldEditor` struct provides the main interface for element processors:

### Construction and Initialization

```rust
pub fn new(region_dir: &str, xzbbox: &'a XZBBox) -> Self
```

- **Parameters**:
  - `region_dir` - Directory where region files will be stored
  - `xzbbox` - Reference to bounding box defining the area's extents

### Ground Integration

```rust
pub fn set_ground(&mut self, ground: &Ground)
pub fn get_ground(&self) -> Option<&Ground>
pub fn get_absolute_y(&self, x: i32, y_offset: i32, z: i32) -> i32
```

The ground system allows blocks to be placed relative to terrain elevation:

- `set_ground` - Associates terrain elevation data with the editor
- `get_ground` - Retrieves ground data if available
- `get_absolute_y` - Converts a Y-offset into an absolute Y position based on ground elevation

### Block Manipulation

```rust
pub fn set_block(
    &mut self, 
    block: Block, 
    x: i32, 
    y: i32, 
    z: i32, 
    override_whitelist: Option<&[Block]>, 
    override_blacklist: Option<&[Block]>
)

pub fn set_block_absolute(
    &mut self, 
    block: Block, 
    x: i32, 
    absolute_y: i32, 
    z: i32,
    override_whitelist: Option<&[Block]>, 
    override_blacklist: Option<&[Block]>
)

pub fn fill_blocks(
    &mut self, 
    block: Block, 
    x1: i32, 
    y1: i32, 
    z1: i32, 
    x2: i32, 
    y2: i32, 
    z2: i32, 
    override_whitelist: Option<&[Block]>, 
    override_blacklist: Option<&[Block]>
)

pub fn fill_blocks_absolute(
    &mut self, 
    block: Block, 
    x1: i32, 
    y1_absolute: i32, 
    z1: i32, 
    x2: i32, 
    y2_absolute: i32, 
    z2: i32, 
    override_whitelist: Option<&[Block]>, 
    override_blacklist: Option<&[Block]>
)
```

Block manipulation methods follow two key patterns:

1. **Ground-relative vs. Absolute**: Methods either use terrain-relative Y coordinates (`set_block`, `fill_blocks`) or absolute Y coordinates (`set_block_absolute`, `fill_blocks_absolute`)

2. **Override Control**: Optional whitelist and blacklist parameters control when blocks can override existing blocks:
   - If `override_whitelist` is provided, only blocks in this list can be overridden
   - If `override_blacklist` is provided, any block not in this list can be overridden
   - If neither is provided, blocks won't override existing blocks

### Block Querying

```rust
pub fn block_at(&self, x: i32, y: i32, z: i32) -> bool
pub fn check_for_block(&self, x: i32, y: i32, z: i32, whitelist: Option<&[Block]>) -> bool
pub fn check_for_block_absolute(&self, x: i32, absolute_y: i32, z: i32, 
                               whitelist: Option<&[Block]>, blacklist: Option<&[Block]>) -> bool
pub fn block_at_absolute(&self, x: i32, absolute_y: i32, z: i32) -> bool
```

Methods for checking block existence follow the same patterns as block manipulation methods, but additionally:

- `block_at` - Simply checks if any block exists (ground-relative)
- `check_for_block` - Checks if a block exists and optionally if it's in a whitelist
- `check_for_block_absolute` - Checks if a block exists using absolute Y, with whitelist/blacklist support
- `block_at_absolute` - Simply checks if any block exists (absolute Y)

### Special Features

```rust
pub fn set_sign(
    &mut self, 
    line1: String, 
    line2: String, 
    line3: String, 
    line4: String, 
    x: i32, 
    y: i32, 
    z: i32, 
    _rotation: i8
)
```

- `set_sign` - Places a sign with specified text at the given location

### World Saving

```rust
pub fn save(&mut self)
```

The `save` method serializes all modified regions to Anvil format:

1. Iterates through all modified regions in parallel using `rayon`
2. For each modified chunk in a region:
   - Reads existing chunk data if present
   - Merges new sections with existing sections
   - Preserves existing block entities (sign data, etc.)
   - Updates chunk metadata
3. Ensures all chunks exist by adding base chunks where needed
4. Reports progress through the progress system

## Technical Implementation Details

### Block Entity Handling

Block entities (like signs, chests, etc.) are stored separately from block data and must be carefully managed:

```rust
fn get_entity_coords(entity: &HashMap<String, Value>) -> (i32, i32, i32)
```

This helper extracts coordinates from entity data to determine entity placement.

### Level Wrapper Generation

```rust
fn create_level_wrapper(chunk: &Chunk) -> HashMap<String, Value>
```

This function creates the NBT structure required by Minecraft for chunk data.

### Base Chunk Creation

```rust
fn create_base_chunk(abs_chunk_x: i32, abs_chunk_z: i32) -> (Vec<u8>, bool)
```

When a chunk is needed but hasn't been modified, this creates a minimal valid chunk with grass blocks at Y=-62.

### Minecraft NBT Format

The system implements Minecraft's Anvil format, which uses NBT (Named Binary Tag):

#### Chunk Structure
```rust
struct Chunk {
    sections: Vec<Section>,
    x_pos: i32,
    z_pos: i32,
    is_light_on: u8,
    other: FnvHashMap<String, Value>,
}
```

#### Section Structure
```rust
struct Section {
    block_states: Blockstates,
    y: i8,
    other: FnvHashMap<String, Value>,
}
```

#### Blockstates Structure
```rust
struct Blockstates {
    palette: Vec<PaletteItem>,
    data: Option<LongArray>,
    other: FnvHashMap<String, Value>,
}
```

#### Palette Item Structure
```rust
struct PaletteItem {
    name: String,
    properties: Option<Value>,
}
```

### Memory Optimization

The system uses several optimization techniques:

1. **FnvHashMap**: Faster than standard HashMap for integer keys
2. **Lazy Chunk Creation**: Chunks are only created when needed
3. **Efficient Bit Packing**: Block data uses the minimum required bits per block
4. **Parallel Processing**: Region saving happens in parallel using `rayon`

## Coordinate Handling

The coordinate system follows Minecraft's conventions:

- X-axis: East-West (increasing east)
- Y-axis: Up-Down (increasing up)
- Z-axis: North-South (increasing south)

Coordinates are transformed through multiple steps:

1. **Region Coordinates**: Division by 32 chunks (512 blocks)
   ```rust
   region_x = chunk_x >> 5; // Divide by 32
   region_z = chunk_z >> 5; // Divide by 32
   ```

2. **Chunk Coordinates**: Division by 16 blocks
   ```rust
   chunk_x = x >> 4; // Divide by 16
   chunk_z = z >> 4; // Divide by 16
   ```

3. **Local Coordinates**: Modulo 16 operation
   ```rust
   local_x = x & 15; // x % 16
   local_z = z & 15; // z % 16
   ```

4. **Section Y Index**: Division by 16 blocks
   ```rust
   section_idx = (y >> 4).try_into().unwrap(); // Divide by 16
   ```

5. **Local Y Coordinate**: Y modulo 16
   ```rust
   local_y = (y & 15).try_into().unwrap(); // y % 16
   ```

## Performance Considerations

1. **Batch Operations**: `fill_blocks` is more efficient than multiple `set_block` calls
2. **Memory Footprint**: The system stores only modified chunks and sections
3. **Parallel Processing**: Region saving uses parallel processing to speed up serialization
4. **Progress Reporting**: Regular progress reports are provided via the `emit_gui_progress_update` function
5. **Collision Avoidance**: Block placement uses a check-before-set pattern to avoid unnecessary overrides

## Limitations and Edge Cases

1. **Memory Usage**: Very large areas may consume significant memory
2. **Block Entity Support**: Currently limited to signs; complex block entities like chests require special handling
3. **Light Calculation**: Lighting data is not calculated automatically (set to 0)
4. **World Border**: Blocks outside the defined bounding box are silently ignored

## Integration with Element Processors

Element processors use the WorldEditor to place blocks by:

1. Receiving a mutable reference to WorldEditor: `&mut WorldEditor`
2. Using ground-relative placement with `set_block(block, x, y, z, ...)` or area-filling with `fill_blocks(...)`
3. Checking for existing blocks before placement with `check_for_block(...)`

## Error Handling

The system uses Rust's built-in error handling:

1. **I/O Errors**: Wrapped in `expect()` to terminate with an error message
2. **Conversion Errors**: Handled with `try_into().unwrap()` for predictable numeric conversions
3. **Bound Checking**: Implemented via the `xzbbox.contains()` method to silently ignore out-of-bounds blocks

## Example Usage

```rust
// Create a WorldEditor for a specific region directory and bounding box
let mut editor = WorldEditor::new("./world/region", &xzbbox);

// Set ground data for terrain-relative placement
editor.set_ground(&ground);

// Place blocks (relative to ground)
editor.set_block(STONE, 100, 1, 200, None, None); // Stone 1 block above ground at (100, 200)
editor.set_block(GRASS_BLOCK, 100, 2, 200, None, None); // Grass 2 blocks above ground

// Fill a cuboid area
editor.fill_blocks(
    STONE, 
    100, 1, 200, // Start coordinates
    110, 5, 210, // End coordinates
    None, None
);

// Place a sign
editor.set_sign(
    "Welcome".to_string(),
    "to".to_string(),
    "Arnis".to_string(),
    "World!".to_string(),
    100, 6, 200,
    0 // Rotation
);

// Save the world to disk
editor.save();
```

## Conclusion

The `WorldEditor` system provides a comprehensive interface for Minecraft world manipulation. It abstracts away the complexity of Minecraft's format while offering an efficient, memory-optimized solution for large-scale terrain generation. By handling coordinate transformations, block collision rules, and serialization details, it allows element processors to focus solely on their specific rendering logic.
