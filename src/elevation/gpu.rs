//! GPU-accelerated elevation post-processing via wgpu.
//!
//! Requires the `gpu` Cargo feature and `ARNIS_GPU=1`. When the feature is
//! absent, no suitable GPU adapter is found, or a grid exceeds what the
//! device can bind, every entry point reports failure so callers fall back
//! to the CPU (rayon) path transparently.
//!
//! ## Supported backends
//!
//! | OS       | Backend            |
//! |----------|--------------------|
//! | Windows  | DX12 / Vulkan      |
//! | Linux    | Vulkan             |
//! | macOS    | Metal              |
//!
//! ## Design notes
//!
//! - **One shared bind-group layout** (`src`, `dst`, `params`, `aux`) covers
//!   all passes, so blur / anomaly-repair / NaN-fill can ping-pong between
//!   two resident buffers without re-creating layouts.
//! - **Resident ping-pong**: anomaly repair (up to 10 passes) and NaN fill
//!   run back-to-back on the GPU with a single upload and a single download,
//!   avoiding the per-call 700+ MB round trips that made the first GPU
//!   attempt slower than CPU.
//! - **Device limits are requested from the adapter**: the default
//!   `max_storage_buffer_binding_size` (128 MB) rejects city-sized grids
//!   (a 16384² f32 grid is 1 GB). We ask for what the adapter supports and
//!   fall back to CPU when the grid still doesn't fit.
//! - **f32 on GPU, f64 on CPU**: WGSL has no f64. Elevation in metres is
//!   well within f32 precision (< 1 mm error at 8848 m). Conversions to and
//!   from the f64 `Vec<Vec<f64>>` world are parallelised with rayon.
//! - Shader semantics mirror the CPU implementations exactly (NaN handling,
//!   edge renormalisation, upper-median selection) so GPU/CPU output differs
//!   only by float rounding.

use rayon::prelude::*;
use std::sync::OnceLock;
use wgpu::util::DeviceExt;

/// Matches `repair_terrain_anomalies` (postprocess.rs): max passes with an
/// early break once a pass repairs nothing.
const MAX_ANOMALY_PASSES: u32 = 10;
/// Safety cap for the Jacobi-style NaN dilation. Each pass extends filled
/// cells by one cell, so this bounds the largest fillable hole radius. If the
/// cap is hit the partially filled grid is downloaded and the CPU loop
/// finishes the remainder.
const MAX_NAN_PASSES: u32 = 4096;

// ---------------------------------------------------------------------------
// lazy GPU context
// ---------------------------------------------------------------------------

/// Initialised GPU context, or [`None`] if no suitable device was found
/// (missing adapter, incompatible backend, WebGPU headless, …).
static GPU: OnceLock<Option<GpuCtx>> = OnceLock::new();

struct GpuCtx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Granted `max_storage_buffer_binding_size`; grids whose byte size
    /// exceeds this must fall back to CPU.
    max_storage_binding: u64,
    /// Layout for the blur passes (aux binding 3 = read-only kernel weights).
    blur_layout: wgpu::BindGroupLayout,
    /// Layout for the iterative passes (aux binding 3 = read_write atomic
    /// change counter).
    iter_layout: wgpu::BindGroupLayout,
    blur_h: wgpu::ComputePipeline,
    blur_v: wgpu::ComputePipeline,
    anomaly: wgpu::ComputePipeline,
    nan_fill: wgpu::ComputePipeline,
}

fn init_gpu() -> Option<GpuCtx> {
    // Guard against driver bugs / incompatible adapters that may panic.
    std::panic::catch_unwind(try_init_gpu).ok().flatten()
}

