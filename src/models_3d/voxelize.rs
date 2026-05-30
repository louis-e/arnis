//! glTF (.glb) → triangles → voxels via dda-voxelize.

use crate::block_definitions::{Block, GLASS, STONE_BRICKS};
use crate::models_3d::palette::closest_block;
use dda_voxelize::DdaVoxelizer;

/// Pure magenta (#FF00FF) in model colors becomes GLASS instead of palette-mapping.
const GLASS_SENTINEL: [f32; 3] = [1.0, 0.0, 1.0];
const GLASS_SENTINEL_TOL: f32 = 1e-3;

#[inline]
fn is_glass_sentinel(c: [f32; 3]) -> bool {
    (c[0] - GLASS_SENTINEL[0]).abs() < GLASS_SENTINEL_TOL
        && (c[1] - GLASS_SENTINEL[1]).abs() < GLASS_SENTINEL_TOL
        && (c[2] - GLASS_SENTINEL[2]).abs() < GLASS_SENTINEL_TOL
}

/// Composed vertex transform: intrinsic glTF/3DMR step, then world placement step.
#[derive(Clone, Copy, Debug)]
pub struct WorldTransform {
    intrinsic_scale: f32,
    intrinsic_rot_cos: f32,
    intrinsic_rot_sin: f32,
    intrinsic_tx: f32,
    intrinsic_ty: f32,
    intrinsic_tz: f32,
    /// Model-space pitch about X (identity when sin=0), applied before scale/yaw.
    pitch_cos: f32,
    pitch_sin: f32,
    world_scale: [f32; 3],
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
        let s = world_scale as f32;
        Self::with_world_scale_xyz(
            intrinsic_yaw_degrees,
            intrinsic_scale,
            intrinsic_translation,
            [s, s, s],
            world_yaw_degrees,
            anchor_x,
            anchor_y,
            anchor_z,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_world_scale_xyz(
        intrinsic_yaw_degrees: f64,
        intrinsic_scale: f64,
        intrinsic_translation: [f64; 3],
        world_scale: [f32; 3],
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
            pitch_cos: 1.0,
            pitch_sin: 0.0,
            world_scale,
            world_rot_cos: wt.cos(),
            world_rot_sin: wt.sin(),
            world_tx: anchor_x,
            world_ty: anchor_y,
            world_tz: anchor_z,
        }
    }

    /// Tilts the model nose-up by `pitch_degrees` about X, so a +Z forward axis climbs into +Y.
    pub fn pitched(mut self, pitch_degrees: f64) -> Self {
        let p = (pitch_degrees as f32).to_radians();
        self.pitch_cos = p.cos();
        self.pitch_sin = p.sin();
        self
    }

    #[inline]
    fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        // Model-space pitch about X: +Z forward tilts up into +Y. Identity when pitch_sin == 0.
        let px = p[0];
        let py = p[1] * self.pitch_cos + p[2] * self.pitch_sin;
        let pz = -p[1] * self.pitch_sin + p[2] * self.pitch_cos;
        let sx = px * self.intrinsic_scale;
        let sy = py * self.intrinsic_scale;
        let sz = pz * self.intrinsic_scale;
        let r1x = sx * self.intrinsic_rot_cos - sz * self.intrinsic_rot_sin + self.intrinsic_tx;
        let r1y = sy + self.intrinsic_ty;
        let r1z = sx * self.intrinsic_rot_sin + sz * self.intrinsic_rot_cos + self.intrinsic_tz;

