/// Sampling density for real-world elevation data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerrainDetailLevel {
    /// One sample every four world blocks.
    Low,
    /// One sample every two world blocks.
    #[default]
    Balanced,
    /// One sample per world block, the finest useful spacing for Minecraft terrain.
    High,
}

impl TerrainDetailLevel {
    /// Number of world blocks between requested elevation samples.
    pub const fn sample_spacing_blocks(self) -> usize {
        match self {
            Self::Low => 4,
            Self::Balanced => 2,
            Self::High => 1,
        }
    }

    /// Parse the persisted GUI value, falling back to the stable default for
    /// missing or unrecognized values from older or newer frontends.
    pub fn from_str_lossy(value: &str) -> Self {
        match value {
            "low" => Self::Low,
            "high" => Self::High,
            _ => Self::Balanced,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TerrainDetailLevel;

    #[test]
    fn parses_detail_levels_and_defaults_unknown_values_to_balanced() {
        assert_eq!(
            TerrainDetailLevel::from_str_lossy("low"),
            TerrainDetailLevel::Low
        );
        assert_eq!(
            TerrainDetailLevel::from_str_lossy("balanced"),
            TerrainDetailLevel::Balanced
        );
        assert_eq!(
            TerrainDetailLevel::from_str_lossy("high"),
            TerrainDetailLevel::High
        );
        assert_eq!(
            TerrainDetailLevel::from_str_lossy(""),
            TerrainDetailLevel::Balanced
        );
    }
}
