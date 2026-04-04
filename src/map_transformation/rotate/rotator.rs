use super::Operator;
use crate::coordinate_system::cartesian::{XZBBox, XZBBoxRect, XZPoint};
use crate::ground::{Ground, RotationMask};
use crate::osm_parser::ProcessedElement;
use serde::Deserialize;
use std::sync::Arc;

/// Rotates the entire map (elements, bounding box, elevation) by a given angle
/// around the center of the current bounding box.
#[derive(Debug, Deserialize, PartialEq)]
pub struct Rotator {
    /// Clockwise rotation angle in degrees (as seen on a map)
    pub angle_degrees: f64,
}

impl Operator for Rotator {
    fn operate(
        &self,
        elements: &mut Vec<ProcessedElement>,
        xzbbox: &mut XZBBox,
        ground: &mut Ground,
    ) {
        if let Err(e) = rotate_world(self.angle_degrees, elements, xzbbox, ground) {
            eprintln!("Rotation failed: {e}");
        }
    }

    fn repr(&self) -> String {
        format!("rotate {}°", self.angle_degrees)
    }
}

/// Create a Rotator from JSON config
pub fn rotator_from_json(config: &serde_json::Value) -> Result<Box<dyn Operator>, String> {
    let result: Result<Box<Rotator>, _> = serde_json::from_value(config.clone())
        .map(Box::new)
        .map_err(|e| e.to_string());
    result
        .map(|o| o as Box<dyn Operator>)
        .map_err(|e| format!("Rotator config format error:\n{e}"))
}

/// Apply rotation to all world data: elements, bounding box, and ground/elevation.
pub fn rotate_world(
    angle_degrees: f64,
    elements: &mut [ProcessedElement],
    xzbbox: &mut XZBBox,
    ground: &mut Ground,
) -> Result<(), String> {
    if angle_degrees.abs() < f64::EPSILON {
        return Ok(()); // No rotation needed
    }

    // Negate: the user-facing convention is positive = clockwise on the map,
    // but the internal XZ rotation formula is counterclockwise.
    let rad = (-angle_degrees).to_radians();
    let sin_r = rad.sin();
    let cos_r = rad.cos();

    // Center of rotation = center of current bounding box
    let cx = (xzbbox.min_x() + xzbbox.max_x()) as f64 / 2.0;
    let cz = (xzbbox.min_z() + xzbbox.max_z()) as f64 / 2.0;

    // Store the original bbox extents for the rotation mask and elevation sampling
    let orig_min_x = xzbbox.min_x();
    let orig_max_x = xzbbox.max_x();
    let orig_min_z = xzbbox.min_z();
    let orig_max_z = xzbbox.max_z();
    let orig_width = (orig_max_x - orig_min_x + 1) as usize;
    let orig_height = (orig_max_z - orig_min_z + 1) as usize;

    // --- 1. Compute new axis-aligned bounding box after rotation ---
    let corners = [
        (xzbbox.min_x() as f64, xzbbox.min_z() as f64),
        (xzbbox.min_x() as f64, xzbbox.max_z() as f64),
        (xzbbox.max_x() as f64, xzbbox.min_z() as f64),
        (xzbbox.max_x() as f64, xzbbox.max_z() as f64),
    ];

    let mut rotated_xs = Vec::with_capacity(4);
    let mut rotated_zs = Vec::with_capacity(4);
    for &(x, z) in &corners {
        let (rx, rz) = rotate_point(x, z, cx, cz, sin_r, cos_r);
        rotated_xs.push(rx);
        rotated_zs.push(rz);
    }

    let new_min_x = rotated_xs
        .iter()
        .cloned()
        .fold(f64::INFINITY, f64::min)
        .floor() as i32;
    let new_max_x = rotated_xs
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil() as i32;
    let new_min_z = rotated_zs
        .iter()
        .cloned()
        .fold(f64::INFINITY, f64::min)
        .floor() as i32;
    let new_max_z = rotated_zs
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max)
        .ceil() as i32;

    *xzbbox = XZBBox::Rect(XZBBoxRect::new(
        XZPoint {
            x: new_min_x,
            z: new_min_z,
        },
        XZPoint {
            x: new_max_x,
            z: new_max_z,
        },
    )?);

    // --- 2. Rotate all elements ---
    for elem in elements.iter_mut() {
        match elem {
            ProcessedElement::Node(node) => {
                let (rx, rz) = rotate_point(node.x as f64, node.z as f64, cx, cz, sin_r, cos_r);
                node.x = rx.round() as i32;
                node.z = rz.round() as i32;
            }
            ProcessedElement::Way(way) => {
                for node in way.nodes.iter_mut() {
                    let (rx, rz) = rotate_point(node.x as f64, node.z as f64, cx, cz, sin_r, cos_r);
                    node.x = rx.round() as i32;
                    node.z = rz.round() as i32;
                }
            }
            ProcessedElement::Relation(rel) => {
                for member in rel.members.iter_mut() {
                    let way = Arc::make_mut(&mut member.way);
                    for node in way.nodes.iter_mut() {
                        let (rx, rz) =
                            rotate_point(node.x as f64, node.z as f64, cx, cz, sin_r, cos_r);
                        node.x = rx.round() as i32;
                        node.z = rz.round() as i32;
                    }
                }
            }
        }
    }

    // --- 3. Rotate elevation and land-cover data ---
    rotate_ground_data(
        ground,
        xzbbox,
        orig_min_x,
        orig_min_z,
        orig_width,
        orig_height,
        cx,
        cz,
        sin_r,
        cos_r,
    );

    // --- 4. Set rotation mask so ground generation skips out-of-bounds blocks ---
    ground.set_rotation_mask(RotationMask {
        cx,
        cz,
        neg_sin: -sin_r,
        cos: cos_r,
        orig_min_x,
        orig_max_x,
        orig_min_z,
        orig_max_z,
    });

    Ok(())
}