        let mx = r1x * self.world_scale[0];
        let my = r1y * self.world_scale[1];
        let mz = r1z * self.world_scale[2];
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

/// World-space bbox of all triangle vertices in a .glb.
pub fn glb_model_bbox(glb_bytes: &[u8]) -> Result<([f32; 3], [f32; 3]), String> {
    let (document, buffers, _) =
        gltf::import_slice(glb_bytes).map_err(|e| format!("glTF parse: {e}"))?;
    let scenes: Vec<_> = if let Some(scene) = document.default_scene() {
        scene.nodes().collect()
    } else {
        document.scenes().flat_map(|s| s.nodes()).collect()
    };
    let mut stack: Vec<(gltf::Node, [[f32; 4]; 4])> =
        scenes.into_iter().map(|n| (n, IDENT)).collect();
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let mut any = false;
    while let Some((node, parent)) = stack.pop() {
        let world = mat_mul(&parent, &node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            for primitive in mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }
                let reader = primitive.reader(|b| Some(&buffers[b.index()]));
                let Some(positions) = reader.read_positions() else {
                    continue;
                };
                for v in positions {
                    let p = transform_point(&world, v);
                    for i in 0..3 {
                        if p[i].is_finite() {
                            if p[i] < min[i] {
                                min[i] = p[i];
                            }
                            if p[i] > max[i] {
                                max[i] = p[i];
                            }
                            any = true;
                        }
                    }
                }
            }
        }
        for child in node.children() {
            stack.push((child, world));
        }
    }
    if !any {
        return Err("glTF has no finite vertices".into());
    }
    Ok((min, max))
}

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

                let material_color = if let Some(tex) = texture_avg {
                    [factor[0] * tex[0], factor[1] * tex[1], factor[2] * tex[2]]
                } else {
                    [factor[0], factor[1], factor[2]]
                };
                let material_uncolored = texture_avg.is_none() && is_default_white(factor);
                let material_value: VoxelValue = (material_color, material_uncolored);

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
                // Vertex colors (COLOR_0) modulate the material color when present.
                let vertex_colors: Option<Vec<[f32; 4]>> =
                    reader.read_colors(0).map(|c| c.into_rgba_f32().collect());

                for tri in indices.chunks_exact(3) {
                    let ia = tri[0] as usize;
                    let ib = tri[1] as usize;
                    let ic = tri[2] as usize;
                    let a = positions[ia];
                    let b = positions[ib];
                    let c = positions[ic];
                    let wa = transform.apply(transform_point(&world, a));
                    let wb = transform.apply(transform_point(&world, b));
                    let wc = transform.apply(transform_point(&world, c));
                    if !triangle_finite(&wa, &wb, &wc) {
                        continue;
                    }

                    let value: VoxelValue = match vertex_colors.as_ref() {
                        Some(vc) if ia < vc.len() && ib < vc.len() && ic < vc.len() => {
                            let ca = vc[ia];
                            let cb = vc[ib];
                            let cc = vc[ic];
                            let avg = [
                                (ca[0] + cb[0] + cc[0]) / 3.0 * material_color[0],
                                (ca[1] + cb[1] + cc[1]) / 3.0 * material_color[1],
                                (ca[2] + cb[2] + cc[2]) / 3.0 * material_color[2],
                            ];
                            (avg, false)
                        }
                        _ => material_value,
                    };
                    let shader = |prev: Option<&VoxelValue>, _: [i32; 3], _: [f32; 3]| {
                        *prev.unwrap_or(&value)
                    };
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
        } else if is_glass_sentinel(color) {
            GLASS
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

/// Voxelizes pre-transformed triangles, painting every occupied voxel as `block`.
pub fn voxelize_uniform_triangles<I>(
    triangles: I,
    transform: WorldTransform,
    block: Block,
) -> Vec<([i32; 3], Block)>
where
    I: IntoIterator<Item = [[f32; 3]; 3]>,
{
    let mut voxelizer: DdaVoxelizer<()> = DdaVoxelizer::new();
    let shader = |_: Option<&()>, _: [i32; 3], _: [f32; 3]| {};
    for tri in triangles {
        let wa = transform.apply(tri[0]);
        let wb = transform.apply(tri[1]);
        let wc = transform.apply(tri[2]);
        if !triangle_finite(&wa, &wb, &wc) {
            continue;
        }
        voxelizer.add_triangle(&[wa, wb, wc], &shader);
    }
    voxelizer
        .finalize()
        .into_keys()
        .map(|pos| (pos, block))
        .collect()
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
    fn world_transform_nonuniform_xyz_scale() {
        let t = WorldTransform::with_world_scale_xyz(
            0.0,
            1.0,
            [0.0, 0.0, 0.0],
            [3.0, 5.0, 7.0],
            0.0,
            0.0,
            0.0,
            0.0,
        );
        let out = t.apply([1.0, 1.0, 1.0]);
        assert_eq!(out, [3.0, 5.0, 7.0]);
    }

    #[test]
    fn pitch_identity_when_unset() {
        // Default transform must behave exactly as before (no tilt).
        let t = make_identity();
        let out = t.apply([0.0, 0.0, 1.0]);
        assert!((out[0]).abs() < 1e-5 && (out[1]).abs() < 1e-5 && (out[2] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn pitch_90_lifts_forward_axis_up() {
        // A +Z nose vertex pitched 90° nose-up lands on +Y.
        let t = make_identity().pitched(90.0);
        let out = t.apply([0.0, 0.0, 1.0]);
        assert!(out[0].abs() < 1e-5, "got {out:?}");
        assert!((out[1] - 1.0).abs() < 1e-5, "got {out:?}");
        assert!(out[2].abs() < 1e-5, "got {out:?}");
    }

    #[test]
    fn pitch_positive_raises_nose_partially() {
        // A modest nose-up pitch gives the +Z nose a positive Y and shorter Z.
        let t = make_identity().pitched(30.0);
        let out = t.apply([0.0, 0.0, 1.0]);
        assert!(out[1] > 0.0, "nose should rise, got {out:?}");
        assert!(
            out[2] > 0.0 && out[2] < 1.0,
            "nose Z should shorten, got {out:?}"
        );
        // Tail (-Z) drops below the pitch axis.
        let tail = t.apply([0.0, 0.0, -1.0]);
        assert!(tail[1] < 0.0, "tail should drop, got {tail:?}");
    }

    #[test]
    fn glass_sentinel_matches_pure_magenta() {
        assert!(is_glass_sentinel([1.0, 0.0, 1.0]));
        assert!(is_glass_sentinel([0.9995, 0.0005, 0.9998]));
        assert!(!is_glass_sentinel([1.0, 0.0, 0.9]));
        assert!(!is_glass_sentinel([1.0, 0.1, 1.0]));
        assert!(!is_glass_sentinel([0.9, 0.0, 1.0]));
        // Reddish-pink shouldn't trigger.
        assert!(!is_glass_sentinel([1.0, 0.5, 0.8]));
    }

    #[test]
    fn default_white_detection() {
        assert!(is_default_white([1.0, 1.0, 1.0, 1.0]));
        assert!(is_default_white([1.0, 1.0, 1.0, 0.5]));
        assert!(!is_default_white([0.5, 0.5, 0.5, 1.0]));
        assert!(!is_default_white([1.0, 0.5, 1.0, 1.0]));
    }
}
