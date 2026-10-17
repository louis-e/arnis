//! glTF (.glb) → triangles → voxels via dda-voxelize.

use crate::block_definitions::{Block, GLASS, STONE_BRICKS};
use crate::block_palette::closest_block;
use dda_voxelize::DdaVoxelizer;
use std::collections::{HashMap, HashSet};

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
            world_scale,
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

/// Samples falling into one voxel: coverage-weighted sRGB plus tallies.
#[derive(Clone, Copy, Default)]
struct VoxelAccum {
    rgb: [f32; 3],
    weight: f32,
    /// Sum of per-sample coverage, normalised so 0.5 is the material's cutoff.
    coverage: f32,
    samples: u16,
    glass: u16,
    uncolored: u16,
}

impl VoxelAccum {
    fn add(mut self, s: &VoxelAccum) -> Self {
        for i in 0..3 {
            self.rgb[i] += s.rgb[i];
        }
        self.weight += s.weight;
        self.coverage += s.coverage;
        self.samples = self.samples.saturating_add(s.samples);
        self.glass = self.glass.saturating_add(s.glass);
        self.uncolored = self.uncolored.saturating_add(s.uncolored);
        self
    }
}

/// What a voxel gets painted with, before it is matched to a block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VoxelPaint {
    Rgb((u8, u8, u8)),
    Glass,
    /// No material colour at all.
    Uncolored,
}

impl VoxelPaint {
    pub fn block_with(self, pick: impl Fn((u8, u8, u8)) -> Block) -> Block {
        match self {
            VoxelPaint::Rgb(c) => pick(c),
            VoxelPaint::Glass => GLASS,
            VoxelPaint::Uncolored => STONE_BRICKS,
        }
    }
}

/// Blended materials keep a voxel once this much of it is opaque.
const BLEND_CUTOFF: f32 = 0.25;
/// Texture samples per side across a voxel's slice of a triangle, spaced
/// `FOOTPRINT_SPAN / FOOTPRINT_STEPS` apart; the span covers a diagonal slice.
const FOOTPRINT_STEPS: usize = 6;
const FOOTPRINT_SPAN: f32 = 1.5;
/// Textures are box-filtered down to this before sampling; finer detail is
/// far below a block anyway.
const MAX_TEXTURE_SIDE: usize = 512;

#[inline]
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// One mip level, premultiplied linear RGB plus alpha.
struct MipLevel {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl MipLevel {
    fn half(&self) -> MipLevel {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (2 * x + dx).min(self.w - 1);
                    let sy = (2 * y + dy).min(self.h - 1);
                    let p = self.px[sy * self.w + sx];
                    for i in 0..4 {
                        acc[i] += p[i] * 0.25;
                    }
                }
                px.push(acc);
            }
        }
        MipLevel { w, h, px }
    }
}

/// A base colour texture as a mip chain, so one sample stands for a whole voxel.
struct MipTexture {
    levels: Vec<MipLevel>,
    wrap: [gltf::texture::WrappingMode; 2],
}

impl MipTexture {
    fn new(img: &gltf::image::Data, wrap: [gltf::texture::WrappingMode; 2]) -> Option<Self> {
        use gltf::image::Format;
        let (stride, has_alpha) = match img.format {
            Format::R8G8B8 => (3, false),
            Format::R8G8B8A8 => (4, true),
            _ => return None,
        };
        let (w, h) = (img.width as usize, img.height as usize);
        if w == 0 || h == 0 || img.pixels.len() < w * h * stride {
            return None;
        }
        let mut lut = [0.0f32; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            *v = srgb_to_linear(i as f32 / 255.0);
        }
        let px = img
            .pixels
            .chunks_exact(stride)
            .take(w * h)
            .map(|c| {
                let a = if has_alpha { c[3] as f32 / 255.0 } else { 1.0 };
                [
                    lut[c[0] as usize] * a,
                    lut[c[1] as usize] * a,
                    lut[c[2] as usize] * a,
                    a,
                ]
            })
            .collect();
        let mut level = MipLevel { w, h, px };
        while level.w.max(level.h) > MAX_TEXTURE_SIDE {
            level = level.half();
        }
        let mut levels = vec![level];
        while let Some(last) = levels.last().filter(|l| l.w > 1 || l.h > 1) {
            let next = last.half();
            levels.push(next);
        }
        Some(Self { levels, wrap })
    }