fn try_init_gpu() -> Option<GpuCtx> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;

    // Request the adapter's real buffer limits. The wgpu defaults cap storage
    // bindings at 128 MB, which rejects every grid above ~33M cells — exactly
    // the large grids where GPU pays off.
    let supported = adapter.limits();
    let required_limits = wgpu::Limits {
        max_storage_buffer_binding_size: supported.max_storage_buffer_binding_size,
        max_buffer_size: supported.max_buffer_size,
        ..wgpu::Limits::default()
    };
    let max_storage_binding = required_limits.max_storage_buffer_binding_size as u64;
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            required_limits,
            ..wgpu::DeviceDescriptor::default()
        },
        None,
    ))
    .ok()?;

    // Shared bindings for every pass:
    //   0 = src (read-only storage)
    //   1 = dst (read_write storage)
    //   2 = params (uniform, 16 bytes: width, height, radius, kernel_size)
    //   3 = aux  — blur reads it as `weights` (read-only), the iterative
    //              passes use it as an atomic change counter (read_write).
    // wgpu validates shader access against the layout exactly, so the two
    // aux access modes need two layouts.
    let common_entries = |aux_read_only: bool| {
        [
            storage_entry(0, true),
            storage_entry(1, false),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            storage_entry(3, aux_read_only),
        ]
    };
    let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("gpu_blur_layout"),
        entries: &common_entries(true),
    });
    let iter_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("gpu_iter_layout"),
        entries: &common_entries(false),
    });
    let blur_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("gpu_blur_pipeline_layout"),
        bind_group_layouts: &[&blur_layout],
        push_constant_ranges: &[],
    });
    let iter_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("gpu_iter_pipeline_layout"),
        bind_group_layouts: &[&iter_layout],
        push_constant_ranges: &[],
    });

    let make_pipeline = |source: &str, label: &str, entry: &str, layout: &wgpu::PipelineLayout| {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            module: &shader,
            entry_point: entry,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        })
    };

    let blur_shader = include_str!("gpu_blur.wgsl");
    Some(GpuCtx {
        blur_h: make_pipeline(blur_shader, "blur_h", "blur_h", &blur_pipeline_layout),
        blur_v: make_pipeline(blur_shader, "blur_v", "blur_v", &blur_pipeline_layout),
        anomaly: make_pipeline(
            include_str!("gpu_anomaly.wgsl"),
            "anomaly",
            "main",
            &iter_pipeline_layout,
        ),
        nan_fill: make_pipeline(
            include_str!("gpu_nan.wgsl"),
            "nan_fill",
            "main",
            &iter_pipeline_layout,
        ),
        device,
        queue,
        max_storage_binding,
        blur_layout,
        iter_layout,
    })
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn ctx() -> Option<&'static GpuCtx> {
    GPU.get_or_init(init_gpu).as_ref()
}

// ---------------------------------------------------------------------------
// public API
// ---------------------------------------------------------------------------

/// Whether a GPU device is available for compute.
#[inline]
#[allow(dead_code)] // diagnostic probe; kept for GUI/CLI status reporting
pub fn gpu_available() -> bool {
    ctx().is_some()
}

/// Kick off adapter/device init on a background thread so its cost (several
/// hundred ms) overlaps with data fetching instead of landing inside the
/// first timed GPU pass. Safe to call multiple times; init happens once.
pub fn init_in_background() {
    std::thread::spawn(|| {
        let _ = ctx();
    });
}

/// Run a 2D separable Gaussian blur on the GPU, matching the CPU
/// `gaussian_blur_grid` semantics (out-of-bounds and non-finite samples are
/// dropped and the kernel weights renormalised).
///
/// Returns [`None`] if no device is available or the grid exceeds the
/// device's storage binding limit, so callers fall back to the CPU path.
pub fn gpu_gaussian_blur_2d(grid: &[Vec<f64>], sigma: f64) -> Option<Vec<Vec<f64>>> {
    let ctx = ctx()?;
    let (w, h) = grid_dims(grid)?;
    let total = checked_grid_cells(ctx, w, h)?;

    // Same kernel formula as the CPU path, downcast to f32.
    let kernel_size = (sigma * 3.0).ceil() as usize * 2 + 1;
    let kernel = create_gaussian_kernel_f32(kernel_size, sigma);
    let params = [
        w as u32,
        h as u32,
        (kernel_size / 2) as u32,
        kernel_size as u32,
    ];

    let flat = flatten_grid(grid);
    let buf_a = create_storage_buffer_init(&ctx.device, &flat, "blur_a");
    let buf_b = create_storage_buffer(&ctx.device, total, "blur_b");
    let weights = create_storage_buffer_init(&ctx.device, &kernel, "weights");
    let params_buf = create_uniform_buffer(&ctx.device, &params);

    run_pass(
        ctx,
        &ctx.blur_h,
        &ctx.blur_layout,
        &buf_a,
        &buf_b,
        &params_buf,
        &weights,
        w,
        h,
    );
    run_pass(
        ctx,
        &ctx.blur_v,
        &ctx.blur_layout,
        &buf_b,
        &buf_a,
        &params_buf,
        &weights,
        w,
        h,
    );

    let flat_out = read_buffer_f32(&ctx.device, &ctx.queue, &buf_a, total);
    Some(reshape_grid(&flat_out, w, h))
}

