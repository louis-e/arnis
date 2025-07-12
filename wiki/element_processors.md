# Element Processors in Arnis

This page documents the various element processors used in Arnis to convert OpenStreetMap (OSM) data into Minecraft blocks. Each processor is responsible for handling specific types of map elements and applying appropriate rendering logic.

## Overview

Element processors form the core of Arnis's terrain generation capability. They translate abstract geographic data into tangible Minecraft structures by:

1. Interpreting OSM tags and attributes
2. Selecting appropriate block types and structures
3. Placing blocks using the WorldEditor interface
4. Applying specialized rendering algorithms for different element types

## Element Processor Modules

![image](https://github.com/user-attachments/assets/3713fb99-616e-4a36-82d0-35bf9c774f07)

### 1. Buildings (`buildings.rs`)

The buildings processor handles all structures tagged as buildings in OSM data. It's one of the most complex processors, handling:

- Generation of building shells with walls and roofs
- Interior layouts using predefined templates
- Different architectural styles based on building tags
- Multi-story buildings with appropriate floor divisions
- Special rendering for landmark buildings

Buildings can vary from simple residential structures to complex commercial buildings with internal layouts. The processor considers:

- Building height (when available)
- Building type/use (residential, commercial, etc.)
- Architectural details from building:material tags

### 2. Highways (`highways.rs`)

The highways processor handles roads, paths, and related infrastructure:

- Different road widths based on highway type (motorway, primary, residential, etc.)
- Appropriate materials (concrete for major roads, gravel for minor roads)
- Sidewalks for urban roads
- Street furniture (lamps, signs)
- Traffic signals and crossings
- Bus stops and transit infrastructure

The processor uses the highway type to determine width and materials, creating a road network that reflects the real-world hierarchy.

### 3. Water Areas (`water_areas.rs`)

This processor handles bodies of water like:

- Lakes
- Ponds
- Reservoirs
- Swimming pools

Water areas are typically rendered as filled polygons of water blocks, with appropriate shorelines and depth gradients when possible.

### 4. Waterways (`waterways.rs`)

Unlike water areas, waterways process flowing water features:

- Rivers
- Streams
- Canals
- Ditches

Waterways are rendered as linear features with appropriate width according to their type, and can include bridges where they intersect with roads.

### 5. Landuse (`landuse.rs`) 

The landuse processor handles different types of land use areas:

- Residential zones (represented with appropriate urban textures)
- Commercial and industrial areas
- Parks and recreational spaces
- Agricultural land (farmland)
- Forests and greenspace
- Construction sites
- Military areas
- Railways

Each landuse type receives appropriate block types, with some types (like forests) getting additional decorations like trees.

### 6. Natural (`natural.rs`)

This processor handles natural features:

- Tree placements
- Beach and coastline rendering
- Rock and stone formations
- Cliffs and natural elevations
- Scrubland and heath

The processor uses specialized algorithms for organic-looking terrain features.

### 7. Amenities (`amenities.rs`)

The amenities processor handles points of interest and facilities:

- Benches
- Waste bins
- Fountains
- Public services
- ATMs and banking facilities
- Fire hydrants

These are typically rendered as small structures or marker blocks at the specific node locations.

### 8. Barriers (`barriers.rs`)

This processor handles various barrier types:

- Fences and walls
- Gates
- Bollards
- City walls and historic barriers
- Highway barriers and guardrails

Barriers are rendered as linear structures following their defined paths.

### 9. Bridges (`bridges.rs`)

The bridges processor creates structures where highways or railways cross waterways:

- Different bridge types based on bridge:type tag
- Appropriate supporting structures
- Clearance for water passage underneath

Bridges coordinate with highway and waterway processors to create coherent crossings.

### 10. Railways (`railways.rs`)

This processor handles railway infrastructure:

- Train tracks with appropriate spacing
- Railway stations and platforms
- Rail signals and switches
- Metro and tram lines

Railway tracks are rendered as continuous lines with specific block patterns to represent rails.

### 11. Leisure (`leisure.rs`)

The leisure processor handles recreational areas:

- Parks
- Playgrounds
- Sports fields and courts
- Swimming pools
- Golf courses

Each leisure type gets specialized rendering appropriate to its purpose.

### 12. Tourisms (`tourisms.rs`)

This processor handles tourist attractions and accommodations:

- Hotels and guest houses
- Camping sites
- Museums and art galleries
- Viewpoints
- Information centers

Tourism features may get special marker blocks or simplified representative structures.

### 13. Trees (`tree.rs`)

While part of natural features, trees have their dedicated processor due to their complexity:

- Different tree types based on species tags
- Random variations in tree shape and size
- Forest density calculations
- Seasonal variations (when specified)

Trees are generated using predefined templates or algorithmic patterns based on the tree type.

### 14. Doors (`doors.rs`)

This specialized processor handles:

- Building entrances
- Gates
- Access points

It works in coordination with buildings and barriers to place doors at appropriate locations.

## Implementation Details

Each element processor follows a similar pattern:

1. Receive an OSM element and WorldEditor reference
2. Extract relevant tags to determine rendering style
3. Calculate geometry (points, lines, or polygons)
4. Place blocks using appropriate algorithms
5. Add decorative elements and details

Processors may use helpers like:
- `bresenham_line` for drawing linear features
- `flood_fill_area` for filling enclosed areas
- Random number generators for variation in natural elements

## Extending Element Processors

When contributing new element processing capabilities:

1. Identify the appropriate processor file based on element category
2. Implement OSM tag handling with appropriate block selection
3. Add rendering logic in the processor's generate function
4. Update any shared constants or utilities as needed

Remember that as per Arnis contribution guidelines, new OSM tags should have at least 1,000 worldwide usages to be considered for implementation.

## Processing Flow

As shown in the architectural diagram, element processors are called from the `DataProcessing` module during world generation. The general flow is:

1. Elements are parsed from OSM data
2. Elements are sorted by priority (landuse first, then buildings, etc.)
3. Map transformations are applied
4. Elements are dispatched to their respective processors
5. Each processor places blocks via the WorldEditor interface
6. The resulting world data is written to Minecraft region files

This modular approach allows for easy extension of Arnis's capabilities as new OSM tags and features are supported.
