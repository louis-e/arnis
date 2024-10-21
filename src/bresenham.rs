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
    let mut points: Vec<(i32, i32, i32)> = Vec::new();

    let dx: i32 = (x2 - x1).abs();
    let dy: i32 = (y2 - y1).abs();
    let dz: i32 = (z2 - z1).abs();

    let xs: i32 = if x1 < x2 { 1 } else { -1 };
    let ys: i32 = if y1 < y2 { 1 } else { -1 };
    let zs: i32 = if z1 < z2 { 1 } else { -1 };

    let mut x: i32 = x1;
    let mut y: i32 = y1;
    let mut z: i32 = z1;

    if dx >= dy && dx >= dz {
        let mut p1: i32 = 2 * dy - dx;
        let mut p2: i32 = 2 * dz - dx;

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
        let mut p1: i32 = 2 * dx - dy;
        let mut p2: i32 = 2 * dz - dy;

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
        let mut p1: i32 = 2 * dy - dz;
        let mut p2: i32 = 2 * dx - dz;

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