/// GPU-resident `repair_terrain_anomalies` + `fill_nan_values`.
///
/// Uploads the grid once, runs up to [`MAX_ANOMALY_PASSES`] 5×5 median/MAD
/// passes followed by Jacobi NaN-dilation passes, then downloads the result
/// back into `grid`. Both passes use a small atomic counter to reproduce the
/// CPU early-break semantics.
///
/// Returns `false` if no device is available or the grid doesn't fit, in
/// which case the caller runs the CPU implementations instead.
pub fn gpu_anomaly_repair_and_nan_fill(grid: &mut Vec<Vec<f64>>) -> bool {
    let Some(ctx) = ctx() else { return false };
    let Some((w, h)) = grid_dims(grid) else {
        return true; // empty grid: nothing to do, nothing to fall back to
    };
    // CPU path no-ops below 5x5; keep behaviour identical.
    if w < 5 || h < 5 {
        return true;
    }
    let Some(total) = checked_grid_cells(ctx, w, h) else {
        return false;
    };

    let flat = flatten_grid(grid);
    let mut buf_a = create_storage_buffer_init(&ctx.device, &flat, "core_a");
    let mut buf_b = create_storage_buffer(&ctx.device, total, "core_b");
    let params = [w as u32, h as u32, 0, 0];
    let params_buf = create_uniform_buffer(&ctx.device, &params);
    // aux slot: single atomic u32 change counter (16 B for alignment).
    let counter = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("counter"),
        size: 16,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_SRC
            | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let run_iterative_passes = |pipeline: &wgpu::ComputePipeline,
                                max_passes: u32,
                                buf_a: &mut wgpu::Buffer,
                                buf_b: &mut wgpu::Buffer|
     -> PassOutcome {
        for _pass in 0..max_passes {
            ctx.queue.write_buffer(&counter, 0, &0u32.to_le_bytes());
            run_pass(
                ctx,
                pipeline,
                &ctx.iter_layout,
                buf_a,
                buf_b,
                &params_buf,
                &counter,
                w,
                h,
            );
            let changed = read_counter(ctx, &counter);
            std::mem::swap(buf_a, buf_b);
            if changed == 0 {
                return PassOutcome::Converged;
            }
        }
        PassOutcome::HitCap
    };

    // Phase 1: terrain anomaly repair (5x5 median/MAD, early break).
    run_iterative_passes(&ctx.anomaly, MAX_ANOMALY_PASSES, &mut buf_a, &mut buf_b);

    // Phase 2: NaN fill (Jacobi neighbour averaging until stable).
    let nan_outcome = run_iterative_passes(&ctx.nan_fill, MAX_NAN_PASSES, &mut buf_a, &mut buf_b);

    // Download back into the caller's grid.
    let flat_out = read_buffer_f32(&ctx.device, &ctx.queue, &buf_a, total);
    grid.par_iter_mut()
        .zip(flat_out.par_chunks(w))
        .for_each(|(row, flat_row)| {
            for (cell, &v) in row.iter_mut().zip(flat_row.iter()) {
                *cell = v as f64;
            }
        });

    if nan_outcome == PassOutcome::HitCap {
        eprintln!(
            "[GPU] NaN fill did not converge within {MAX_NAN_PASSES} passes; finishing on CPU"
        );
        crate::elevation::postprocess::fill_nan_values(grid);
    }
    true
}

#[derive(PartialEq)]
enum PassOutcome {
    Converged,
    HitCap,
}

// ---------------------------------------------------------------------------
// pass plumbing
// ---------------------------------------------------------------------------

fn grid_dims(grid: &[Vec<f64>]) -> Option<(usize, usize)> {
    let h = grid.len();
    if h == 0 {
        return None;
    }
    let w = grid[0].len();
    if w == 0 {
        return None;
    }
    Some((w, h))
}

/// Total cell count if the grid fits into one storage binding on this device.
fn checked_grid_cells(ctx: &GpuCtx, w: usize, h: usize) -> Option<usize> {
    let total = w.checked_mul(h)?;
    let bytes = total.checked_mul(4)? as u64;
    if bytes > ctx.max_storage_binding {
        eprintln!(
            "[GPU] grid {}x{} ({} MB) exceeds device storage binding limit ({} MB); using CPU",
            w,
            h,
            bytes / (1024 * 1024),
            ctx.max_storage_binding / (1024 * 1024),
        );
        return None;
    }
    Some(total)
}

