//! Deterministic random number generation for consistent element processing.
//!
//! This module provides seeded RNG that ensures the same element always produces
//! the same random values, regardless of processing order. This is essential for
//! region-by-region streaming where the same element may be processed multiple times
//! (once for each region it touches).
//!
//! # Example
//! ```ignore
//! let mut rng = element_rng(element_id);
//! let color = rng.random_bool(0.5); // Always same result for same element_id
//! ```

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use std::sync::atomic::{AtomicU64, Ordering};

/// The `--seed` of this run. 0, the default, leaves every stream as it always was.
static WORLD_SEED: AtomicU64 = AtomicU64::new(0);

/// Sets the world seed for this process; called once before generation starts.
pub fn set_world_seed(seed: u64) {
    WORLD_SEED.store(seed, Ordering::Relaxed);
}

/// `value` with the world seed mixed in, unchanged while the seed is 0. Every stream and
/// coordinate hash routes its seed through here, so `--seed` changes the look everywhere
/// and stays a pure function of world position and element id.
#[inline]
pub fn seeded(value: u64) -> u64 {
    mix_seed(value, WORLD_SEED.load(Ordering::Relaxed))
}

#[inline]
fn mix_seed(value: u64, seed: u64) -> u64 {
    if seed == 0 {
        return value;
    }
    // SplitMix64 of the seed, so nearby seeds give unrelated looks.
    value ^ crate::building_facades::choose::hash64(seed)
}

/// Creates a deterministic RNG seeded from an element ID.
///
/// The same element ID will always produce the same sequence of random values,
/// ensuring consistent results when an element is processed multiple times
/// (e.g., once per region it touches during streaming).
///
/// # Arguments
/// * `element_id` - The unique OSM element ID (way ID, node ID, or relation ID)
///
/// # Returns
/// A seeded ChaCha8Rng that will produce deterministic random values
#[inline]
pub fn element_rng(element_id: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(seeded(element_id))
}

/// Creates a deterministic RNG seeded from an element ID with an additional salt.
///
/// Use this when you need multiple independent random sequences for the same element.
/// For example, one sequence for wall colors and another for roof style.
///
/// # Arguments
/// * `element_id` - The unique OSM element ID
/// * `salt` - Additional value to create a different sequence (e.g., use different
///   salt values for different purposes within the same element)
#[inline]
#[allow(dead_code)]
pub fn element_rng_salted(element_id: u64, salt: u64) -> ChaCha8Rng {
    // Combine element_id and salt using XOR and bit rotation to avoid collisions
    let combined = element_id ^ salt.rotate_left(32);
    ChaCha8Rng::seed_from_u64(seeded(combined))
}

/// Creates a deterministic RNG seeded from coordinates.
///
/// Use this for per-block randomness that needs to be consistent regardless
/// of processing order (e.g., random flower placement within a natural area).
///
/// # Arguments
/// * `x` - X coordinate
/// * `z` - Z coordinate
/// * `element_id` - The element ID for additional uniqueness
#[inline]
pub fn coord_rng(x: i32, z: i32, element_id: u64) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(seeded(coord_seed(x, z, element_id)))
}

/// Coordinates and element_id combined into a seed. Cast through u32 to handle negative
/// coordinates consistently.
fn coord_seed(x: i32, z: i32, element_id: u64) -> u64 {
    let coord_part = ((x as u32 as i64) << 32) | (z as u32 as i64);
    (coord_part as u64) ^ element_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    #[test]
    fn test_element_rng_deterministic() {
        let mut rng1 = element_rng(12345);
        let mut rng2 = element_rng(12345);

        // Same seed should produce same sequence
        for _ in 0..100 {
            assert_eq!(rng1.random::<u64>(), rng2.random::<u64>());
        }
    }

    #[test]
    fn test_different_elements_different_values() {
        let mut rng1 = element_rng(12345);
        let mut rng2 = element_rng(12346);

        // Different seeds should (almost certainly) produce different values
        let v1: u64 = rng1.random();
        let v2: u64 = rng2.random();
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_salted_rng_different_from_base() {
        let mut rng1 = element_rng(12345);
        let mut rng2 = element_rng_salted(12345, 1);

        let v1: u64 = rng1.random();
        let v2: u64 = rng2.random();
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_coord_rng_deterministic() {
        let mut rng1 = coord_rng(100, 200, 12345);
        let mut rng2 = coord_rng(100, 200, 12345);

        assert_eq!(rng1.random::<u64>(), rng2.random::<u64>());
    }

    #[test]
    fn test_coord_rng_negative_coordinates() {
        // Negative coordinates are common in Minecraft worlds
        let mut rng1 = coord_rng(-100, -200, 12345);
        let mut rng2 = coord_rng(-100, -200, 12345);

        assert_eq!(rng1.random::<u64>(), rng2.random::<u64>());

        // Ensure different negative coords produce different seeds
        let mut rng3 = coord_rng(-100, -200, 12345);
        let mut rng4 = coord_rng(-101, -200, 12345);

        assert_ne!(rng3.random::<u64>(), rng4.random::<u64>());
    }

    #[test]
    fn world_seed_zero_is_the_identity_and_others_reseed() {
        // The global stays 0 under test, so this also pins the default streams.
        assert_eq!(seeded(12345), 12345);
        let mut base = element_rng(12345);
        let mut plain = ChaCha8Rng::seed_from_u64(12345);
        assert_eq!(base.random::<u64>(), plain.random::<u64>());
        for v in [0, 1, 12345, u64::MAX] {
            assert_eq!(mix_seed(v, 0), v);
            assert_ne!(mix_seed(v, 42), v);
            assert_ne!(mix_seed(v, 42), mix_seed(v, 43));
        }
    }

    /// The tree roll of `ground_generation`, for a seed: which of 4096 blocks get a tree.
    fn tree_blocks(seed: u64) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for x in 0..64 {
            for z in 0..64 {
                let mut rng = ChaCha8Rng::seed_from_u64(mix_seed(coord_seed(x, z, 0), seed));
                if rng.random_range(0..30) == 0 {
                    out.push((x, z));
                }
            }
        }
        out
    }

    #[test]
    fn same_seed_same_world() {
        assert_eq!(tree_blocks(42), tree_blocks(42));
        assert!(!tree_blocks(42).is_empty());
    }

    #[test]
    fn different_seed_different_trees() {
        let (a, b) = (tree_blocks(42), tree_blocks(43));
        assert_ne!(a, b);
        assert_ne!(a, tree_blocks(0));
        // Seed 0 is the stream coord_rng always had.
        let mut rng = coord_rng(5, 9, 0);
        let mut plain = ChaCha8Rng::seed_from_u64(coord_seed(5, 9, 0));
        assert_eq!(rng.random::<u64>(), plain.random::<u64>());
    }
}
