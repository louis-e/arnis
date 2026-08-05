// Terrain anomaly repair — one 5x5 median/MAD pass per dispatch, mirroring
// `repair_terrain_anomalies` (postprocess.rs): a cell whose deviation from
// the neighbourhood median exceeds both an absolute threshold and a
// MAD-relative factor is replaced by the median. The host re-dispatches
// until a pass repairs nothing (early break) or MAX_ANOMALY_PASSES is hit.
//
// Border cells (2-cell rim), non-finite centres, and neighbourhoods with
// fewer than 8 finite samples pass through unchanged, as on CPU.

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

const RADIUS: i32 = 2;
const MAX_NEIGHBOURS: u32 = 24u; // 5x5 window minus the centre
const ABS_THRESHOLD: f32 = 6.0;
const RELATIVE_FACTOR: f32 = 3.0;
const MIN_SAMPLES: u32 = 8u;

fn is_finite_f(v: f32) -> bool {
    return (bitcast<u32>(v) & 0x7f800000u) != 0x7f800000u;
}

// Insertion sort over the first n elements; n <= 24 so this stays cheap.
fn sort_first_n(a: ptr<function, array<f32, 24>>, n: u32) {
    for (var i = 1u; i < n; i++) {
        let key = (*a)[i];
        var j = i;
        while j > 0u && (*a)[j - 1u] > key {
            (*a)[j] = (*a)[j - 1u];
            j -= 1u;
        }
        (*a)[j] = key;
    }
}

@compute @workgroup_size(256, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    if x >= params.width || y >= params.height {
        return;
    }
    let idx = y * params.width + x;
    let center = src[idx];

    let on_border = x < u32(RADIUS) || y < u32(RADIUS)
        || x >= params.width - u32(RADIUS) || y >= params.height - u32(RADIUS);
    if on_border || !is_finite_f(center) {
        dst[idx] = center;
        return;
    }

    var vals: array<f32, 24>;
    var n = 0u;
    for (var dz = -RADIUS; dz <= RADIUS; dz++) {
        for (var dx = -RADIUS; dx <= RADIUS; dx++) {
            if dx == 0 && dz == 0 {
                continue;
            }
            let v = src[u32(i32(y) + dz) * params.width + u32(i32(x) + dx)];
            if is_finite_f(v) {
                vals[n] = v;
                n += 1u;
            }
        }
    }
    if n < MIN_SAMPLES {
        dst[idx] = center;
        return;
    }

    sort_first_n(&vals, n);
    // n/2 with integer division = upper median, matching the CPU
    // select_nth_unstable(len/2).
    let median = vals[n / 2u];

    for (var i = 0u; i < n; i++) {
        vals[i] = abs(vals[i] - median);
    }
    sort_first_n(&vals, n);
    let mad = vals[n / 2u];

    let deviation = abs(center - median);
    if deviation > ABS_THRESHOLD && deviation > RELATIVE_FACTOR * max(mad, 1.0) {
        dst[idx] = median;
        atomicAdd(&counter[0], 1u);
    } else {
        dst[idx] = center;
    }
}
