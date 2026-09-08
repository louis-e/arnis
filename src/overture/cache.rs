//! On-disk cache for Overture Maps data.
//!
//! Overture publishes immutable, dated releases (`2026-08-19.0`) and keeps only
//! the newest two online under a 60-day retention rule. Two consequences shape
//! this module:
//!
//! * **Everything is keyed by release**, so a cached byte range can never go
//!   stale. A release name identifies fixed content; there is no TTL to get
//!   wrong, and no revalidation request to pay for. The only thing that ages is
//!   *which* release is newest, which release discovery answers on every run.
//! * **The release that last worked is remembered**, because the compile-time
//!   fallback is a date and dates rot. When discovery fails on a machine whose
//!   last successful release has since been retired, we would otherwise fall
//!   back to a constant that expired even earlier.
//!
//! Writes are atomic (temp file + rename) so a killed run, a full disk or two
//! concurrent Arnis processes can never leave a truncated file that a later run
//! reads back as valid data.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::elevation::cache::CacheClearStats;

/// Cache root under the OS cache directory, alongside `arnis-tile-cache` and
/// `arnis-landcover-cache`.
const OVERTURE_CACHE_DIR: &str = "arnis-overture-cache";

/// File holding the last release that actually served data on this machine.
const LAST_GOOD_RELEASE_FILE: &str = "last_good_release";

/// Cache root. Falls back to a relative directory when the OS has no cache dir,
/// matching every other Arnis cache.
pub fn cache_root() -> PathBuf {
    match dirs::cache_dir() {
        Some(dir) => dir.join(OVERTURE_CACHE_DIR),
        None => PathBuf::from(format!("./{OVERTURE_CACHE_DIR}")),
    }
}

/// Clear every cached Overture artefact. Entry point for the GUI cache-clean
/// command, which calls one of these per cache root.
pub fn clear_overture_cache() -> CacheClearStats {
    crate::elevation::cache::clear_cache_dir(&cache_root())
}

/// A release name is `YYYY-MM-DD.N`. Validated before it is ever used as a path
/// segment, so a hostile or corrupt bucket listing cannot escape the cache root.
pub fn is_valid_release(release: &str) -> bool {
    let Some((date, revision)) = release.split_once('.') else {
        return false;
    };
    if revision.is_empty() || !revision.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let parts: Vec<&str> = date.split('-').collect();
    matches!(parts.as_slice(), [y, m, d]
        if y.len() == 4 && m.len() == 2 && d.len() == 2
            && [y, m, d].iter().all(|p| p.bytes().all(|b| b.is_ascii_digit())))
}

/// Directory holding one release's cached artefacts. `None` for a name that is
/// not a well-formed release, which keeps path traversal impossible by
/// construction rather than by escaping.
pub fn release_dir(release: &str) -> Option<PathBuf> {
    is_valid_release(release).then(|| cache_root().join(release))
}

/// Read a cached file, or `None` if it is absent or unreadable. A read error is
/// never fatal: the caller refetches.
pub fn read(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// Write `bytes` to `path` atomically.
///
/// The temp file carries the process id so two Arnis processes caching the same
/// tile cannot write through each other's partial file; the rename then makes
/// whichever finishes last the visible one, and both hold identical bytes.
/// Every failure is silent by design - the cache is an optimisation, and a
/// read-only or full disk must not fail a generation.
pub fn write_atomic(path: &Path, bytes: &[u8]) {
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("cache"),
        std::process::id()
    ));
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(bytes)?;
        // The rename below only orders the directory entry, not the file's own
        // data, so without this a crash can leave a correctly named file whose
        // contents are still in the page cache.
        f.sync_all()
    });
    if written.is_err() || std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The release that last served data on this machine, if it is still a
/// well-formed name.
pub fn last_good_release() -> Option<String> {
    let raw = std::fs::read_to_string(cache_root().join(LAST_GOOD_RELEASE_FILE)).ok()?;
    let release = raw.trim().to_string();
    is_valid_release(&release).then_some(release)
}

/// Remember `release` as the one that last worked. Skipped when it is already
/// recorded, so a normal run does no write at all.
pub fn set_last_good_release(release: &str) {
    if !is_valid_release(release) || last_good_release().as_deref() == Some(release) {
        return;
    }
    write_atomic(
        &cache_root().join(LAST_GOOD_RELEASE_FILE),
        release.as_bytes(),
    );
}

/// Delete cached release directories older than `keep`.
///
/// Overture retires a release after 60 days, so without this the cache grows a
/// new directory every month and never loses one. Only names that parse as
/// releases are considered, and only ones that sort strictly older than the
/// release actually in use - so a run that pinned an older release cannot
/// delete the newer data another run is still using.
pub fn prune_releases_older_than(keep: &str) {
    if !is_valid_release(keep) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(cache_root()) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !is_valid_release(&name)
            || super::release_sort_key(&name) >= super::release_sort_key(keep)
        {
            continue;
        }
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_names_are_validated_before_becoming_path_segments() {
        assert!(is_valid_release("2026-08-19.0"));
        assert!(is_valid_release("2026-08-19.12"));

        // Anything that could escape the cache root, or is simply malformed.
        for bad in [
            "../etc",
            "2026-08-19",
            "2026-8-19.0",
            "2026-08-19.",
            "2026-08-19.x",
            "..",
            "",
            "release/2026-08-19.0",
            "2026-08-19.0/..",
        ] {
            assert!(!is_valid_release(bad), "{bad} must be rejected");
            assert!(release_dir(bad).is_none(), "{bad} must have no cache dir");
        }
    }

    #[test]
    fn a_write_that_fails_leaves_no_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        // A path whose parent is an existing *file* cannot be created.
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        write_atomic(&blocker.join("nested").join("f.bin"), b"data");

        // The blocker is untouched and no temp file was left in the directory.
        assert_eq!(std::fs::read(&blocker).unwrap(), b"x");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind");
    }

    #[test]
    fn an_atomic_write_round_trips_and_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("file.bin");

        write_atomic(&path, b"first");
        assert_eq!(read(&path).unwrap(), b"first");

        write_atomic(&path, b"second");
        assert_eq!(read(&path).unwrap(), b"second");

        // No temp files survive a successful write.
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn read_of_a_missing_file_is_none_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(&dir.path().join("absent")).is_none());
    }
}