#[allow(clippy::too_many_arguments)]
fn run_pass(
    ctx: &GpuCtx,
    pipeline: &wgpu::ComputePipeline,
    layout: &wgpu::BindGroupLayout,
    src: &wgpu::Buffer,
    dst: &wgpu::Buffer,
    params: &wgpu::Buffer,
    aux: &wgpu::Buffer,
    w: usize,
    h: usize,
) {
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("gpu_pass"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: src.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: dst.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: aux.as_entire_binding(),
            },
        ],
    });
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gpu_pass"),
        });
    {
        let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("gpu_pass"),
            timestamp_writes: None,
        });
        cpass.set_pipeline(pipeline);
        cpass.set_bind_group(0, &bind_group, &[]);
        // Workgroup size 256 along x; one thread per cell, bounds-checked in
        // the shader. h <= 16384 stays far below the 65535 y-dispatch limit.
        cpass.dispatch_workgroups(w.div_ceil(256) as u32, h as u32, 1);
    }
    ctx.queue.submit(Some(encoder.finish()));
}

/// Copy the atomic counter out and return its value. Synchronises the queue,
/// so it also acts as a pass fence.
fn read_counter(ctx: &GpuCtx, counter: &wgpu::Buffer) -> u32 {
    let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("counter_staging"),
        size: 16,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("counter_copy"),
        });
    encoder.copy_buffer_to_buffer(counter, 0, &staging, 0, 4);
    ctx.queue.submit(Some(encoder.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).ok();
    });
    ctx.device.poll(wgpu::Maintain::Wait);
    let _ = rx.recv().ok();
    let view = slice.get_mapped_range();
    let value = u32::from_le_bytes([view[0], view[1], view[2], view[3]]);
    drop(view);
    staging.unmap();
    value
}

// ---------------------------------------------------------------------------
// host-side data movement (all parallelised; the f64<->f32 conversion and
// Vec<Vec> <-> flat reshapes were the hidden cost of the first GPU attempt)
// ---------------------------------------------------------------------------

fn flatten_grid(grid: &[Vec<f64>]) -> Vec<f32> {
    let w = grid[0].len();
    let mut flat: Vec<f32> = vec![0.0; grid.len() * w];
    flat.par_chunks_mut(w)
        .zip(grid.par_iter())
        .for_each(|(dst, src)| {
            for (d, &v) in dst.iter_mut().zip(src.iter()) {
                *d = v as f32;
            }
        });
    flat
}

fn reshape_grid(flat: &[f32], w: usize, h: usize) -> Vec<Vec<f64>> {
    flat.par_chunks(w)
        .take(h)
        .map(|row| row.iter().map(|&v| v as f64).collect())
        .collect()
}

fn create_gaussian_kernel_f32(size: usize, sigma: f64) -> Vec<f32> {
    // Center at size/2.0 — NOT (size-1)/2 — to match the CPU
    // `create_gaussian_kernel` weights bit-for-bit (CPU centres the kernel
    // between the two middle taps; the convolution offset uses size/2).
    let center = size as f64 / 2.0;
    let two_sigma_sq = 2.0 * sigma * sigma;
    let weights: Vec<f32> = (0..size)
        .map(|i| {
            let x = i as f64 - center;
            ((-(x * x) / two_sigma_sq).exp()) as f32
        })
        .collect();
    let sum: f32 = weights.iter().sum();
    weights.into_iter().map(|w| w / sum).collect()
}

/// Reinterpret a `&[f32]` as `&[u8]` (same length in bytes).
fn cast_f32_slice(s: &[f32]) -> &[u8] {
    // SAFETY: f32 has no invalid bit patterns.
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}

/// Reinterpret a `&[u32]` as `&[u8]`.
fn cast_u32_slice(s: &[u32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}

/// Reinterpret a `&[u8]` as `&[f32]`.
fn cast_slice_to_f32(s: &[u8]) -> &[f32] {
    assert!(s.len().is_multiple_of(std::mem::size_of::<f32>()));
    unsafe {
        std::slice::from_raw_parts(
            s.as_ptr() as *const f32,
            s.len() / std::mem::size_of::<f32>(),
        )
    }
}

fn create_storage_buffer_init(device: &wgpu::Device, data: &[f32], label: &str) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: cast_f32_slice(data),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
    })
}

