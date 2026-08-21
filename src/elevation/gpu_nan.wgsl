// NaN fill — one Jacobi iteration of the CPU `fill_nan_values` per dispatch.
//
// Each NaN cell becomes the mean of its finite in-bounds 3x3 neighbours
// (reading the snapshot in `src`), and the host re-dispatches until a pass
// fills nothing, exactly like the CPU convergence loop. Note the CPU path
// treats inf as a *valid* neighbour value (it only checks `is_nan`), so this
// shader deliberately tests NaN rather than finiteness.

struct Params {
    width: u32,
    height: u32,
    pad0: u32,
    pad1: u32,
}

@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var<storage, read_write> counter: array<atomic<u32>>;

fn is_nan_f(v: f32) -> bool {
    let b = bitcast<u32>(v);
    return (b & 0x7f800000u) == 0x7f800000u && (b & 0x007fffffu) != 0u;
}

fn nan_f() -> f32 {
    return bitcast<f32>(0x7fc00000u);
}

@compute @workgroup_size(256, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if x >= params.width || y >= params.height {
        return;
    }
    let idx = y * params.width + x;
    let v = src[idx];
    if !is_nan_f(v) {
        dst[idx] = v;
        return;
    }

    var sum = 0.0;
    var count = 0u;
    for (var dz = -1; dz <= 1; dz++) {
        for (var dx = -1; dx <= 1; dx++) {
            let nx = i32(x) + dx;
            let ny = i32(y) + dz;
            if nx < 0 || ny < 0 || nx >= i32(params.width) || ny >= i32(params.height) {
                continue;
            }
            let nv = src[u32(ny) * params.width + u32(nx)];
            if !is_nan_f(nv) {
                sum += nv;
                count += 1u;
            }
        }
    }

    if count > 0u {
        dst[idx] = sum / f32(count);
        atomicAdd(&counter[0], 1u);
    } else {
        dst[idx] = nan_f();
    }
}
