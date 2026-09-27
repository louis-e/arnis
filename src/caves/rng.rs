//! Clean-room port of Minecraft 1.21.8 worldgen RNG (Xoroshiro128++ + the seed-derivation /
//! positional-fork / md5 named-seed machinery), derived from the decompiled official mojmap source
//! (`Xoroshiro128PlusPlus`, `XoroshiroRandomSource`, `RandomSupport`).
//!
//! Only the subset the cave NormalNoises need: `next_long`, `next_double`, `next_int(bound)`,
//! `fork_positional`, `from_hash_of`. Gaussian / float paths omitted.

const GOLDEN_RATIO_64: i64 = -7046029254386353131; // 0x9E3779B97F4A7C15 as signed
const SILVER_RATIO_64: i64 = 7640891576956012809; // 0x6A09E667F3BCC909

#[inline]
fn mix_stafford13(mut s: i64) -> i64 {
    s = (s ^ ((s as u64 >> 30) as i64)).wrapping_mul(-4658895280553007687);
    s = (s ^ ((s as u64 >> 27) as i64)).wrapping_mul(-7723592293110705685);
    s ^ ((s as u64 >> 31) as i64)
}

/// 128-bit seed (matches `RandomSupport.Seed128bit`).
#[derive(Clone, Copy)]
pub struct Seed128 {
    pub lo: i64,
    pub hi: i64,
}

impl Seed128 {
    fn mixed(self) -> Seed128 {
        Seed128 {
            lo: mix_stafford13(self.lo),
            hi: mix_stafford13(self.hi),
        }
    }
    fn xor(self, lo: i64, hi: i64) -> Seed128 {
        Seed128 {
            lo: self.lo ^ lo,
            hi: self.hi ^ hi,
        }
    }
}

fn upgrade_seed_to_128bit(seed: i64) -> Seed128 {
    let lo = seed ^ SILVER_RATIO_64;
    let hi = lo.wrapping_add(GOLDEN_RATIO_64);
    Seed128 { lo, hi }.mixed()
}

/// `RandomSupport.seedFromHashOf`: md5(name) → (loBE bytes0..7, hiBE bytes8..15). NOT re-mixed.
fn seed_from_hash_of(name: &str) -> Seed128 {
    let d = md5_16(name.as_bytes());
    let be = |b: &[u8]| -> i64 {
        ((b[0] as i64) << 56)
            | ((b[1] as i64) << 48)
            | ((b[2] as i64) << 40)
            | ((b[3] as i64) << 32)
            | ((b[4] as i64) << 24)
            | ((b[5] as i64) << 16)
            | ((b[6] as i64) << 8)
            | (b[7] as i64)
    };
    Seed128 {
        lo: be(&d[0..8]),
        hi: be(&d[8..16]),
    }
}

/// Xoroshiro128++ core (`Xoroshiro128PlusPlus.java`).
#[derive(Clone)]
pub struct Xoroshiro {
    lo: i64,
    hi: i64,
}

impl Xoroshiro {
    fn new(mut lo: i64, mut hi: i64) -> Self {
        if (lo | hi) == 0 {
            lo = GOLDEN_RATIO_64;
            hi = SILVER_RATIO_64;
        }
        Xoroshiro { lo, hi }
    }
    #[inline]
    pub fn next_long(&mut self) -> i64 {
        let l = self.lo;
        let mut l1 = self.hi;
        let l2 = l.wrapping_add(l1).rotate_left(17).wrapping_add(l);
        l1 ^= l;
        self.lo = l.rotate_left(49) ^ l1 ^ (l1 << 21);
        self.hi = l1.rotate_left(28);
        l2
    }
}

/// `XoroshiroRandomSource`.
#[derive(Clone)]
pub struct XoroRandom {
    rng: Xoroshiro,
}

