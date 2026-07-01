/// Generates the coordinates for a line between two points using the Bresenham algorithm.
/// The result is a vector of 3D coordinates (x, y, z).
pub fn bresenham_line(
    x1: i32,
    y1: i32,
    z1: i32,
    x2: i32,
    y2: i32,
    z2: i32,
) -> Vec<(i32, i32, i32)> {
    // Calculate max possible points needed
    let dx = if x2 > x1 { x2 - x1 } else { x1 - x2 };
    let dy = if y2 > y1 { y2 - y1 } else { y1 - y2 };
    let dz = if z2 > z1 { z2 - z1 } else { z1 - z2 };

    // Pre-allocate vector with exact size needed
    let capacity = dx.max(dy).max(dz) + 1;
    let mut points = Vec::with_capacity(capacity as usize);
    points.reserve_exact(capacity as usize);

    let xs = if x1 < x2 { 1 } else { -1 };
    let ys = if y1 < y2 { 1 } else { -1 };
    let zs = if z1 < z2 { 1 } else { -1 };

    let mut x = x1;
    let mut y = y1;
    let mut z = z1;

    // Determine dominant axis once, outside the loop
    if dx >= dy && dx >= dz {
        let mut p1 = 2 * dy - dx;
        let mut p2 = 2 * dz - dx;

        while x != x2 {
            points.push((x, y, z));

            if p1 >= 0 {
                y += ys;
                p1 -= 2 * dx;
            }
            if p2 >= 0 {
                z += zs;
                p2 -= 2 * dx;
            }
            p1 += 2 * dy;
            p2 += 2 * dz;
            x += xs;
        }
    } else if dy >= dx && dy >= dz {
        let mut p1 = 2 * dx - dy;
        let mut p2 = 2 * dz - dy;

        while y != y2 {
            points.push((x, y, z));

            if p1 >= 0 {
                x += xs;
                p1 -= 2 * dy;
            }
            if p2 >= 0 {
                z += zs;
                p2 -= 2 * dy;
            }
            p1 += 2 * dx;
            p2 += 2 * dz;
            y += ys;
        }
    } else {
        let mut p1 = 2 * dy - dz;
        let mut p2 = 2 * dx - dz;

        while z != z2 {
            points.push((x, y, z));

            if p1 >= 0 {
                y += ys;
                p1 -= 2 * dz;
            }
            if p2 >= 0 {
                x += xs;
                p2 -= 2 * dz;
            }
            p1 += 2 * dy;
            p2 += 2 * dx;
            z += zs;
        }
    }

    points.push((x2, y2, z2));
    points
}
