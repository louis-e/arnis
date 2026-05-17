//! glTF (.glb) → triangles → voxels via dda-voxelize.

use crate::block_definitions::{Block, STONE_BRICKS};
use crate::three_dmr::palette::closest_block;
use dda_voxelize::DdaVoxelizer;

/// Composed vertex transform: intrinsic glTF/3DMR step, then world placement step.
#[derive(Clone, Copy, Debug)]
pub struct WorldTransform {
    intrinsic_scale: f32,
    intrinsic_rot_cos: f32,
    intrinsic_rot_sin: f32,
    intrinsic_tx: f32,
    intrinsic_ty: f32,
    intrinsic_tz: f32,
    world_scale: f32,
    world_rot_cos: f32,
    world_rot_sin: f32,
    world_tx: f32,
    world_ty: f32,
    world_tz: f32,
}

impl WorldTransform {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        intrinsic_yaw_degrees: f64,
        intrinsic_scale: f64,
        intrinsic_translation: [f64; 3],
        world_scale: f64,
        world_yaw_degrees: f64,
        anchor_x: f32,
        anchor_y: f32,
        anchor_z: f32,
    ) -> Self {
        let it = (intrinsic_yaw_degrees as f32).to_radians();
        let wt = (world_yaw_degrees as f32).to_radians();
        Self {
            intrinsic_scale: intrinsic_scale as f32,
            intrinsic_rot_cos: it.cos(),
            intrinsic_rot_sin: it.sin(),
            intrinsic_tx: intrinsic_translation[0] as f32,
            intrinsic_ty: intrinsic_translation[1] as f32,
            intrinsic_tz: intrinsic_translation[2] as f32,
            world_scale: world_scale as f32,
            world_rot_cos: wt.cos(),
            world_rot_sin: wt.sin(),
            world_tx: anchor_x,
            world_ty: anchor_y,
            world_tz: anchor_z,
        }
    }

    #[inline]
    fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let sx = p[0] * self.intrinsic_scale;
        let sy = p[1] * self.intrinsic_scale;
        let sz = p[2] * self.intrinsic_scale;
        let r1x = sx * self.intrinsic_rot_cos - sz * self.intrinsic_rot_sin + self.intrinsic_tx;
        let r1y = sy + self.intrinsic_ty;
        let r1z = sx * self.intrinsic_rot_sin + sz * self.intrinsic_rot_cos + self.intrinsic_tz;

        let mx = r1x * self.world_scale;
        let my = r1y * self.world_scale;
        let mz = r1z * self.world_scale;
        let r2x = mx * self.world_rot_cos - mz * self.world_rot_sin + self.world_tx;
        let r2y = my + self.world_ty;
        let r2z = mx * self.world_rot_sin + mz * self.world_rot_cos + self.world_tz;
        [r2x, r2y, r2z]
    }
}

/// True when baseColorFactor matches glTF's default-white (≈1,1,1).
fn is_default_white(c: [f32; 4]) -> bool {
    (c[0] - 1.0).abs() < 1e-3 && (c[1] - 1.0).abs() < 1e-3 && (c[2] - 1.0).abs() < 1e-3
}

/// Multiplies a 4×4 column-major matrix by a position vector (homogeneous w=1).
#[inline]
fn transform_point(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[1][0] * p[1] + m[2][0] * p[2] + m[3][0],
        m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1],
        m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2],
    ]
}

#[inline]
fn mat_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            out[i][j] =
                a[0][j] * b[i][0] + a[1][j] * b[i][1] + a[2][j] * b[i][2] + a[3][j] * b[i][3];
        }
    }
    out
}

const IDENT: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Per-voxel: RGB sampled from the primitive's material plus an "uncolored" flag.
type VoxelValue = ([f32; 3], bool);

