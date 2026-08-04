// Separable Gaussian blur — two 1D compute passes (horizontal then vertical).

struct Params {
    width: u32,
    height: u32,
    radius: u32,
}

@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var<storage, read> weights: array<f32>;

@compute @workgroup_size(256, 1, 1)
fn blur_h(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if x >= params.width || y >= params.height {
        return;
    }

    let row_start = y * params.width;
    let r = params.radius;
    let ix = i32(x);
    let w_max = i32(params.width) - 1;

    var sum = src[row_start + x] * weights[0];
    var wsum = weights[0];

    for (var k = 1u; k <= r; k++) {
        let ik = i32(k);
        let w = weights[k];
        let lx = u32(clamp(ix - ik, 0, w_max));
        let rx = u32(clamp(ix + ik, 0, w_max));
        sum += src[row_start + lx] * w + src[row_start + rx] * w;
        wsum += w + w;
    }

    dst[row_start + x] = sum / wsum;
}

@compute @workgroup_size(256, 1, 1)
fn blur_v(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if x >= params.width || y >= params.height {
        return;
    }

    let r = params.radius;
    let iy = i32(y);
    let h_max = i32(params.height) - 1;

    var sum = src[y * params.width + x] * weights[0];
    var wsum = weights[0];

    for (var k = 1u; k <= r; k++) {
        let ik = i32(k);
        let w = weights[k];
        let ty = u32(clamp(iy - ik, 0, h_max));
        let by = u32(clamp(iy + ik, 0, h_max));
        sum += src[ty * params.width + x] * w + src[by * params.width + x] * w;
        wsum += w + w;
    }

    dst[y * params.width + x] = sum / wsum;
}
