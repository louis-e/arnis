//! Minimal binary STL parser; ASCII STL is rejected.

use std::io::Read;

#[inline]
fn read_le_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

#[inline]
fn read_le_f32(b: &[u8]) -> f32 {
    f32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Parses a binary STL blob into a flat list of triangles.
pub fn parse_triangles(bytes: &[u8]) -> Result<Vec<[[f32; 3]; 3]>, String> {
    if bytes.len() < 84 {
        return Err("STL too short for header + count".into());
    }
    // ASCII STL files start with "solid"; reject unless the body matches the binary layout.
    if bytes.starts_with(b"solid") && !looks_like_binary_with_solid_header(bytes) {
        return Err("ASCII STL not supported".into());
    }
    let count = read_le_u32(&bytes[80..84]) as usize;
    let expected = 84 + count * 50;
    if bytes.len() < expected {
        return Err(format!(
            "STL truncated: header says {count} triangles, body has {} bytes (need {})",
            bytes.len() - 84,
            count * 50
        ));
    }
    let mut out = Vec::with_capacity(count);
    let mut p = 84;
    for _ in 0..count {
        let v_off = p + 12;
        let v0 = [
            read_le_f32(&bytes[v_off..]),
            read_le_f32(&bytes[v_off + 4..]),
            read_le_f32(&bytes[v_off + 8..]),
        ];
        let v1 = [
            read_le_f32(&bytes[v_off + 12..]),
            read_le_f32(&bytes[v_off + 16..]),
            read_le_f32(&bytes[v_off + 20..]),
        ];
        let v2 = [
            read_le_f32(&bytes[v_off + 24..]),
            read_le_f32(&bytes[v_off + 28..]),
            read_le_f32(&bytes[v_off + 32..]),
        ];
        out.push([v0, v1, v2]);
        p += 50;
    }
    Ok(out)
}

/// Some binary STLs start with "solid "; if the trailing length matches the count, accept anyway.
fn looks_like_binary_with_solid_header(bytes: &[u8]) -> bool {
    if bytes.len() < 84 {
        return false;
    }
    let count = read_le_u32(&bytes[80..84]) as usize;
    let expected = 84 + count * 50;
    expected == bytes.len() || (expected <= bytes.len() && expected + 4 >= bytes.len())
}

/// Loads `R` to bytes and forwards to `parse_triangles`.
#[allow(dead_code)]
pub fn parse_from_reader<R: Read>(mut r: R) -> Result<Vec<[[f32; 3]; 3]>, String> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    parse_triangles(&buf)
}

/// Axis-aligned bounding box of a triangle stream in model space.
pub fn bbox(triangles: &[[[f32; 3]; 3]]) -> Option<([f32; 3], [f32; 3])> {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let mut any = false;
    for tri in triangles {
        for v in tri {
            if !v.iter().all(|c| c.is_finite()) {
                continue;
            }
            for i in 0..3 {
                if v[i] < min[i] {
                    min[i] = v[i];
                }
                if v[i] > max[i] {
                    max[i] = v[i];
                }
            }
            any = true;
        }
    }
    if any {
        Some((min, max))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthesize_binary_stl(tris: &[[[f32; 3]; 3]]) -> Vec<u8> {
        let mut buf = vec![0u8; 80];
        buf.extend_from_slice(&(tris.len() as u32).to_le_bytes());
        for tri in tris {
            buf.extend_from_slice(&[0u8; 12]); // normal
            for v in tri {
                for c in v {
                    buf.extend_from_slice(&c.to_le_bytes());
                }
            }
            buf.extend_from_slice(&[0u8; 2]);
        }
        buf
    }

    #[test]
    fn parses_a_single_triangle() {
        let t = [[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]];
        let bytes = synthesize_binary_stl(&t);
        let parsed = parse_triangles(&bytes).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0][2], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn bbox_of_unit_cube_triangles() {
        let tris = [
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
            [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0]],
        ];
        let (min, max) = bbox(&tris).unwrap();
        assert_eq!(min, [0.0, 0.0, 0.0]);
        assert_eq!(max, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn rejects_ascii_stl() {
        let ascii = b"solid foo\nfacet normal 0 0 1\n   outer loop\n      vertex 0 0 0\n      vertex 1 0 0\n      vertex 0 1 0\n   endloop\nendfacet\nendsolid foo\n";
        let r = parse_triangles(ascii);
        assert!(r.is_err());
    }

    #[test]
    fn rejects_truncated() {
        let mut buf = vec![0u8; 84];
        buf[80] = 10; // claims 10 triangles
        let r = parse_triangles(&buf);
        assert!(r.is_err());
    }
}
