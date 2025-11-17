//! Global performance config for RAM/thread/CPU optimizations (Apple Silicon, cross-platform)
use crate::cpu_info::{PlatformInfo, SimdFeatures};
use once_cell::sync::OnceCell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuOptMode {
    Auto,
    Off,
    Native,
}

#[derive(Debug, Clone)]
pub struct PerformanceConfig {
    pub effective_max_ram_bytes: u64,
    pub effective_threads: usize,
    pub cpu_opt_mode: CpuOptMode,
    pub platform: PlatformInfo,
}

static PERF_CONFIG: OnceCell<PerformanceConfig> = OnceCell::new();

impl PerformanceConfig {
    /// Initialize from detected platform and (future) GUI/CLI settings
    pub fn init_default() -> &'static Self {
        let platform = PlatformInfo::detect();
        // Default: 16GB or system RAM, whichever is lower
        let default_ram = 16 * 1024 * 1024 * 1024u64;
        let effective_max_ram_bytes = platform.total_ram_bytes.min(default_ram);
        let effective_threads = platform.logical_cpus.max(1);
        let cpu_opt_mode = match platform.simd {
            SimdFeatures::NEON | SimdFeatures::AVX2 | SimdFeatures::AVX512 => CpuOptMode::Native,
            _ => CpuOptMode::Auto,
        };
        let config = PerformanceConfig {
            effective_max_ram_bytes,
            effective_threads,
            cpu_opt_mode,
            platform,
        };
        PERF_CONFIG.get_or_init(|| config)
    }

    pub fn get() -> &'static Self {
        PERF_CONFIG
            .get()
            .expect("PerformanceConfig not initialized")
    }

    pub fn log_config(&self) {
        println!(
            "[perf] RAM: {:.1} GB, threads: {}, arch: {}, SIMD: {}",
            self.effective_max_ram_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            self.effective_threads,
            self.platform.arch,
            self.platform.simd
        );
    }
}