/// Rotate a single (x, z) point around center (cx, cz).
/// Counterclockwise rotation in the XZ plane.
#[inline]
fn rotate_point(x: f64, z: f64, cx: f64, cz: f64, sin_r: f64, cos_r: f64) -> (f64, f64) {
    let dx = x - cx;
    let dz = z - cz;
    let rx = dx * cos_r + dz * sin_r + cx;
    let rz = -dx * sin_r + dz * cos_r + cz;
    (rx, rz)
}

/// Rotate a single integer (x, z) point by `angle_degrees` around the center of `xzbbox`.
/// Used by CLI and GUI to rotate spawn points to match the rotated world.
pub fn rotate_xz_point(x: i32, z: i32, angle_degrees: f64, xzbbox: &XZBBox) -> (i32, i32) {
    if angle_degrees.abs() < f64::EPSILON {
        return (x, z);
    }
    let rad = (-angle_degrees).to_radians();
    let cx = (xzbbox.min_x() + xzbbox.max_x()) as f64 / 2.0;
    let cz = (xzbbox.min_z() + xzbbox.max_z()) as f64 / 2.0;
    let (rx, rz) = rotate_point(x as f64, z as f64, cx, cz, rad.sin(), rad.cos());
    (rx.round() as i32, rz.round() as i32)
}

