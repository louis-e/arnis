//! Shared helpers for building-related unit tests: an in-memory editor that is
//! never saved, synthetic closed building rings, and prefilled coordinate bitmaps.
//! Only compiled for tests (`#[cfg(test)]` at the module declaration).
// Consumed incrementally by the building-overhaul test modules; helpers may be
// briefly unused between stages.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;

use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::floodfill_cache::CoordinateBitmap;
use crate::osm_parser::{ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;

/// In-memory editor (never saved) over the given bounds at ground Y=0.
/// The geographic bbox is Arnis-adjacent, i.e. `Climate::Temperate`.
pub fn test_editor(xzbbox: &XZBBox) -> WorldEditor<'_> {
    test_editor_at(xzbbox, LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap())
}

/// In-memory editor over a caller-chosen geographic bbox (for climate-dependent tests).
pub fn test_editor_at(xzbbox: &XZBBox, llbbox: LLBBox) -> WorldEditor<'_> {
    WorldEditor::new(PathBuf::from("/dev/null/unused"), xzbbox, llbbox)
}

/// Builds a tag map from string pairs.
pub fn tag_map(tags: &[(&str, &str)]) -> HashMap<String, String> {
    tags.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Closed rectangular ring (corners inclusive, first node repeated as the last),
/// the shape `generate_buildings` expects for a simple building footprint.
pub fn rect_way(
    id: u64,
    x0: i32,
    z0: i32,
    x1: i32,
    z1: i32,
    tags: &[(&str, &str)],
) -> ProcessedWay {
    let corners = [(x0, z0), (x1, z0), (x1, z1), (x0, z1), (x0, z0)];
    let nodes = corners
        .iter()
        .enumerate()
        .map(|(i, &(x, z))| ProcessedNode {
            id: id * 100 + i as u64,
            tags: HashMap::new(),
            x,
            z,
        })
        .collect();
    ProcessedWay {
        id,
        nodes,
        tags: tag_map(tags),
    }
}

/// Bitmap over `xzbbox` with exactly the given cells set.
pub fn bitmap_with(xzbbox: &XZBBox, cells: &[(i32, i32)]) -> CoordinateBitmap {
    let mut bitmap = CoordinateBitmap::new(xzbbox);
    for &(x, z) in cells {
        bitmap.set(x, z);
    }
    bitmap
}

/// Bitmap over `xzbbox` with a filled rectangle (corners inclusive) set.
pub fn bitmap_with_rect(xzbbox: &XZBBox, x0: i32, z0: i32, x1: i32, z1: i32) -> CoordinateBitmap {
    let mut bitmap = CoordinateBitmap::new(xzbbox);
    for x in x0.min(x1)..=x0.max(x1) {
        for z in z0.min(z1)..=z0.max(z1) {
            bitmap.set(x, z);
        }
    }
    bitmap
}
