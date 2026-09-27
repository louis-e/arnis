//! Clean-room port of Minecraft 1.21.8 `ImprovedNoise` / `PerlinNoise` / `NormalNoise` (and the
//! `Mth` helpers they rely on), derived from the decompiled official mojmap source. Bit-for-bit
//! faithful (validated by a Java value-parity harness).

use super::rng::{PositionalFactory, XoroRandom};

// SimplexNoise.GRADIENT (16 × 3) — used by ImprovedNoise via `gradDot(g & 15, ...)`.
const GRADIENT: [[i32; 3]; 16] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
    [1, 1, 0],
    [0, -1, 1],
    [-1, 1, 0],
    [0, -1, -1],
];

#[inline]
fn grad_dot(g: usize, x: f64, y: f64, z: f64) -> f64 {
    let v = GRADIENT[g & 15];
    v[0] as f64 * x + v[1] as f64 * y + v[2] as f64 * z
}

// ---- Mth helpers (exact) ----
#[inline]
fn mth_floor(v: f64) -> i32 {
    let i = v as i32;
    if v < i as f64 {
        i - 1
    } else {
        i
    }
}
#[inline]
fn lfloor(v: f64) -> i64 {
    let l = v as i64;
    if v < l as f64 {
        l - 1
    } else {
        l
    }
}
#[inline]
fn smoothstep(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
#[inline]
fn lerp(d: f64, a: f64, b: f64) -> f64 {
    a + d * (b - a)
}
#[inline]
fn lerp2(d1: f64, d2: f64, s1: f64, e1: f64, s2: f64, e2: f64) -> f64 {
    lerp(d2, lerp(d1, s1, e1), lerp(d1, s2, e2))
}
#[allow(clippy::too_many_arguments)]
#[inline]
fn lerp3(
    d1: f64,
    d2: f64,
    d3: f64,
    s1: f64,
    e1: f64,
    s2: f64,
    e2: f64,
    s3: f64,
    e3: f64,
    s4: f64,
    e4: f64,
) -> f64 {
    lerp(
        d3,
        lerp2(d1, d2, s1, e1, s2, e2),
        lerp2(d1, d2, s3, e3, s4, e4),
    )
}

/// One octave of improved perlin noise (`ImprovedNoise`).
struct ImprovedNoise {
    p: [u8; 256],
    xo: f64,
    yo: f64,
    zo: f64,
}

impl ImprovedNoise {
    fn new(random: &mut XoroRandom) -> Self {
        let xo = random.next_double() * 256.0;
        let yo = random.next_double() * 256.0;
        let zo = random.next_double() * 256.0;
        let mut p = [0u8; 256];
        for (i, v) in p.iter_mut().enumerate() {
            *v = i as u8;
        }
        for i in 0..256 {
            let r = random.next_int(256 - i as i32) as usize;
            p.swap(i, i + r);
        }
        ImprovedNoise { p, xo, yo, zo }
    }

    #[inline]
    fn p(&self, idx: i32) -> i32 {
        self.p[(idx & 0xFF) as usize] as i32
    }

    /// `noise(x,y,z, yScale, yMax)`. For cave noises yScale is always 0 → `weirdDeltaY == deltaY`.
    fn noise(&self, x: f64, y: f64, z: f64, y_scale: f64, y_max: f64) -> f64 {
        let dx = x + self.xo;
        let dy = y + self.yo;
        let dz = z + self.zo;
        let fx = mth_floor(dx);
        let fy = mth_floor(dy);
        let fz = mth_floor(dz);
        let d3 = dx - fx as f64;
        let d4 = dy - fy as f64;
        let d5 = dz - fz as f64;
        let d7 = if y_scale != 0.0 {
            let d6 = if y_max >= 0.0 && y_max < d4 {
                y_max
            } else {
                d4
            };
            (d6 / y_scale + 1.0e-7_f32 as f64).floor() * y_scale
        } else {
            0.0
        };
        self.sample_and_lerp(fx, fy, fz, d3, d4 - d7, d5, d4)
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_and_lerp(
        &self,
        gx: i32,
        gy: i32,
        gz: i32,
        dx: f64,
        weird_dy: f64,
        dz: f64,
        dy: f64,
    ) -> f64 {
        let i = self.p(gx);
        let i1 = self.p(gx + 1);
        let i2 = self.p(i + gy);
        let i3 = self.p(i + gy + 1);
        let i4 = self.p(i1 + gy);
        let i5 = self.p(i1 + gy + 1);
        let d = grad_dot(self.p(i2 + gz) as usize, dx, weird_dy, dz);
        let d1 = grad_dot(self.p(i4 + gz) as usize, dx - 1.0, weird_dy, dz);
        let d2 = grad_dot(self.p(i3 + gz) as usize, dx, weird_dy - 1.0, dz);
        let d3 = grad_dot(self.p(i5 + gz) as usize, dx - 1.0, weird_dy - 1.0, dz);
        let d4 = grad_dot(self.p(i2 + gz + 1) as usize, dx, weird_dy, dz - 1.0);
        let d5 = grad_dot(self.p(i4 + gz + 1) as usize, dx - 1.0, weird_dy, dz - 1.0);
        let d6 = grad_dot(self.p(i3 + gz + 1) as usize, dx, weird_dy - 1.0, dz - 1.0);
        let d7 = grad_dot(
            self.p(i5 + gz + 1) as usize,
            dx - 1.0,
            weird_dy - 1.0,
            dz - 1.0,
        );
        let sx = smoothstep(dx);
        let sy = smoothstep(dy);
        let sz = smoothstep(dz);
        lerp3(sx, sy, sz, d, d1, d2, d3, d4, d5, d6, d7)
    }
}

const ROUND_OFF: f64 = 3.3554432e7;
#[inline]
fn wrap(v: f64) -> f64 {
    v - lfloor(v / ROUND_OFF + 0.5) as f64 * ROUND_OFF
}

/// `PerlinNoise` — stack of octaves seeded via `forkPositional().fromHashOf("octave_N")`.
pub struct PerlinNoise {
    levels: Vec<Option<ImprovedNoise>>,
    amplitudes: Vec<f64>,
    lowest_freq_input_factor: f64,
    lowest_freq_value_factor: f64,
}

impl PerlinNoise {
    pub fn create(random: &mut XoroRandom, first_octave: i32, amplitudes: &[f64]) -> Self {
        let size = amplitudes.len();
        let i = -first_octave;
        let mut levels: Vec<Option<ImprovedNoise>> = (0..size).map(|_| None).collect();
        let pf: PositionalFactory = random.fork_positional();
        for idx in 0..size {
            if amplitudes[idx] != 0.0 {
                let octave = first_octave + idx as i32;
                let mut r = pf.from_hash_of(&format!("octave_{}", octave));
                levels[idx] = Some(ImprovedNoise::new(&mut r));
            }
        }
        let lowest_freq_input_factor = 2f64.powi(-i);
        let lowest_freq_value_factor = 2f64.powi(size as i32 - 1) / (2f64.powi(size as i32) - 1.0);
        PerlinNoise {
            levels,
            amplitudes: amplitudes.to_vec(),
            lowest_freq_input_factor,
            lowest_freq_value_factor,
        }
    }

    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut d = 0.0;
        let mut d1 = self.lowest_freq_input_factor;
        let mut d2 = self.lowest_freq_value_factor;
        for (idx, lvl) in self.levels.iter().enumerate() {
            if let Some(n) = lvl {
                let v = n.noise(wrap(x * d1), wrap(y * d1), wrap(z * d1), 0.0, 0.0);
                d += self.amplitudes[idx] * v * d2;
            }
            d1 *= 2.0;
            d2 /= 2.0;
        }
        d
    }
}

const INPUT_FACTOR: f64 = 1.0181268882175227;

/// `NormalNoise` — two perlin stacks, the second sampled at `× INPUT_FACTOR`.
pub struct NormalNoise {
    first: PerlinNoise,
    second: PerlinNoise,
    value_factor: f64,
}

impl NormalNoise {
    /// `NormalNoise.create(random, firstOctave, amplitudes)`.
    pub fn create(random: &mut XoroRandom, first_octave: i32, amplitudes: &[f64]) -> Self {
        let first = PerlinNoise::create(random, first_octave, amplitudes);
        let second = PerlinNoise::create(random, first_octave, amplitudes);
        // span of non-zero amplitude indices
        let mut min_idx = i32::MAX;
        let mut max_idx = i32::MIN;
        for (i, &a) in amplitudes.iter().enumerate() {
            if a != 0.0 {
                min_idx = min_idx.min(i as i32);
                max_idx = max_idx.max(i as i32);
            }
        }
        let value_factor = 0.16666666666666666 / expected_deviation(max_idx - min_idx);
        NormalNoise {
            first,
            second,
            value_factor,
        }
    }

    #[inline]
    pub fn get_value(&self, x: f64, y: f64, z: f64) -> f64 {
        (self.first.get_value(x, y, z)
            + self
                .second
                .get_value(x * INPUT_FACTOR, y * INPUT_FACTOR, z * INPUT_FACTOR))
            * self.value_factor
    }
}

#[inline]
fn expected_deviation(octaves: i32) -> f64 {
    0.1 * (1.0 + 1.0 / (octaves as f64 + 1.0))
}
