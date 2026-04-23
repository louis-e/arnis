use std::path::PathBuf;

/// Subdirectory name for tile cache within the OS cache directory
const TILE_CACHE_DIR_NAME: &str = "arnis-tile-cache";

/// Maximum age for cached tiles in days before they are cleaned up
const TILE_CACHE_MAX_AGE_DAYS: u64 = 7;

/// Returns the tile cache directory path for a specific provider.
/// Uses the OS-standard cache directory (e.g. AppData/Local on Windows, ~/.cache on Linux).
/// Falls back to ./arnis-tile-cache if the OS cache directory is unavailable.
pub fn get_cache_dir(provider_name: &str) -> PathBuf {
    let base = if let Some(cache_dir) = dirs::cache_dir() {
        cache_dir.join(TILE_CACHE_DIR_NAME)
    } else {
        PathBuf::from(format!("./{TILE_CACHE_DIR_NAME}"))
    };
    base.join(provider_name)
}

/// Returns the base tile cache directory path (without provider subdirectory).
pub fn get_base_cache_dir() -> PathBuf {
    if let Some(cache_dir) = dirs::cache_dir() {
        cache_dir.join(TILE_CACHE_DIR_NAME)
    } else {
        PathBuf::from(format!("./{TILE_CACHE_DIR_NAME}"))
    }
}

/// Summary of a cache-clear operation, returned to the GUI so it can
/// report "cleared N files, freed X MB" to the user.
#[derive(Clone, Copy, Debug, Default)]
pub struct CacheClearStats {
    pub files_deleted: u64,
    pub bytes_freed: u64,
    pub errors: u64,
}

impl CacheClearStats {
    /// Combine two stats values — used when we clean multiple caches
    /// (elevation + land-cover) in one UI operation.
    pub fn combined(self, other: Self) -> Self {
        Self {
            files_deleted: self.files_deleted + other.files_deleted,
            bytes_freed: self.bytes_freed + other.bytes_freed,
            errors: self.errors + other.errors,
        }
    }
}

/// Recursively remove everything inside `dir`, leaving `dir` itself in
/// place (so subsequent cache writes don't need to recreate the root
/// handle). Missing directory is a no-op.
///
/// Safety considerations implemented here:
/// - Symlinks are removed but not followed. We never recurse into an
///   arbitrary filesystem the user may have pointed at with a stray
///   symlink inside the cache.
/// - Unreadable entries contribute to the error count instead of
///   propagating — the GUI surfaces the error count as a warning.
/// - No panics: every fs call is matched; transient errors (e.g. a
///   file busy-locked by another reader) just increment `errors`.
pub fn clear_cache_dir(dir: &std::path::Path) -> CacheClearStats {
    let mut stats = CacheClearStats::default();
    if !dir.exists() {
        return stats;
    }
    clear_recursive(dir, &mut stats);
    stats
}

fn clear_recursive(dir: &std::path::Path, stats: &mut CacheClearStats) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => {
            stats.errors += 1;
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => {
                stats.errors += 1;
                continue;
            }
        };
        // Symlink: remove the link itself, don't traverse into it.
        // `std::fs::symlink_metadata` would be another way but the
        // iterator's `metadata()` already returns lstat-style info on
        // most platforms — we explicitly check `is_symlink()` before
        // the dir/file branches for clarity.
        if meta.file_type().is_symlink() {
            if std::fs::remove_file(&path).is_err() {
                stats.errors += 1;
            }
            continue;
        }
        if meta.is_dir() {
            clear_recursive(&path, stats);
            // `remove_dir` only succeeds once the directory is empty;
            // if a nested clear left something behind that's already
            // reflected in `errors` so we just note another failure.
            if std::fs::remove_dir(&path).is_err() {
                stats.errors += 1;
            }
            continue;
        }
        if meta.is_file() {
            let size = meta.len();
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    stats.files_deleted += 1;
                    stats.bytes_freed += size;
                }
                Err(_) => stats.errors += 1,
            }
        }
    }
}

/// Clear every cached elevation tile across all providers. The root
/// cache directory itself is left in place.
pub fn clear_all_cached_tiles() -> CacheClearStats {
    clear_cache_dir(&get_base_cache_dir())
}

/// Cleans up old cached files from all provider cache directories.
/// Only deletes files older than TILE_CACHE_MAX_AGE_DAYS.
pub fn cleanup_old_cached_files() {
    let base_dir = get_base_cache_dir();

    if !base_dir.exists() || !base_dir.is_dir() {
        return;
    }

    let max_age = std::time::Duration::from_secs(TILE_CACHE_MAX_AGE_DAYS * 24 * 60 * 60);
    let now = std::time::SystemTime::now();
    let mut deleted_count = 0;
    let mut error_count = 0;

    // Walk all files in the cache directory tree
    cleanup_dir_recursive(
        &base_dir,
        max_age,
        now,
        &mut deleted_count,
        &mut error_count,
    );

    if deleted_count > 0 {
        println!(
            "Cleaned up {deleted_count} old cached elevation files (older than {TILE_CACHE_MAX_AGE_DAYS} days)"
        );
    }
    if error_count > 1 {
        eprintln!("Warning: Failed to delete {error_count} old cached files");
    }
}

fn cleanup_dir_recursive(
    dir: &std::path::Path,
    max_age: std::time::Duration,
    now: std::time::SystemTime,
    deleted_count: &mut u32,
    error_count: &mut u32,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            cleanup_dir_recursive(&path, max_age, now, deleted_count, error_count);
            continue;
        }

        if !path.is_file() {
            continue;
        }

        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let modified = match metadata.modified() {
            Ok(time) => time,
            Err(_) => continue,
        };

        let age = match now.duration_since(modified) {
            Ok(duration) => duration,
            Err(_) => continue,
        };

        if age > max_age {
            match std::fs::remove_file(&path) {
                Ok(()) => *deleted_count += 1,
                Err(e) => {
                    if *error_count == 0 {
                        eprintln!(
                            "Warning: Failed to delete old cached file {}: {e}",
                            path.display()
                        );
                    }
                    *error_count += 1;
                }
            }
        }
    }
}
