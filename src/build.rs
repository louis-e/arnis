use std::process::Command;

fn main() {
    emit_build_hash();

    #[cfg(feature = "gui")]
    tauri_build::build()
}

/// Exposes the commit the binary was built from as `ARNIS_BUILD_HASH`, so a
/// custom build is distinguishable from an official release. Falls back to
/// "unknown" when git is unavailable, e.g. building from a source tarball.
fn emit_build_hash() {
    let hash = match git(&["rev-parse", "--short=7", "HEAD"]) {
        Some(hash) => {
            // Untracked files are ignored so stray local files don't mark an
            // otherwise pristine checkout as modified.
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
                .is_some_and(|status| !status.is_empty());
            if dirty {
                format!("{hash}-dirty")
            } else {
                hash
            }
        }
        None => "unknown".to_string(),
    };

    println!("cargo:rustc-env=ARNIS_BUILD_HASH={hash}");

    // Rebuild when the checked-out commit or the staged tree changes.
    for path in [".git/HEAD", ".git/index"] {
        if std::path::Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
}