fn create_storage_buffer(device: &wgpu::Device, count: usize, label: &str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (count * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

fn create_uniform_buffer(device: &wgpu::Device, params: &[u32; 4]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("params"),
        contents: cast_u32_slice(params),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}

/// Read an f32 storage buffer back to CPU.
fn read_buffer_f32(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buf: &wgpu::Buffer,
    count: usize,
) -> Vec<f32> {
    let size = (count * 4) as u64;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("readback"),
    });
    encoder.copy_buffer_to_buffer(buf, 0, &staging, 0, size);
    queue.submit(Some(encoder.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).ok();
    });
    device.poll(wgpu::Maintain::Wait);
    let _ = rx.recv().ok();
    let view = slice.get_mapped_range();
    let result: Vec<f32> = cast_slice_to_f32(&view).to_vec();
    drop(view);
    staging.unmap();
    result
}

// ---------------------------------------------------------------------------
// tests (require a real GPU adapter; they skip cleanly on headless CI)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random grid with hills, a spike, and a NaN hole.
    fn sample_grid(w: usize, h: usize) -> Vec<Vec<f64>> {
        let mut grid = vec![vec![0.0f64; w]; h];
        for (y, row) in grid.iter_mut().enumerate() {
            for (x, cell) in row.iter_mut().enumerate() {
                let v = ((x * 31 + y * 17 + 13) % 97) as f64 / 3.0
                    + (x as f64 * 0.1).sin() * 5.0
                    + (y as f64 * 0.07).cos() * 5.0
                    + 100.0;
                *cell = v;
            }
        }
        grid
    }

    fn assert_grids_close(a: &[Vec<f64>], b: &[Vec<f64>], tol: f64) {
        assert_eq!(a.len(), b.len());
        for (row_a, row_b) in a.iter().zip(b.iter()) {
            assert_eq!(row_a.len(), row_b.len());
            for (&va, &vb) in row_a.iter().zip(row_b.iter()) {
                if va.is_nan() && vb.is_nan() {
                    continue;
                }
                assert!(
                    (va - vb).abs() <= tol,
                    "grid mismatch: {va} vs {vb} (tol {tol})"
                );
            }
        }
    }

    #[test]
    fn gpu_blur_matches_cpu() {
        let grid = sample_grid(97, 63);
        // NaN holes must be skipped + renormalised exactly like the CPU path.
        let mut with_nan = grid.clone();
        for row in with_nan.iter_mut().take(25).skip(20) {
            for cell in row.iter_mut().take(36).skip(30) {
                *cell = f64::NAN;
            }
        }
        let cpu = crate::elevation::postprocess::gaussian_blur_grid(&with_nan, 3.0);
        let Some(gpu) = gpu_gaussian_blur_2d(&with_nan, 3.0) else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        assert_grids_close(&cpu, &gpu, 0.05);
    }

    #[test]
    fn gpu_anomaly_and_nan_fill_matches_cpu() {
        let mut grid = sample_grid(64, 64);
        grid[30][30] += 500.0; // isolated spike -> anomaly repair
        grid[31][30] += 500.0;
        for row in grid.iter_mut().take(13).skip(10) {
            for cell in row.iter_mut().take(14).skip(10) {
                *cell = f64::NAN; // small NaN hole -> NaN fill
            }
        }
        let mut cpu_grid = grid.clone();
        crate::elevation::postprocess::repair_terrain_anomalies(&mut cpu_grid);
        crate::elevation::postprocess::fill_nan_values(&mut cpu_grid);

        let mut gpu_grid = grid;
        if !gpu_anomaly_repair_and_nan_fill(&mut gpu_grid) {
            eprintln!("no GPU adapter; skipping");
            return;
        }
        // Anomaly thresholds near ties can flip individual cells between f32
        // and f64, so allow a small fraction of mismatches at spike sites.
        let mut mismatches = 0usize;
        let mut total = 0usize;
        for (row_c, row_g) in cpu_grid.iter().zip(gpu_grid.iter()) {
            for (&vc, &vg) in row_c.iter().zip(row_g.iter()) {
                total += 1;
                let both_nan = vc.is_nan() && vg.is_nan();
                if !both_nan && (vc - vg).abs() > 0.5 {
                    mismatches += 1;
                }
            }
        }
        assert!(
            mismatches * 100 <= total,
            "too many CPU/GPU mismatches: {mismatches}/{total}"
        );
        // The NaN hole must be fully filled by both paths.
        for row in gpu_grid.iter().take(13).skip(10) {
            for &cell in row.iter().take(14).skip(10) {
                assert!(cell.is_finite());
            }
        }
    }
}
