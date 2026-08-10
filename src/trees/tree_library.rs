//! Shared tree size tiers used by the region tree engine.

const SMALL_MAX_HEIGHT: i32 = 6;
const MEDIUM_MAX_HEIGHT: i32 = 12;
const BIG_MAX_HEIGHT: i32 = 20;
const TALL_MAX_HEIGHT: i32 = 28;

/// Ordered smallest to largest; `Ord` follows declaration order, which is what
/// the size cap compares against.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, clap::ValueEnum)]
pub enum TreeSize {
    Small,
    Medium,
    Big,
    Tall,
    Giant,
}

/// Bucket a schematic by its height.
pub fn size_for_height(height: i32) -> TreeSize {
    if height <= SMALL_MAX_HEIGHT {
        TreeSize::Small
    } else if height <= MEDIUM_MAX_HEIGHT {
        TreeSize::Medium
    } else if height <= BIG_MAX_HEIGHT {
        TreeSize::Big
    } else if height <= TALL_MAX_HEIGHT {
        TreeSize::Tall
    } else {
        TreeSize::Giant
    }
}

/// Bucket a measured canopy top. One block is one metre.
pub fn size_for_canopy_m(metres: u8) -> TreeSize {
    size_for_height(i32::from(metres))
}

/// The five size tiers + which are enabled. Default: all (giant stays 1:1-gated in the engine).
#[derive(Clone, Copy, Debug)]
pub struct SizeFilter {
    pub small: bool,
    pub medium: bool,
    pub big: bool,
    pub tall: bool,
    pub giant: bool,
}

impl Default for SizeFilter {
    fn default() -> Self {
        SizeFilter {
            small: true,
            medium: true,
            big: true,
            tall: true,
            giant: true,
        }
    }
}

impl TreeSize {
    /// Parse a GUI name, falling back to no cap on anything unrecognised.
    #[allow(dead_code)]
    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "small" => TreeSize::Small,
            "medium" => TreeSize::Medium,
            "big" => TreeSize::Big,
            "tall" => TreeSize::Tall,
            _ => TreeSize::Giant,
        }
    }
}

impl SizeFilter {
    /// Every tier up to and including `max`. Oversized picks fall back to a
    /// smaller species in the same community where there is one.
    pub fn up_to(max: TreeSize) -> Self {
        SizeFilter {
            small: TreeSize::Small <= max,
            medium: TreeSize::Medium <= max,
            big: TreeSize::Big <= max,
            tall: TreeSize::Tall <= max,
            giant: TreeSize::Giant <= max,
        }
    }

    pub fn allows(&self, size: TreeSize) -> bool {
        match size {
            TreeSize::Small => self.small,
            TreeSize::Medium => self.medium,
            TreeSize::Big => self.big,
            TreeSize::Tall => self.tall,
            TreeSize::Giant => self.giant,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_buckets() {
        assert_eq!(size_for_height(6), TreeSize::Small);
        assert_eq!(size_for_height(7), TreeSize::Medium);
        assert_eq!(size_for_height(13), TreeSize::Big);
        assert_eq!(size_for_height(21), TreeSize::Tall);
        assert_eq!(size_for_height(35), TreeSize::Giant);
    }

    #[test]
    fn default_enables_all_sizes() {
        let d = SizeFilter::default();
        assert!(d.small && d.medium && d.big && d.tall && d.giant);
    }
}
