//! Shared tree size tiers used by the region tree engine.

const SMALL_MAX_HEIGHT: i32 = 6;
const MEDIUM_MAX_HEIGHT: i32 = 12;
const BIG_MAX_HEIGHT: i32 = 20;
const TALL_MAX_HEIGHT: i32 = 28;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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

/// The five size tiers + which are enabled. Default: all but Giant.
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
            giant: false,
        }
    }
}

impl SizeFilter {
    pub fn allows(&self, size: TreeSize) -> bool {
        match size {
            TreeSize::Small => self.small,
            TreeSize::Medium => self.medium,
            TreeSize::Big => self.big,
            TreeSize::Tall => self.tall,
            TreeSize::Giant => self.giant,
        }
    }

    /// Parse a comma list of enabled tiers. An empty/all-unknown list falls back to the default.
    pub fn parse(list: &str) -> SizeFilter {
        let mut f = SizeFilter {
            small: false,
            medium: false,
            big: false,
            tall: false,
            giant: false,
        };
        let mut any = false;
        for tok in list.split(',') {
            match tok.trim().to_ascii_lowercase().as_str() {
                "small" | "s" => {
                    f.small = true;
                    any = true;
                }
                "medium" | "m" => {
                    f.medium = true;
                    any = true;
                }
                "big" | "b" => {
                    f.big = true;
                    any = true;
                }
                "tall" | "t" => {
                    f.tall = true;
                    any = true;
                }
                "giant" | "g" | "huge" => {
                    f.giant = true;
                    any = true;
                }
                _ => {}
            }
        }
        if any {
            f
        } else {
            SizeFilter::default()
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
    fn size_filter_parse() {
        let f = SizeFilter::parse("small,big");
        assert!(f.small && f.big && !f.medium && !f.giant);
        let d = SizeFilter::parse("");
        assert!(d.small && d.tall && !d.giant);
    }
}
