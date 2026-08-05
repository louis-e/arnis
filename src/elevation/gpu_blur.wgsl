// Separable Gaussian blur — two 1D compute passes (horizontal then vertical).
//
// Semantics mirror the CPU `gaussian_blur_grid_reported` exactly:
// out-of-bounds and non-finite (NaN/inf) samples are dropped from the
// convolution and the kernel weights are renormalised over the valid
// samples. A cell whose entire window is invalid becomes NaN.

struct Params {
    width: u32,
    height: u32,
    radius: u32,
    kernel_size: u32,
}

@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var<storage, read> weights: array<f32>;

// Bit-tested finiteness: exponent bits all set means NaN or inf. (Robust
// against any compiler fast-math assumptions around `v == v`.)
fn is_finite_f(v: f32) -> bool {
    return (bitcast<u32>(v) & 0x7f800000u) != 0x7f800000u;
}

fn nan_f() -> f32 {
    return bitcast<f32>(0x7fc00000u);
}

@compute @workgroup_size(256, 1, 1)
fn blur_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if x >= params.width || y >= params.height {
        return;
    }
    let row_start = y * params.width;
    let ix = i32(x);
    let w_max = i32(params.width);

    var sum = 0.0;
    var wsum = 0.0;
    for (var k = 0u; k < params.kernel_size; k++) {
        let sx = ix + i32(k) - i32(params.radius);
        if sx < 0 || sx >= w_max {
            continue;
        }
        let v = src[row_start + u32(sx)];
        if is_finite_f(v) {
            let wgt = weights[k];
            sum += v * wgt;
            wsum += wgt;
        }
    }

    let idx = row_start + x;
    if wsum > 0.0 {
        dst[idx] = sum / wsum;
    } else {
        dst[idx] = nan_f();
    }
}

@compute @workgroup_size(256, 1, 1)
fn blur_v(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if x >= params.width || y >= params.height {
        return;
    }
    let iy = i32(y);
    let h_max = i32(params.height);

    var sum = 0.0;
    var wsum = 0.0;
    for (var k = 0u; k < params.kernel_size; k++) {
        let sy = iy + i32(k) - i32(params.radius);
        if sy < 0 || sy >= h_max {
            continue;
        }
        let v = src[u32(sy) * params.width + x];
        if is_finite_f(v) {
            let wgt = weights[k];
            sum += v * wgt;
            wsum += wgt;
        }
    }

    let idx = y * params.width + x;
    if wsum > 0.0 {
        dst[idx] = sum / wsum;
    } else {
        dst[idx] = nan_f();
    }
}