impl XoroRandom {
    /// `new XoroshiroRandomSource(long seed)` — applies `upgradeSeedTo128bit`.
    pub fn from_seed(seed: i64) -> Self {
        let s = upgrade_seed_to_128bit(seed);
        XoroRandom {
            rng: Xoroshiro::new(s.lo, s.hi),
        }
    }
    /// `new XoroshiroRandomSource(Seed128bit)` — used directly, no upgrade (for `from_hash_of`).
    fn from_seed128(s: Seed128) -> Self {
        XoroRandom {
            rng: Xoroshiro::new(s.lo, s.hi),
        }
    }
    #[inline]
    pub fn next_long(&mut self) -> i64 {
        self.rng.next_long()
    }
    #[inline]
    fn next_bits(&mut self, bits: u32) -> i64 {
        ((self.next_long() as u64) >> (64 - bits)) as i64
    }
    /// `nextDouble = (float)nextBits(53) * 1.110223E-16F` — vanilla computes this in **float**
    /// precision (the `(float)` cast is real, NOT a decompiler artifact: confirmed by the Java parity
    /// harness, which diverged from a full-f64 version after ~7 sig-figs). Must match exactly, since
    /// ImprovedNoise's xo/yo/zo offsets are `nextDouble()*256`.
    #[inline]
    pub fn next_double(&mut self) -> f64 {
        (self.next_bits(53) as f32 * 1.110223e-16f32) as f64
    }
    /// `nextFloat = (float)nextBits(24) * 5.9604645E-8F` (2^-24).
    #[allow(dead_code)] // vanilla-parity API kept complete
    #[inline]
    pub fn next_float(&mut self) -> f32 {
        self.next_bits(24) as f32 * 5.9604645e-8f32
    }
    #[inline]
    fn next_int_raw(&mut self) -> i32 {
        self.next_long() as i32
    }
    /// `nextInt(bound)` — the exact Lemire path from `XoroshiroRandomSource`.
    pub fn next_int(&mut self, bound: i32) -> i32 {
        debug_assert!(bound > 0);
        let b = bound as u64;
        let mut l = (self.next_int_raw() as u32) as u64;
        let mut l1 = l.wrapping_mul(b);
        let mut l2 = l1 & 0xFFFF_FFFF;
        if l2 < b {
            let i = ((1u64 << 32).wrapping_sub(b)) % b; // remainderUnsigned(~bound+1, bound)
            while l2 < i {
                l = (self.next_int_raw() as u32) as u64;
                l1 = l.wrapping_mul(b);
                l2 = l1 & 0xFFFF_FFFF;
            }
        }
        (l1 >> 32) as i32
    }
    /// `forkPositional()` → factory seeded from two draws.
    pub fn fork_positional(&mut self) -> PositionalFactory {
        let lo = self.next_long();
        let hi = self.next_long();
        PositionalFactory { lo, hi }
    }
}

/// `XoroshiroPositionalRandomFactory`.
#[derive(Clone, Copy)]
pub struct PositionalFactory {
    lo: i64,
    hi: i64,
}

impl PositionalFactory {
    /// `fromHashOf(name)` — md5 seed xored with the factory, used directly (not re-mixed).
    #[allow(clippy::wrong_self_convention)] // mirrors vanilla's forkPositional().fromHashOf
    pub fn from_hash_of(&self, name: &str) -> XoroRandom {
        let s = seed_from_hash_of(name).xor(self.lo, self.hi);
        XoroRandom::from_seed128(s)
    }
}

// ---------------------------------------------------------------------------------------------
// Minimal MD5 (RFC 1321), hand-rolled — returns the 16-byte digest. No external crate so the
// build stays offline-safe. Used only for the named-noise seeding (a few dozen calls at startup).
// ---------------------------------------------------------------------------------------------
fn md5_16(msg: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    let mut data = msg.to_vec();
    let bitlen = (msg.len() as u64).wrapping_mul(8);
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bitlen.to_le_bytes());

    for chunk in data.as_chunks::<64>().0 {
        let mut m = [0u32; 16];
        for i in 0..16 {
            m[i] = u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_known_vectors() {
        // RFC 1321 test vectors
        let hex = |d: [u8; 16]| d.iter().map(|b| format!("{:02x}", b)).collect::<String>();
        assert_eq!(hex(md5_16(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(md5_16(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex(md5_16(b"The quick brown fox jumps over the lazy dog")),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
    }
}
