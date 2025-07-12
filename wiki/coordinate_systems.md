# Coordinate Systems in Arnis

This document provides a comprehensive technical overview of the coordinate system implementation in Arnis. The coordinate system is fundamental to the entire project as it handles the transformation between real-world geographic coordinates and Minecraft's block-based world.

## Overview

Arnis uses a modular approach to coordinate systems, with the current implementation focusing on a Cartesian XZ coordinate system that maps directly to Minecraft's coordinate space. The system is designed to be extensible and provides robust handling for:

- Points in 2D space (X and Z coordinates)
- Vectors between points
- Bounding boxes for defining areas

## Architecture

The coordinate system is organized in a hierarchical module structure:

```
coordinate_system/
├── mod.rs                 // Main module declaration
└── cartesian/             // Cartesian implementation
    ├── mod.rs             // Cartesian module exports
    ├── xzpoint.rs         // Point representation
    ├── xzvector.rs        // Vector representation
    └── xzbbox/            // Bounding box implementation
        ├── mod.rs         // Bounding box exports
        ├── rectangle.rs   // Rectangle implementation
        └── xzbbox_enum.rs // Bounding box enum wrapper
```

## Key Components

### XZPoint

`XZPoint` represents a discrete point in Minecraft's XZ plane:

```rust
pub struct XZPoint {
    pub x: i32,
    pub z: i32,
}
```

#### Features

- **Creation**: `XZPoint::new(x, z)` constructs a new point
- **Display**: Implements `fmt::Display` for human-readable output
- **Operations**: Supports addition/subtraction with vectors:
  - `point + vector -> point`
  - `point - vector -> point`
  - `point - point -> vector`

#### Usage Examples

```rust
// Create a new point
let p1 = XZPoint::new(10, 20);

// Vector arithmetic
let v = XZVector { dx: 5, dz: -3 };
let p2 = p1 + v;  // Results in XZPoint { x: 15, z: 17 }

// Difference between points
let v2 = p2 - p1;  // Results in XZVector { dx: 5, dz: -3 }
```

### XZVector

`XZVector` represents a displacement in the XZ plane:

```rust
pub struct XZVector {
    pub dx: i32,
    pub dz: i32,
}
```

#### Features

- **Display**: Implements `fmt::Display` for human-readable output
- **Operations**: Supports vector arithmetic:
  - `vector + vector -> vector`
  - `vector - vector -> vector`

#### Usage Examples

```rust
// Create vectors
let v1 = XZVector { dx: 5, dz: 10 };
let v2 = XZVector { dx: 3, dz: 4 };

// Vector addition
let sum = v1 + v2;  // Results in XZVector { dx: 8, dz: 14 }

// Vector subtraction
let diff = v1 - v2;  // Results in XZVector { dx: 2, dz: 6 }
```

### XZBBox

`XZBBox` defines a bounding box in the XZ plane. Currently implemented as a rectangle, but designed for extension to other shapes:

```rust
pub enum XZBBox {
    Rect(XZBBoxRect),
}
```

#### Features

- **Creation**:
  - `XZBBox::rect_from_xz_lengths(length_x, length_z)` - Creates a rectangle from dimensions
- **Bounds Checking**:
  - `contains(&XZPoint)` - Checks if a point is inside the bounding box
- **Boundary Access**:
  - `min_x()`, `max_x()`, `min_z()`, `max_z()` - Get the bounds of the box
- **Manipulation**:
  - Translation via `+` and `-` operators with `XZVector`

#### Usage Examples

```rust
// Create a bounding box for a 100x100 area
let bbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();

// Check if a point is inside the box
let point = XZPoint::new(50, 50);
let is_inside = bbox.contains(&point);  // true

// Get the dimensions
let width = bbox.max_x() - bbox.min_x();  // 100
let height = bbox.max_z() - bbox.min_z(); // 100

// Move the bounding box
let vector = XZVector { dx: 10, dz: 20 };
let moved_bbox = bbox + vector;
```

## Technical Implementation Details

### Rectangle Implementation (XZBBoxRect)

The rectangle implementation ensures valid bounds with:

- Point normalization (ensuring min_point has smaller coordinates than max_point)
- Bounds validation (preventing invalid rectangles)
- Efficient containment checks

### Coordinate Transformations

While not directly part of the coordinate system module, Arnis uses multiple coordinate transformations:

1. **Geographic to Cartesian**: Converting latitude/longitude to XZ coordinates
   - Implemented in the OSM parser module
   - Uses scaling factors based on real-world distances

2. **XZ to Region/Chunk/Block**: Converting global XZ coordinates to Minecraft's internal structure
   - Implemented in the WorldEditor module
   - Uses bitwise operations for efficiency (`>>` for division, `&` for modulo)

### Error Handling

The coordinate system implements robust error handling:

- Bounding box creation returns `Result<Self, String>` with descriptive error messages
- Overflow checks prevent integer overflow for large worlds
- Validation ensures coordinates stay within Minecraft's limits

## Integration with Other Systems

### WorldEditor Integration

The coordinate system integrates seamlessly with the WorldEditor:

- `WorldEditor` receives an `&XZBBox` reference during initialization
- Block placement methods check against the bounding box for validity
- Coordinates are transformed from XZ space to Minecraft's internal representation

### OSM Parser Integration

The OSM parser uses the coordinate system for mapping real-world coordinates:

- Geographic coordinates are projected into the XZ plane
- A scaling factor is applied based on the desired level of detail
- The resulting XZ coordinates are used for element placement

## Performance Considerations

The coordinate system is optimized for performance:

1. **Memory Efficiency**:
   - Small struct sizes (two `i32` fields for points and vectors)
   - No heap allocations in the core types

2. **Computational Efficiency**:
   - `#[inline]` attributes on critical methods
   - Use of integer arithmetic where possible
   - Efficient bound checking

3. **Minimized Allocations**:
   - Operations modify in place where appropriate (`add_assign`, `sub_assign`)

## Future Extension Points

The coordinate system is designed to be extensible:

1. **Additional Shapes**:
   - The `XZBBox` enum can be extended to support circles, polygons, or other shapes
   - Each shape would implement the same interface for containment checks and bounds

2. **3D Coordinate Support**:
   - The pattern could be extended to fully support Y coordinates for more complex 3D operations
   - This would enable advanced operations like heightmap manipulation and 3D transformations

3. **Coordinate Systems**:
   - Other projections could be implemented beyond Cartesian
   - More sophisticated geographic projections could be added for higher accuracy

## Conclusion

The coordinate system in Arnis provides a robust foundation for spatial operations. Its clean, modular design enables straightforward mapping between real-world geographic data and Minecraft's block-based world, while its extensible nature allows for future enhancements as the project evolves.