/// Rotate elevation grid and land-cover data, applying Laplacian smoothing to
/// reduce jagged edges from coordinate discretization during rotation.
#[allow(clippy::too_many_arguments)]
fn rotate_ground_data(
    ground: &mut Ground,
    xzbbox: &XZBBox,
    orig_min_x: i32,
    orig_min_z: i32,
    orig_width: usize,
    orig_height: usize,
    cx: f64,
    cz: f64,
    sin_r: f64,
    cos_r: f64,
) {
    // Check elevation_enabled BEFORE cloning to avoid unnecessary allocation
    if !ground.elevation_enabled {
        return;
    }

    let original_ground = ground.clone();

    let new_w = (xzbbox.max_x() - xzbbox.min_x() + 1) as usize;
    let new_h = (xzbbox.max_z() - xzbbox.min_z() + 1) as usize;

    // For each cell in the new grid, inverse-rotate to find the source cell
    let neg_sin_r = -sin_r; // Inverse rotation
    let mut new_heights: Vec<Vec<i32>> = Vec::with_capacity(new_h);
    let mut has_data: Vec<Vec<bool>> = Vec::with_capacity(new_h);

    // Also rotate land-cover grids if present
    let has_land_cover = original_ground.has_land_cover();
    let mut new_cover: Option<Vec<Vec<u8>>> = has_land_cover.then(|| Vec::with_capacity(new_h));
    let mut new_water: Option<Vec<Vec<u8>>> = has_land_cover.then(|| Vec::with_capacity(new_h));

    for z_idx in 0..new_h {
        let mut height_row = Vec::with_capacity(new_w);
        let mut data_row = Vec::with_capacity(new_w);
        let mut cover_row: Option<Vec<u8>> = has_land_cover.then(|| Vec::with_capacity(new_w));
        let mut water_row: Option<Vec<u8>> = has_land_cover.then(|| Vec::with_capacity(new_w));

        for x_idx in 0..new_w {
            let world_x = xzbbox.min_x() + x_idx as i32;
            let world_z = xzbbox.min_z() + z_idx as i32;

            // Inverse-rotate this world coordinate back to original space
            let (orig_x, orig_z) =
                rotate_point(world_x as f64, world_z as f64, cx, cz, neg_sin_r, cos_r);

            // Convert to coordinates relative to the original bbox origin,
            // which is what Ground::level / cover_class / water_distance expect
            let rel_x = orig_x.round() as i32 - orig_min_x;
            let rel_z = orig_z.round() as i32 - orig_min_z;

            // Full bounds check: both lower AND upper against original grid dimensions
            let in_original = rel_x >= 0
                && rel_z >= 0
                && (rel_x as usize) < orig_width
                && (rel_z as usize) < orig_height;

            let coord = XZPoint::new(rel_x, rel_z);
            height_row.push(original_ground.level(coord));
            data_row.push(in_original);

            if let Some(ref mut cr) = cover_row {
                cr.push(original_ground.cover_class(coord));
            }
            if let Some(ref mut wr) = water_row {
                wr.push(original_ground.water_distance(coord));
            }
        }
        new_heights.push(height_row);
        has_data.push(data_row);
        if let Some(ref mut cg) = new_cover {
            cg.push(cover_row.unwrap());
        }
        if let Some(ref mut wd) = new_water {
            wd.push(water_row.unwrap());
        }
    }

    // Apply Laplacian smoothing (3 iterations) to reduce jagged edges
    // from coordinate discretization during rotation
    const SMOOTH_ITERATIONS: usize = 3;
    for _ in 0..SMOOTH_ITERATIONS {
        let prev = new_heights.clone();
        for z_idx in 1..new_h.saturating_sub(1) {
            for x_idx in 1..new_w.saturating_sub(1) {
                if !has_data[z_idx][x_idx] {
                    continue; // Don't smooth padding areas
                }
                let neighbors_sum = prev[z_idx - 1][x_idx] as f64
                    + prev[z_idx + 1][x_idx] as f64
                    + prev[z_idx][x_idx - 1] as f64
                    + prev[z_idx][x_idx + 1] as f64;
                let avg = neighbors_sum / 4.0;
                // Blend: 70% original + 30% neighbor average
                new_heights[z_idx][x_idx] =
                    (0.7 * prev[z_idx][x_idx] as f64 + 0.3 * avg).round() as i32;
            }
        }
    }

    // Update ground with rotated elevation
    ground.set_elevation_data(new_heights, new_w, new_h);

    // Update land cover with rotated data
    if let (Some(cover_grid), Some(water_dist)) = (new_cover, new_water) {
        ground.set_land_cover_data(cover_grid, water_dist, new_w, new_h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_rotation_is_noop() {
        let mut elements = Vec::new();
        let mut xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut ground = Ground::new_flat(-62);

        let original_bbox = xzbbox.clone();
        rotate_world(0.0, &mut elements, &mut xzbbox, &mut ground).unwrap();

        assert_eq!(xzbbox.min_x(), original_bbox.min_x());
        assert_eq!(xzbbox.max_x(), original_bbox.max_x());
    }

    #[test]
    fn test_rotate_point_90_degrees() {
        let rad = 90.0_f64.to_radians();
        let (rx, rz) = rotate_point(10.0, 0.0, 0.0, 0.0, rad.sin(), rad.cos());
        // 90° CCW: (10, 0) -> (0, -10)
        assert!((rx - 0.0).abs() < 1e-10);
        assert!((rz - (-10.0)).abs() < 1e-10);
    }

    #[test]
    fn test_bbox_expands_on_45deg_rotation() {
        let mut elements = Vec::new();
        let mut xzbbox = XZBBox::rect_from_xz_lengths(100.0, 100.0).unwrap();
        let mut ground = Ground::new_flat(-62);

        let orig_area = xzbbox.bounding_rect().total_blocks();
        rotate_world(45.0, &mut elements, &mut xzbbox, &mut ground).unwrap();
        let new_area = xzbbox.bounding_rect().total_blocks();

        // 45° rotation of a square produces a larger bounding rect
        assert!(new_area > orig_area);
    }
}