    fn size(&self) -> (f32, f32) {
        (self.levels[0].w as f32, self.levels[0].h as f32)
    }

    /// Bilinear sample at the level where one texel spans about one voxel.
    fn sample(&self, uv: [f32; 2], texels_per_voxel: f32) -> [f32; 4] {
        let lod = texels_per_voxel.max(1.0).log2().round() as usize;
        let lvl = &self.levels[lod.min(self.levels.len() - 1)];
        let fx = uv[0] * lvl.w as f32 - 0.5;
        let fy = uv[1] * lvl.h as f32 - 0.5;
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let at = |x: f32, y: f32| {
            let xi = wrap_index(x as i64, lvl.w, self.wrap[0]);
            let yi = wrap_index(y as i64, lvl.h, self.wrap[1]);
            lvl.px[yi * lvl.w + xi]
        };
        let (a, b, c, d) = (
            at(x0, y0),
            at(x0 + 1.0, y0),
            at(x0, y0 + 1.0),
            at(x0 + 1.0, y0 + 1.0),
        );
        let mut out = [0.0f32; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bottom = c[i] + (d[i] - c[i]) * tx;
            out[i] = top + (bottom - top) * ty;
        }
        out
    }
}

fn wrap_index(i: i64, n: usize, mode: gltf::texture::WrappingMode) -> usize {
    use gltf::texture::WrappingMode;
    let n = n as i64;
    match mode {
        WrappingMode::ClampToEdge => i.clamp(0, n - 1) as usize,
        WrappingMode::Repeat => i.rem_euclid(n) as usize,
        WrappingMode::MirroredRepeat => {
            let m = i.rem_euclid(2 * n);
            (if m < n { m } else { 2 * n - 1 - m }) as usize
        }
    }
}

/// Per-primitive shading inputs.
struct Material<'a> {
    /// Linear base colour factor, alpha included.
    factor: [f32; 4],
    texture: Option<&'a MipTexture>,
    tex_coord: u32,
    /// Coverage below which a sample counts as see-through; None when opaque.
    cutoff: Option<f32>,
    uncolored: bool,
}

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
    Ok(voxelize_glb_paint(glb_bytes, transform)?
        .into_iter()
        .map(|(pos, paint)| (pos, paint.block_with(closest_block)))
        .collect())
}

