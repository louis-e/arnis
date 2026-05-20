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
    element_rng_salted_with_global(element_id, salt, 0)
}

/// Same as `element_rng_salted` plus a project-wide `global_seed` mixed
/// into the seed. Lets external schedulers force re-renders to land on
/// the same building palette regardless of which Arnis process ran. When
/// `global_seed = 0`, behaviour is identical to `element_rng_salted`.
///
/// # Arguments
/// * `element_id` — OSM element ID
/// * `salt` — per-decision salt (1=wall, 2=floor, ...)
/// * `global_seed` — project-wide seed (0 = no global mixing)
#[inline]
#[allow(dead_code)]
pub fn element_rng_salted_with_global(element_id: u64, salt: u64, global_seed: u64) -> ChaCha8Rng {
    let combined = element_id ^ salt.rotate_left(32) ^ global_seed.rotate_left(16);
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
    fn test_element_rng_salted_with_global_seed_zero_matches_unseeded() {
        // Backward compat: global_seed=0 must produce the same stream
        // as the old `element_rng_salted(id, salt)`.
        let mut rng_old = element_rng_salted(12345, 1);
        let mut rng_new = element_rng_salted_with_global(12345, 1, 0);
        for _ in 0..10 {
            assert_eq!(rng_old.random::<u64>(), rng_new.random::<u64>());
        }
    }

    #[test]
    fn test_element_rng_salted_with_global_same_seed_deterministic() {
        let mut a = element_rng_salted_with_global(12345, 1, 42);
        let mut b = element_rng_salted_with_global(12345, 1, 42);
        for _ in 0..10 {
            assert_eq!(a.random::<u64>(), b.random::<u64>());
        }
    }

    #[test]
    fn test_element_rng_salted_with_global_different_seeds_diverge() {
        let mut a = element_rng_salted_with_global(12345, 1, 1);
        let mut b = element_rng_salted_with_global(12345, 1, 999);
        let v1: u64 = a.random();
        let v2: u64 = b.random();
        assert_ne!(v1, v2);
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
}
