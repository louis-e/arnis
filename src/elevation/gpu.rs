//! GPU-accelerated compute operations via wgpu.
//!
//! Requires the `gpu` Cargo feature.  When the feature is absent or no
//! suitable GPU adapter is found, all functions return [`None`] so
//! callers fall back to the existing CPU (rayon) path transparently.
//!
//! ## Supported backends
//!
//! | OS       | Backend            |
//! |----------|--------------------|
//! | Windows  | DX12 / Vulkan      |
//! | Linux    | Vulkan             |
//! | macOS    | Metal              |

use std::sync::OnceLock;
use wgpu::util::DeviceExt;

// ---------------------------------------------------------------------------
// lazy GPU context
// ---------------------------------------------------------------------------

/// Initialised GPU context, or [`None`] if no suitable device was found
/// (missing adapter, incompatible backend, WebGPU headless, …).
static GPU: OnceLock<Option<GpuCtx>> = OnceLock::new();

struct GpuCtx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    blur_pipeline_h: wgpu::ComputePipeline,
    blur_pipeline_v: wgpu::ComputePipeline,
    blur_bind_group_layout: wgpu::BindGroupLayout,
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
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
            .ok()?;

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("gaussian_blur"),
        source: wgpu::ShaderSource::Wgsl(include_str!("gpu_blur.wgsl").into()),
    });

    // Params uniform (width, height, radius)
    let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("blur_params"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
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
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("blur_pipeline_layout"),
        bind_group_layouts: &[&params_layout],
        push_constant_ranges: &[],
    });

    let blur_pipeline_h = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("blur_h"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: "blur_h",
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });

    let blur_pipeline_v = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("blur_v"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: "blur_v",
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });

    Some(GpuCtx {
        device,
        queue,
        blur_pipeline_h,
        blur_pipeline_v,
        blur_bind_group_layout: params_layout,
    })
}

fn ctx() -> Option<&'static GpuCtx> {
    GPU.get_or_init(init_gpu).as_ref()
}

// ---------------------------------------------------------------------------
// public API
// ---------------------------------------------------------------------------

/// Whether a GPU device is available for compute.
#[inline]
#[allow(dead_code)]
pub fn gpu_available() -> bool {
    ctx().is_some()
}

/// Run a 2D separable Gaussian blur on the GPU.
///
/// Returns [`None`] if the GPU feature is disabled or no device is
/// available, so callers should fall back to the CPU path.
pub fn gpu_gaussian_blur_2d(grid: &[Vec<f64>], sigma: f64) -> Option<Vec<Vec<f64>>> {
    let ctx = ctx()?;
    let h = grid.len();
    if h == 0 {
        return Some(Vec::new());
    }
    let w = grid[0].len();
    if w == 0 {
        return Some(vec![Vec::new(); h]);
    }

    // Precompute Gaussian kernel (same formula as CPU path)
    let kernel_size = (sigma * 3.0).ceil() as usize * 2 + 1;
    let kernel = create_gaussian_kernel_f32(kernel_size, sigma);
    let radius = (kernel_size / 2) as u32;

    // Flatten grid to f32
    let total = w * h;
    let mut flat: Vec<f32> = Vec::with_capacity(total);
    for row in grid {
        for &v in row {
            flat.push(v as f32);
        }
    }

    // GPU buffers
    let src_buf = create_storage_buffer(&ctx.device, &flat, "src");
    let dst_buf = create_storage_buffer_f32(&ctx.device, total, "dst");
    let weights_buf = create_storage_buffer(&ctx.device, &kernel, "weights");

    // Params uniform: [width, height, radius, 0 (pad)]
    let params_data: [u32; 4] = [w as u32, h as u32, radius, 0];
    let params_buf = create_uniform_buffer(&ctx.device, cast_u32_slice(&params_data));

    // Dispatch a single pass (horizontal or vertical)
    let dispatch_pass = |pipeline: &wgpu::ComputePipeline| {
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blur"),
            layout: &ctx.blur_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: src_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: dst_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: weights_buf.as_entire_binding(),
                },
            ],
        });
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("blur_encoder"),
            });
        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("blur_pass"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(pipeline);
            cpass.set_bind_group(0, &bind_group, &[]);
            // Workgroup size 256 — each thread handles one pixel.
            // The shader checks bounds for grid edges.
            let wg_count_x = w.div_ceil(256);
            cpass.dispatch_workgroups(wg_count_x as u32, h as u32, 1);
        }
        ctx.queue.submit(Some(encoder.finish()));
    };

    // ── horizontal pass: src → dst ──
    dispatch_pass(&ctx.blur_pipeline_h);

    // ── vertical pass: src (now contains H result) → dst ──
    // Copy dst (horizontal result) back to src
    copy_buffer(&ctx.device, &ctx.queue, &dst_buf, &src_buf, total);
    dispatch_pass(&ctx.blur_pipeline_v);

    // Read back result
    let result_flat = read_storage_buffer_f32(&ctx.device, &ctx.queue, &dst_buf, total);

    // Reshape to Vec<Vec<f64>>
    let mut out: Vec<Vec<f64>> = Vec::with_capacity(h);
    for row_start in (0..total).step_by(w) {
        out.push(
            result_flat[row_start..row_start + w]
                .iter()
                .map(|&v| v as f64)
                .collect(),
        );
    }

    Some(out)
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn create_gaussian_kernel_f32(size: usize, sigma: f64) -> Vec<f32> {
    let half = size as isize / 2;
    let two_sigma_sq = 2.0 * sigma * sigma;
    let weights: Vec<f32> = (0..size)
        .map(|i| {
            let x = i as f64 - half as f64;
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

fn create_storage_buffer(device: &wgpu::Device, data: &[f32], label: &str) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: cast_f32_slice(data),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
    })
}

fn create_storage_buffer_f32(device: &wgpu::Device, count: usize, label: &str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (count * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

fn create_uniform_buffer(device: &wgpu::Device, data: &[u8]) -> wgpu::Buffer {
    // wgpu uniform buffer size must be a multiple of 16.
    let size = data.len().next_multiple_of(16) as u64;
    let mut padded = data.to_vec();
    padded.resize(size as usize, 0);
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("params"),
        contents: &padded,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}

/// Copy contents of `src` buffer into `dst` buffer (both f32, same length).
fn copy_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    src: &wgpu::Buffer,
    dst: &wgpu::Buffer,
    count: usize,
) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("copy"),
    });
    encoder.copy_buffer_to_buffer(src, 0, dst, 0, (count * 4) as u64);
    queue.submit(Some(encoder.finish()));
}

/// Read an f32 storage buffer back to CPU.
fn read_storage_buffer_f32(
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