/// Every voxel any triangle touches, with its shading samples.
fn accumulate_glb(
    glb_bytes: &[u8],
    transform: WorldTransform,
) -> Result<HashMap<[i32; 3], VoxelAccum>, String> {
    let (document, buffers, images) =
        gltf::import_slice(glb_bytes).map_err(|e| format!("glTF parse: {e}"))?;

    let mut accum: HashMap<[i32; 3], VoxelAccum> = HashMap::new();
    let mut textures: Vec<Option<Option<MipTexture>>> = (0..images.len()).map(|_| None).collect();

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
                let tex_info = pbr.base_color_texture();
                let texture = tex_info.as_ref().and_then(|info| {
                    let tex = info.texture();
                    let idx = tex.source().index();
                    let sampler = tex.sampler();
                    let slot = textures.get_mut(idx)?;
                    slot.get_or_insert_with(|| {
                        images.get(idx).and_then(|img| {
                            MipTexture::new(img, [sampler.wrap_s(), sampler.wrap_t()])
                        })
                    });
                    Some(idx)
                });
                let texture = texture.and_then(|i| textures[i].as_ref()?.as_ref());
                let material = Material {
                    factor,
                    texture,
                    tex_coord: tex_info.as_ref().map_or(0, |i| i.tex_coord()),
                    cutoff: match mat.alpha_mode() {
                        gltf::material::AlphaMode::Opaque => None,
                        gltf::material::AlphaMode::Mask => Some(mat.alpha_cutoff().unwrap_or(0.5)),
                        gltf::material::AlphaMode::Blend => Some(BLEND_CUTOFF),
                    },
                    uncolored: texture.is_none() && is_default_white(factor),
                };

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
                // Taken as authored: 3D Builder, which made the stadium, writes them as sRGB.
                let vertex_colors: Option<Vec<[f32; 4]>> =
                    reader.read_colors(0).map(|c| c.into_rgba_f32().collect());
                let uvs: Option<Vec<[f32; 2]>> = material.texture.and_then(|_| {
                    reader
                        .read_tex_coords(material.tex_coord)
                        .map(|t| t.into_f32().collect())
                });

                for &[a0, b0, c0] in indices.as_chunks::<3>().0 {
                    let (ia, ib, ic) = (a0 as usize, b0 as usize, c0 as usize);
                    if ia.max(ib).max(ic) >= positions.len() {
                        continue;
                    }
                    let wa = transform.apply(transform_point(&world, positions[ia]));
                    let wb = transform.apply(transform_point(&world, positions[ib]));
                    let wc = transform.apply(transform_point(&world, positions[ic]));
                    if !triangle_finite(&wa, &wb, &wc) {
                        continue;
                    }

                    let vcol = vertex_colors.as_ref().and_then(|vc| {
                        let (a, b, c) = (vc.get(ia)?, vc.get(ib)?, vc.get(ic)?);
                        Some([0, 1, 2, 3].map(|i| (a[i] + b[i] + c[i]) / 3.0))
                    });
                    let tri_uv = uvs
                        .as_ref()
                        .and_then(|t| Some([*t.get(ia)?, *t.get(ib)?, *t.get(ic)?]));
                    let tri = [wa, wb, wc];
                    let Some(frame) = TriFrame::new(&tri) else {
                        continue;
                    };
                    let texels_per_voxel = match (material.texture, tri_uv) {
                        (Some(tex), Some(uv)) => texel_density(tex.size(), &uv, tri),
                        _ => 1.0,
                    };
                    // The DDA picks the cells; its weights drift on diagonal
                    // lines, so each cell is shaded from its own footprint.
                    let mut cells: DdaVoxelizer<()> = DdaVoxelizer::new();
                    cells.add_triangle(&tri, &|_: Option<&()>, _: [i32; 3], _: [f32; 3]| {});
                    for cell in cells.finalize().into_keys() {
                        let s = if tri_uv.is_some() {
                            shade_footprint(
                                &material,
                                &frame,
                                tri_uv.as_ref(),
                                texels_per_voxel,
                                vcol,
                                cell,
                            )
                        } else {
                            shade_sample(&material, None, 1.0, vcol, [1.0 / 3.0; 3])
                        };
                        accum
                            .entry(cell)
                            .and_modify(|a| *a = a.add(&s))
                            .or_insert(s);
                    }
                }
            }
        }

        for child in node.children() {
            stack.push((child, world));
        }
    }

    Ok(accum)
}

/// Voxelizes a .glb model, leaving the block choice to the caller. Textures are
/// sampled per voxel, so alpha-cut lattices and decals stay open and keep their
/// own colours rather than one average per material.
pub fn voxelize_glb_paint(
    glb_bytes: &[u8],
    transform: WorldTransform,
) -> Result<Vec<([i32; 3], VoxelPaint)>, String> {
    let accum = accumulate_glb(glb_bytes, transform)?;
    let kept = kept_voxels(&accum);
    let mut out = Vec::with_capacity(kept.len());
    for pos in kept {
        let acc = &accum[&pos];
        let n = acc.samples.max(1) as f32;
        let paint = if acc.glass as f32 * 2.0 >= n {
            VoxelPaint::Glass
        } else if acc.weight <= 0.0 || acc.uncolored as f32 * 2.0 > n {
            VoxelPaint::Uncolored
        } else {
            let c = acc
                .rgb
                .map(|v| ((v / acc.weight).clamp(0.0, 1.0) * 255.0).round() as u8);
            VoxelPaint::Rgb((c[0], c[1], c[2]))
        };
        out.push((pos, paint));
    }
    Ok(out)
}

/// Base-level texels per world block across a triangle.
fn texel_density(size: (f32, f32), uv: &[[f32; 2]; 3], p: [[f32; 3]; 3]) -> f32 {
    let (du1, dv1) = (
        (uv[1][0] - uv[0][0]) * size.0,
        (uv[1][1] - uv[0][1]) * size.1,
    );
    let (du2, dv2) = (
        (uv[2][0] - uv[0][0]) * size.0,
        (uv[2][1] - uv[0][1]) * size.1,
    );
    let uv_area = (du1 * dv2 - du2 * dv1).abs();
    let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
    let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
    let cx = e1[1] * e2[2] - e1[2] * e2[1];
    let cy = e1[2] * e2[0] - e1[0] * e2[2];
    let cz = e1[0] * e2[1] - e1[1] * e2[0];
    let world_area = (cx * cx + cy * cy + cz * cz).sqrt();
    if world_area <= 1e-6 || !uv_area.is_finite() {
        return 1.0;
    }
    (uv_area / world_area).sqrt()
}

