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
//! let color = rng.gen_bool(0.5); // Always same result for same element_id
//! ```

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

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
    ChaCha8Rng::seed_from_u64(element_id)
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
    ChaCha8Rng::seed_from_u64(combined)
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
    // Combine coordinates and element_id into a seed.
    // Cast through u32 to handle negative coordinates consistently.
    let coord_part = ((x as u32 as i64) << 32) | (z as u32 as i64);
    let seed = (coord_part as u64) ^ element_id;
    ChaCha8Rng::seed_from_u64(seed)
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
            assert_eq!(rng1.gen::<u64>(), rng2.gen::<u64>());
        }
    }

    #[test]
    fn test_different_elements_different_values() {
        let mut rng1 = element_rng(12345);
        let mut rng2 = element_rng(12346);

        // Different seeds should (almost certainly) produce different values
        let v1: u64 = rng1.gen();
        let v2: u64 = rng2.gen();
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_salted_rng_different_from_base() {
        let mut rng1 = element_rng(12345);
        let mut rng2 = element_rng_salted(12345, 1);

        let v1: u64 = rng1.gen();
        let v2: u64 = rng2.gen();
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_coord_rng_deterministic() {
        let mut rng1 = coord_rng(100, 200, 12345);
        let mut rng2 = coord_rng(100, 200, 12345);

        assert_eq!(rng1.gen::<u64>(), rng2.gen::<u64>());
    }
}
