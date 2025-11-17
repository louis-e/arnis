//! Platform and SIMD feature detection for performance tuning (Apple Silicon, AVX2, NEON, RAM, cores)
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdFeatures {
    None,
    NEON,
    AVX2,
    AVX512,
    Other,
}

#[derive(Debug, Clone)]
pub struct PlatformInfo {
    pub total_ram_bytes: u64,
    pub logical_cpus: usize,
    pub physical_cpus: usize,
    pub simd: SimdFeatures,
    pub arch: &'static str,
}

impl fmt::Display for SimdFeatures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SimdFeatures::None => write!(f, "None"),
            SimdFeatures::NEON => write!(f, "NEON"),
            SimdFeatures::AVX2 => write!(f, "AVX2"),
            SimdFeatures::AVX512 => write!(f, "AVX512"),
            SimdFeatures::Other => write!(f, "Other"),
        }
    }
}

impl PlatformInfo {
    pub fn detect() -> Self {
        let logical_cpus = num_cpus::get();
        let physical_cpus = num_cpus::get_physical();
        let total_ram_bytes = get_total_ram_bytes();
        let arch = std::env::consts::ARCH;
        let simd = detect_simd_features();
        PlatformInfo {
            total_ram_bytes,
            logical_cpus,
            physical_cpus,
            simd,
            arch,
        }
    }
}

fn get_total_ram_bytes() -> u64 {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    sys.total_memory() * 1024
}

fn detect_simd_features() -> SimdFeatures {
    // Apple Silicon (aarch64) always has NEON
    #[cfg(all(target_arch = "aarch64", feature = "simd-native"))]
    {
        return SimdFeatures::NEON;
    }
    #[cfg(all(target_arch = "x86_64", feature = "simd-native"))]
    {
        // Use runtime detection for AVX2/AVX512
        if std::is_x86_feature_detected!("avx512f") {
            return SimdFeatures::AVX512;
        }
        if std::is_x86_feature_detected!("avx2") {
            return SimdFeatures::AVX2;
        }
        return SimdFeatures::Other;
    }
    SimdFeatures::None
}