/// Share of a voxel's samples that count as solid, 0.5 at the material cutoff.
fn coverage_of(acc: &VoxelAccum) -> f32 {
    acc.coverage / acc.samples.max(1) as f32
}

/// Touched voxels that become blocks. Half covered is solid. A strip thinner
/// than a block that straddles two cells on one layer goes to the stronger
/// cell, so girders stay unbroken; partial voxels left with no neighbour are
/// dropped as speckle.
fn kept_voxels(accum: &HashMap<[i32; 3], VoxelAccum>) -> HashSet<[i32; 3]> {
    let cov = |p: &[i32; 3]| accum.get(p).map(coverage_of);
    let rescued = |p: [i32; 3], c: f32| {
        let mut best = 0.0f32;
        for (dx, dz) in LAYER_NEIGHBOURS {
            let q = [p[0] + dx, p[1], p[2] + dz];
            let Some(cq) = cov(&q).filter(|&cq| cq < 0.5) else {
                continue;
            };
            if cq > c || (cq == c && q < p) {
                return false;
            }
            best = best.max(cq);
        }
        c + best >= 0.5
    };
    let kept: HashSet<[i32; 3]> = accum
        .iter()
        .filter(|(&p, acc)| {
            let c = coverage_of(acc);
            c >= 0.5 || (c >= RESCUE_MIN && rescued(p, c))
        })
        .map(|(&p, _)| p)
        .collect();
    let lone = |p: &[i32; 3]| {
        cov(p).is_some_and(|c| c < 0.999)
            && !(-1..=1).any(|dx| {
                (-1..=1).any(|dy| {
                    (-1..=1).any(|dz| {
                        (dx, dy, dz) != (0, 0, 0)
                            && kept.contains(&[p[0] + dx, p[1] + dy, p[2] + dz])
                    })
                })
            })
    };
    let specks: Vec<[i32; 3]> = kept.iter().filter(|p| lone(p)).copied().collect();
    let mut kept = kept;
    for p in specks {
        kept.remove(&p);
    }
    kept
}

/// Partly covered voxels below this never come back as part of a strip.
const RESCUE_MIN: f32 = 0.25;
const LAYER_NEIGHBOURS: [(i32, i32); 8] = [
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (0, -1),
    (0, 1),
    (1, -1),
    (1, 0),
    (1, 1),
];

#[inline]
fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn unit3(a: [f32; 3]) -> Option<[f32; 3]> {
    let len = dot3(a, a).sqrt();
    (len > 1e-9 && len.is_finite()).then(|| a.map(|v| v / len))
}

/// A triangle's plane: an in-plane basis and exact barycentrics.
struct TriFrame {
    p0: [f32; 3],
    e1: [f32; 3],
    e2: [f32; 3],
    n: [f32; 3],
    u: [f32; 3],
    v: [f32; 3],
    d00: f32,
    d01: f32,
    d11: f32,
    inv_den: f32,
}

impl TriFrame {
    fn new(t: &[[f32; 3]; 3]) -> Option<Self> {
        let e1 = sub3(t[1], t[0]);
        let e2 = sub3(t[2], t[0]);
        let n = unit3(cross3(e1, e2))?;
        let u = unit3(e1)?;
        let v = cross3(n, u);
        let (d00, d01, d11) = (dot3(e1, e1), dot3(e1, e2), dot3(e2, e2));
        let den = d00 * d11 - d01 * d01;
        (den.abs() > 1e-12).then(|| Self {
            p0: t[0],
            e1,
            e2,
            n,
            u,
            v,
            d00,
            d01,
            d11,
            inv_den: 1.0 / den,
        })
    }

    fn barycentric(&self, p: [f32; 3]) -> [f32; 3] {
        let d = sub3(p, self.p0);
        let (d20, d21) = (dot3(d, self.e1), dot3(d, self.e2));
        let b1 = (self.d11 * d20 - self.d01 * d21) * self.inv_den;
        let b2 = (self.d00 * d21 - self.d01 * d20) * self.inv_den;
        [1.0 - b1 - b2, b1, b2]
    }
}

