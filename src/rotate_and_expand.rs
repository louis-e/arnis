use crate::coordinate_system::cartesian::{XZBBox, XZPoint, XZBBoxRect};
use crate::osm_parser::ProcessedElement;
use crate::ground::Ground;

// Function to perform rotation and expansion
pub fn rotate_and_expand_world_data(
    elements: &mut Vec<ProcessedElement>,
    xzbbox: &mut XZBBox,
    ground: &mut Ground,
    rotation_angle: f64,
) -> Result<(), String> {
    if rotation_angle.abs() < std::f64::EPSILON {
        return Ok(()); // Do nothing if the angle is 0
    }

    let cx = (xzbbox.min_x() + xzbbox.max_x()) as f64 / 2.0;
    let cz = (xzbbox.min_z() + xzbbox.max_z()) as f64 / 2.0;

    let rad = rotation_angle.to_radians();
    let sin_r = rad.sin();
    let cos_r = rad.cos();

    // Rotate XZBBox
    let corners = vec![
        (xzbbox.min_x() as f64, xzbbox.min_z() as f64),
        (xzbbox.min_x() as f64, xzbbox.max_z() as f64),
        (xzbbox.max_x() as f64, xzbbox.min_z() as f64),
        (xzbbox.max_x() as f64, xzbbox.max_z() as f64),
    ];

    let mut xs = vec![];
    let mut zs = vec![];
    for &(x, z) in &corners {
        let x0 = x - cx;
        let z0 = z - cz;
        xs.push(x0 * cos_r + z0 * sin_r);
        zs.push(-x0 * sin_r + z0 * cos_r);
    }

    let min_x_rot = xs.iter().cloned().fold(f64::INFINITY, f64::min).floor() as i32 + cx.round() as i32;
    let max_x_rot = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max).ceil() as i32 + cx.round() as i32;
    let min_z_rot = zs.iter().cloned().fold(f64::INFINITY, f64::min).floor() as i32 + cz.round() as i32;
    let max_z_rot = zs.iter().cloned().fold(f64::NEG_INFINITY, f64::max).ceil() as i32 + cz.round() as i32;

    *xzbbox = XZBBox::Rect(XZBBoxRect::new(
        XZPoint { x: min_x_rot, z: min_z_rot },
        XZPoint { x: max_x_rot, z: max_z_rot },
    )?);

    // Rotate elements
    for elem in elements.iter_mut() {
        match elem {
            ProcessedElement::Node(node) => {
                let x0 = node.x as f64 - cx;
                let z0 = node.z as f64 - cz;
                node.x = (x0 * cos_r + z0 * sin_r + cx).round() as i32;
                node.z = (-x0 * sin_r + z0 * cos_r + cz).round() as i32;
            }
            ProcessedElement::Way(way) => {
                for node in way.nodes.iter_mut() {
                    let x0 = node.x as f64 - cx;
                    let z0 = node.z as f64 - cz;
                    node.x = (x0 * cos_r + z0 * sin_r + cx).round() as i32;
                    node.z = (-x0 * sin_r + z0 * cos_r + cz).round() as i32;
                }
            }
            ProcessedElement::Relation(rel) => {
                for member in rel.members.iter_mut() {
                    for node in member.way.nodes.iter_mut() {
                        let x0 = node.x as f64 - cx;
                        let z0 = node.z as f64 - cz;
                        node.x = (x0 * cos_r + z0 * sin_r + cx).round() as i32;
                        node.z = (-x0 * sin_r + z0 * cos_r + cz).round() as i32;
                    }
                }
            }
        }
    }

    // Rotate Ground
    if let Some(elev_data) = ground.elevation_data_mut() {
        let old_w = elev_data.width;
        let old_h = elev_data.height;
        let new_w = (xzbbox.max_x() - xzbbox.min_x() + 1) as usize;
        let new_h = (xzbbox.max_z() - xzbbox.min_z() + 1) as usize;

        // new_heights: rotated heights (None = empty)
        let mut new_heights = vec![vec![None; new_w]; new_h];
        // has_original_data: marks original elevation cells
        let mut has_original_data = vec![vec![false; new_w]; new_h];

        // Map old heights to rotated coordinates and mark original cells
        for y in 0..old_h {
            for x in 0..old_w {
                let h = elev_data.heights[y][x];

                let xf = x as f64 + 0.5 - old_w as f64 / 2.0;
                let zf = y as f64 + 0.5 - old_h as f64 / 2.0;

                let x_rot = xf * cos_r + zf * sin_r;
                let z_rot = -xf * sin_r + zf * cos_r;

                let xi = (x_rot + new_w as f64 / 2.0).round() as usize;
                let zi = (z_rot + new_h as f64 / 2.0).round() as usize;

                if zi < new_h && xi < new_w {
                    new_heights[zi][xi] = Some(h);
                    has_original_data[zi][xi] = true; // Mark original data
                }
            }
        }

        // Fill holes using neighbors, leaving edges untouched
        let mut filled = true;
        while filled {
            filled = false;
            for y in 0..new_h {
                for x in 0..new_w {
                    if new_heights[y][x].is_none() {
                        let mut sum = 0;
                        let mut count = 0;
                        let mut neighbor_has_original = false;

                        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                            let nx = x as isize + dx;
                            let nz = y as isize + dz;
                            if nx >= 0 && nz >= 0 && (nx as usize) < new_w && (nz as usize) < new_h {
                                if let Some(h) = new_heights[nz as usize][nx as usize] {
                                    sum += h;
                                    count += 1;
                                    if has_original_data[nz as usize][nx as usize] {
                                        neighbor_has_original = true;
                                    }
                                }
                            }
                        }

                        // Only interpolate if at least one neighbor has original data
                        if count > 0 && neighbor_has_original {
                            new_heights[y][x] = Some(sum / count);
                            filled = true;
                        }
                    }
                }
            }
        }

        // Replace None with default height (-62)
        elev_data.heights = new_heights
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|opt| opt.unwrap_or(-62)) // default height
                    .collect()
            })
            .collect();

        elev_data.width = new_w;
        elev_data.height = new_h;
    }

    Ok(()) 
}