/// Voxelizes a .glb model and returns each occupied voxel with its chosen Minecraft block.
pub fn voxelize_glb(
    glb_bytes: &[u8],
    transform: WorldTransform,
) -> Result<Vec<([i32; 3], Block)>, String> {
    let (document, buffers, images) =
        gltf::import_slice(glb_bytes).map_err(|e| format!("glTF parse: {e}"))?;

    let mut voxelizer: DdaVoxelizer<VoxelValue> = DdaVoxelizer::new();

    let scenes: Vec<_> = if let Some(scene) = document.default_scene() {
        scene.nodes().collect()
    } else {
        document.scenes().flat_map(|s| s.nodes()).collect()
    };

    let mut stack: Vec<(gltf::Node, [[f32; 4]; 4])> =
        scenes.into_iter().map(|n| (n, IDENT)).collect();

    while let Some((node, parent)) = stack.pop() {
        let local = node.transform().matrix();
        let world = mat_mul(&parent, &local);

        if let Some(mesh) = node.mesh() {
            for primitive in mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }

                let mat = primitive.material();
                let pbr = mat.pbr_metallic_roughness();
                let factor = pbr.base_color_factor();
                let texture_avg = pbr
                    .base_color_texture()
                    .and_then(|info| {
                        let idx = info.texture().source().index();
                        images.get(idx)
                    })
                    .and_then(image_average_color);

                let final_color = if let Some(tex) = texture_avg {
                    [factor[0] * tex[0], factor[1] * tex[1], factor[2] * tex[2]]
                } else {
                    [factor[0], factor[1], factor[2]]
                };
                let uncolored = texture_avg.is_none() && is_default_white(factor);
                let value: VoxelValue = (final_color, uncolored);

                let shader =
                    |prev: Option<&VoxelValue>, _: [i32; 3], _: [f32; 3]| *prev.unwrap_or(&value);

                let reader = primitive.reader(|b| Some(&buffers[b.index()]));
                let Some(positions) = reader.read_positions() else {
                    continue;
                };
                let positions: Vec<[f32; 3]> = positions.collect();
                let indices: Vec<u32> = if let Some(idx) = reader.read_indices() {
                    idx.into_u32().collect()
                } else {
                    (0..positions.len() as u32).collect()
                };

                for tri in indices.chunks_exact(3) {
                    let a = positions[tri[0] as usize];
                    let b = positions[tri[1] as usize];
                    let c = positions[tri[2] as usize];
                    let wa = transform.apply(transform_point(&world, a));
                    let wb = transform.apply(transform_point(&world, b));
                    let wc = transform.apply(transform_point(&world, c));
                    if !triangle_finite(&wa, &wb, &wc) {
                        continue;
                    }
                    voxelizer.add_triangle(&[wa, wb, wc], &shader);
                }
            }
        }

        for child in node.children() {
            stack.push((child, world));
        }
    }

    let occupied = voxelizer.finalize();
    let mut out = Vec::with_capacity(occupied.len());
    for (pos, (color, uncolored)) in occupied {
        let block = if uncolored {
            STONE_BRICKS
        } else {
            let r = (color[0].clamp(0.0, 1.0) * 255.0) as u8;
            let g = (color[1].clamp(0.0, 1.0) * 255.0) as u8;
            let b = (color[2].clamp(0.0, 1.0) * 255.0) as u8;
            closest_block((r, g, b))
        };
        out.push((pos, block));
    }
    Ok(out)
}

/// Mean RGB (0..1) across non-transparent pixels of a decoded glTF image.
fn image_average_color(img: &gltf::image::Data) -> Option<[f32; 3]> {
    use gltf::image::Format;
    let (stride, has_alpha) = match img.format {
        Format::R8G8B8 => (3, false),
        Format::R8G8B8A8 => (4, true),
        _ => return None,
    };
    let mut sum_r: u64 = 0;
    let mut sum_g: u64 = 0;
    let mut sum_b: u64 = 0;
    let mut count: u64 = 0;
    for chunk in img.pixels.chunks_exact(stride) {
        if has_alpha && chunk[3] < 16 {
            continue;
        }
        sum_r += chunk[0] as u64;
        sum_g += chunk[1] as u64;
        sum_b += chunk[2] as u64;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    Some([
        sum_r as f32 / count as f32 / 255.0,
        sum_g as f32 / count as f32 / 255.0,
        sum_b as f32 / count as f32 / 255.0,
    ])
}

#[inline]
fn triangle_finite(a: &[f32; 3], b: &[f32; 3], c: &[f32; 3]) -> bool {
    [a, b, c].iter().all(|v| v.iter().all(|x| x.is_finite()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_identity() -> WorldTransform {
        WorldTransform::new(0.0, 1.0, [0.0, 0.0, 0.0], 1.0, 0.0, 0.0, 0.0, 0.0)
    }

    #[test]
    fn world_transform_identity() {
        let t = make_identity();
        let out = t.apply([1.0, 2.0, 3.0]);
        assert!((out[0] - 1.0).abs() < 1e-5);
        assert!((out[1] - 2.0).abs() < 1e-5);
        assert!((out[2] - 3.0).abs() < 1e-5);
    }

    #[test]
    fn world_transform_world_yaw_90() {
        let t = WorldTransform::new(0.0, 1.0, [0.0, 0.0, 0.0], 1.0, 90.0, 0.0, 0.0, 0.0);
        let out = t.apply([1.0, 0.0, 0.0]);
        assert!(out[0].abs() < 1e-5, "got {out:?}");
        assert!((out[2] - 1.0).abs() < 1e-5, "got {out:?}");
    }

    #[test]
    fn world_transform_intrinsic_yaw_90() {
        let t = WorldTransform::new(90.0, 1.0, [0.0, 0.0, 0.0], 1.0, 0.0, 0.0, 0.0, 0.0);
        let out = t.apply([1.0, 0.0, 0.0]);
        assert!(out[0].abs() < 1e-5, "got {out:?}");
        assert!((out[2] - 1.0).abs() < 1e-5, "got {out:?}");
    }

    #[test]
    fn world_transform_scale_translate() {
        let t = WorldTransform::new(0.0, 1.0, [0.0, 0.0, 0.0], 2.0, 0.0, 10.0, 20.0, 30.0);
        let out = t.apply([1.0, 1.0, 1.0]);
        assert_eq!(out, [12.0, 22.0, 32.0]);
    }

    #[test]
    fn world_transform_intrinsic_scale_and_translation() {
        let t = WorldTransform::new(0.0, 2.0, [5.0, 0.0, 0.0], 1.0, 0.0, 0.0, 0.0, 0.0);
        let out = t.apply([1.0, 0.0, 0.0]);
        assert_eq!(out, [7.0, 0.0, 0.0]);
    }

    #[test]
    fn default_white_detection() {
        assert!(is_default_white([1.0, 1.0, 1.0, 1.0]));
        assert!(is_default_white([1.0, 1.0, 1.0, 0.5]));
        assert!(!is_default_white([0.5, 0.5, 0.5, 1.0]));
        assert!(!is_default_white([1.0, 0.5, 1.0, 1.0]));
    }
}