/// Shades the part of a triangle inside one voxel from a grid of samples, so a
/// thin opaque strip counts by the area it covers rather than by luck of one
/// sample point.
fn shade_footprint(
    mat: &Material,
    frame: &TriFrame,
    uv: Option<&[[f32; 2]; 3]>,
    texels_per_voxel: f32,
    vcol: Option<[f32; 4]>,
    cell: [i32; 3],
) -> VoxelAccum {
    let c = cell.map(|v| v as f32);
    let off = dot3(sub3(c, frame.p0), frame.n);
    let q = [0, 1, 2].map(|i| c[i] - frame.n[i] * off);
    let step = FOOTPRINT_SPAN / FOOTPRINT_STEPS as f32;
    let texels = texels_per_voxel * step;
    let mut acc: Option<VoxelAccum> = None;
    for i in 0..FOOTPRINT_STEPS {
        let s = (i as f32 + 0.5) * step - FOOTPRINT_SPAN / 2.0;
        for j in 0..FOOTPRINT_STEPS {
            let t = (j as f32 + 0.5) * step - FOOTPRINT_SPAN / 2.0;
            let p = [0, 1, 2].map(|k| q[k] + frame.u[k] * s + frame.v[k] * t);
            if (0..3).any(|k| (p[k] - c[k]).abs() > 0.5) {
                continue;
            }
            let w = frame.barycentric(p);
            if w.iter().any(|&x| x < -1e-4) {
                continue;
            }
            let sample = shade_sample(mat, uv, texels, vcol, w);
            acc = Some(acc.map_or(sample, |a| a.add(&sample)));
        }
    }
    // A sliver between the samples still gets one, at its nearest point.
    acc.unwrap_or_else(|| {
        let w = frame.barycentric(q).map(|x| x.max(0.0));
        let sum = (w[0] + w[1] + w[2]).max(1e-6);
        shade_sample(mat, uv, texels_per_voxel, vcol, w.map(|x| x / sum))
    })
}

/// One sample at barycentric weights `w`, colour in sRGB.
fn shade_sample(
    mat: &Material,
    uv: Option<&[[f32; 2]; 3]>,
    texels_per_voxel: f32,
    vcol: Option<[f32; 4]>,
    w: [f32; 3],
) -> VoxelAccum {
    let mut lin = [mat.factor[0], mat.factor[1], mat.factor[2]];
    let mut alpha = mat.factor[3];
    if let (Some(tex), Some(uv)) = (mat.texture, uv) {
        let p = [0, 1].map(|i| uv[0][i] * w[0] + uv[1][i] * w[1] + uv[2][i] * w[2]);
        let t = tex.sample(p, texels_per_voxel);
        if t[3] > 1e-4 {
            for i in 0..3 {
                lin[i] *= t[i] / t[3];
            }
        }
        alpha *= t[3];
    }
    let mut rgb = lin.map(linear_to_srgb);
    if let Some(vc) = vcol {
        for i in 0..3 {
            rgb[i] *= vc[i];
        }
        alpha *= vc[3];
    }
    let coverage = match mat.cutoff {
        None => 1.0,
        Some(cut) => (alpha / (2.0 * cut.max(1e-3))).min(1.0),
    };
    let uncolored = mat.uncolored && vcol.is_none();
    let weight = if uncolored { 0.0 } else { coverage };
    VoxelAccum {
        rgb: rgb.map(|c| c * weight),
        weight,
        coverage,
        samples: 1,
        glass: is_glass_sentinel(rgb) as u16,
        uncolored: uncolored as u16,
    }
}

/// Voxelizes triangles in one block. A closed mesh comes out solid: a surface
/// shell only one voxel thick shows daylight through every diagonal step.
pub fn voxelize_solid_triangles(
    triangles: &[[[f32; 3]; 3]],
    transform: WorldTransform,
    block: Block,
) -> Vec<([i32; 3], Block)> {
    let world: Vec<[[f32; 3]; 3]> = triangles
        .iter()
        .map(|t| t.map(|v| transform.apply(v)))
        .filter(|t| triangle_finite(&t[0], &t[1], &t[2]))
        .collect();
    let mut voxelizer: DdaVoxelizer<()> = DdaVoxelizer::new();
    let shader = |_: Option<&()>, _: [i32; 3], _: [f32; 3]| {};
    for t in &world {
        voxelizer.add_triangle(t, &shader);
    }
    let mut cells: std::collections::HashSet<[i32; 3]> = voxelizer.finalize().into_keys().collect();
    for ([x, z], hits) in column_crossings(&world) {
        // An odd count means the mesh leaks here; leave that column as a shell.
        if hits.len() % 2 != 0 {
            continue;
        }
        for pair in hits.chunks_exact(2) {
            for y in pair[0].ceil() as i32..=pair[1].floor() as i32 {
                cells.insert([x, y, z]);
            }
        }
    }
    cells.into_iter().map(|pos| (pos, block)).collect()
}

