//! Relative popularity of the schematic tree size tiers (`--tree-size-weights`).
//!
//! A tree of unmeasured height takes one of five height bands (`default_height_m`),
//! one per tier. The weights multiply each band's share and rescale the same
//! position-seeded roll, so a weighted pick is still identical from any tile. Left at
//! their defaults they reproduce the bands exactly.

use crate::trees::tree_library::SizeFilter;

/// The tiers' names, smallest to largest, as `TreeSize` parses them.
pub const NAMES: [&str; 5] = ["small", "medium", "big", "tall", "giant"];

/// Default share of each height band out of 1000, smallest tier to largest.
const BAND_SHARES: [u32; 5] = [150, 350, 330, 135, 35];

/// Percent per tier, smallest to largest: 100 keeps the default share, 0 turns the tier off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SizeWeights([u16; 5]);

impl Default for SizeWeights {
    fn default() -> Self {
        SizeWeights([100; 5])
    }
}

impl SizeWeights {
    /// Parse `name=percent` pairs (small, medium, big, tall, giant), each 0-200.
    /// Omitted tiers stay at 100; not every tier may be 0.
    pub fn parse(spec: &str) -> Result<SizeWeights, String> {
        let mut w = [100u16; 5];
        for tok in spec.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            let (k, v) = tok
                .split_once('=')
                .ok_or_else(|| format!("{tok}: expected name=percent"))?;
            let k = k.trim().to_ascii_lowercase();
            let i = NAMES
                .iter()
                .position(|&n| n == k)
                .ok_or_else(|| format!("{k}: expected one of {}", NAMES.join(", ")))?;
            w[i] = v
                .trim()
                .parse()
                .ok()
                .filter(|&pct| pct <= 200)
                .ok_or_else(|| format!("{tok}: percent must be 0-200"))?;
        }
        if w.iter().all(|&pct| pct == 0) {
            return Err(format!("{spec}: give at least one non-zero percent"));
        }
        Ok(SizeWeights(w))
    }

    /// A tier weighted 0 is off, so a canopy-measured height cannot bring it back either.
    pub fn restrict(&self, sizes: &mut SizeFilter) {
        let [s, m, b, t, g] = self.0.map(|pct| pct > 0);
        sizes.small &= s;
        sizes.medium &= m;
        sizes.big &= b;
        sizes.tall &= t;
        sizes.giant &= g;
    }
}

/// The height band (0 = small .. 4 = giant) for `roll` in 0..1000.
pub fn band(roll: u64, weights: Option<&SizeWeights>) -> usize {
    let w = weights.copied().unwrap_or_default().0;
    let shares = BAND_SHARES.map(f64::from);
    let weighted: Vec<f64> = shares
        .iter()
        .zip(w)
        .map(|(s, pct)| s * f64::from(pct) / 100.0)
        .collect();
    let sum: f64 = weighted.iter().sum();
    // Exact for the defaults: every share is a whole number and they sum to 1000.
    let target = roll as f64 * sum / 1000.0;
    let mut cum = 0.0;
    for (i, share) in weighted.iter().enumerate() {
        cum += share;
        if target < cum {
            return i;
        }
    }
    // Rounding at the very top of the roll: the last tier still on.
    weighted.iter().rposition(|&s| s > 0.0).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_weights_parse() {
        let w = SizeWeights::parse("big=150,tall=50,giant=0").unwrap();
        assert_eq!(w.0, [100, 100, 150, 50, 0]);
        assert_eq!(SizeWeights::parse(" Big = 70 ").unwrap().0[2], 70);
        assert!(SizeWeights::parse("huge=10").is_err());
        assert!(SizeWeights::parse("big").is_err());
        assert!(SizeWeights::parse("big=250").is_err());
        assert!(SizeWeights::parse("small=0,medium=0,big=0,tall=0,giant=0").is_err());
    }

    #[test]
    fn zero_weight_turns_the_tier_off() {
        use crate::trees::tree_library::TreeSize;
        let mut f = SizeFilter::up_to(TreeSize::Tall);
        SizeWeights::parse("medium=0").unwrap().restrict(&mut f);
        assert!(f.small && !f.medium && f.big && f.tall && !f.giant);
    }

    #[test]
    fn defaults_reproduce_the_height_bands() {
        let old = |roll: u64| {
            [150, 500, 830, 965, 1000]
                .iter()
                .position(|&c| roll < c)
                .unwrap()
        };
        let d = SizeWeights::default();
        for roll in 0..1000 {
            assert_eq!(band(roll, None), old(roll));
            assert_eq!(band(roll, Some(&d)), old(roll));
        }
    }

    #[test]
    fn weights_shift_the_mix() {
        let count =
            |w: &SizeWeights, tier: usize| (0..1000).filter(|&r| band(r, Some(w)) == tier).count();
        let d = SizeWeights::default();
        let w = SizeWeights::parse("small=0,big=200").unwrap();
        assert_eq!(count(&w, 0), 0);
        assert!(count(&w, 2) > count(&d, 2));
        assert_eq!(count(&SizeWeights::parse("giant=0").unwrap(), 4), 0);
    }
}