/// Where a vertical ray through each voxel column (voxel centres sit on integers,
/// as in dda-voxelize) crosses the mesh, sorted bottom up.
fn column_crossings(tris: &[[[f32; 3]; 3]]) -> std::collections::HashMap<[i32; 2], Vec<f32>> {
    // Off-centre, so a ray never runs exactly along a shared edge and counts it twice.
    const JX: f32 = 0.0123;
    const JZ: f32 = 0.0371;
    let mut out: std::collections::HashMap<[i32; 2], Vec<f32>> = std::collections::HashMap::new();
    for t in tris {
        let (ax, az, bx, bz, cx, cz) = (t[0][0], t[0][2], t[1][0], t[1][2], t[2][0], t[2][2]);
        let den = (bz - cz) * (ax - cx) + (cx - bx) * (az - cz);
        if den.abs() < 1e-9 {
            continue;
        }
        let x0 = (ax.min(bx).min(cx) - JX).ceil() as i32;
        let x1 = (ax.max(bx).max(cx) - JX).floor() as i32;
        let z0 = (az.min(bz).min(cz) - JZ).ceil() as i32;
        let z1 = (az.max(bz).max(cz) - JZ).floor() as i32;
        for x in x0..=x1 {
            for z in z0..=z1 {
                let (px, pz) = (x as f32 + JX, z as f32 + JZ);
                let w0 = ((bz - cz) * (px - cx) + (cx - bx) * (pz - cz)) / den;
                let w1 = ((cz - az) * (px - cx) + (ax - cx) * (pz - cz)) / den;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let y = w0 * t[0][1] + w1 * t[1][1] + w2 * t[2][1];
                out.entry([x, z]).or_default().push(y);
            }
        }
    }
    for hits in out.values_mut() {
        hits.sort_by(f32::total_cmp);
    }
    out
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

    fn partial(c: f32) -> VoxelAccum {
        VoxelAccum {
            coverage: c,
            samples: 1,
            ..Default::default()
        }
    }

    #[test]
    fn strip_split_between_cells_stays_unbroken() {
        // A vertical strip under a block wide, halved between x 0 and 1 on odd layers.
        let mut accum = HashMap::new();
        for y in 0..6 {
            if y % 2 == 0 {
                accum.insert([0, y, 0], partial(0.8));
            } else {
                accum.insert([0, y, 0], partial(0.4));
                accum.insert([1, y, 0], partial(0.4));
            }
            accum.insert([4, y, 0], partial(0.1));
        }
        let kept = kept_voxels(&accum);
        for y in 0..6 {
            let n = [[0, y, 0], [1, y, 0]]
                .iter()
                .filter(|p| kept.contains(*p))
                .count();
            assert_eq!(n, 1, "layer {y} should keep exactly one strip cell");
            assert!(!kept.contains(&[4, y, 0]));
        }
    }

    #[test]
    fn lone_partial_voxel_is_dropped_but_lone_solid_kept() {
        let mut accum = HashMap::new();
        accum.insert([0, 0, 0], partial(0.6));
        accum.insert([10, 0, 0], partial(1.0));
        let kept = kept_voxels(&accum);
        assert!(!kept.contains(&[0, 0, 0]));
        assert!(kept.contains(&[10, 0, 0]));
    }

    #[test]
    fn footprint_measures_area_inside_voxel() {
        // A plane at y 0 covering x >= -0.25: three quarters of voxel (0,0,0) are inside.
        let tri = [[-0.25, 0.0, -5.0], [-0.25, 0.0, 5.0], [10.0, 0.0, 0.0]];
        let frame = TriFrame::new(&tri).unwrap();
        let mat = Material {
            factor: [0.5, 0.5, 0.5, 1.0],
            texture: None,
            tex_coord: 0,
            cutoff: None,
            uncolored: false,
        };
        let acc = shade_footprint(&mat, &frame, None, 1.0, None, [0, 0, 0]);
        assert_eq!(
            acc.samples,
            3 * 4,
            "a 4x4 grid spans the cube, 3 of its 4 columns inside"
        );
        let w = frame.barycentric([3.0, 0.0, 1.0]);
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(w.iter().all(|&x| x >= 0.0));
    }
}
