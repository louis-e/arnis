use crate::coordinate_system::geographic::LLBBox;
use crate::retrieve_data;
use fastnbt::Value;
use flate2::read::GzDecoder;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::{fs, io::Write};

/// Replaces `path` by writing a temporary beside it and renaming it into place.
///
/// `level.dat` is rewritten by several features, and a plain `fs::write`
/// truncates it first: a crash, a full disk or a pulled plug in that window
/// leaves a zero length or half written `level.dat`, which Minecraft cannot
/// open, and the world is gone. A rename either happens or does not.
pub fn replace_file_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    // A rename is only atomic within one filesystem, so the temporary has to be
    // a sibling; `with_extension` keeps it in the same directory by
    // construction, and a path with no directory at all has nowhere to put one.
    if path.parent().is_none() {
        return Err(format!("{} has no parent directory", path.display()));
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    if let Err(e) = fs::write(&tmp, bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("write {}: {e}", tmp.display()));
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("replace {}: {e}", path.display())
    })
}

/// Returns the Desktop directory for Bedrock .mcworld file output.
/// Falls back to home directory, then current directory.
pub fn get_bedrock_output_directory() -> PathBuf {
    dirs::desktop_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Returns Luanti's worlds directory for the current OS.
/// Windows: %APPDATA%\Minetest\worlds
/// macOS:   ~/Library/Application Support/minetest/worlds
/// Linux:   ~/.minetest/worlds
/// Falls back to Desktop/Arnis Luanti Worlds if no path can be resolved.
pub fn get_luanti_worlds_directory() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        dirs::data_dir().map(|p| p.join("Minetest"))
    } else if cfg!(target_os = "macos") {
        dirs::data_dir().map(|p| p.join("minetest"))
    } else {
        dirs::home_dir().map(|p| p.join(".minetest"))
    };

    base.map(|p| p.join("worlds")).unwrap_or_else(|| {
        dirs::desktop_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Arnis Luanti Worlds")
    })
}

/// Gets the area name for a given bounding box using the center point.
pub fn get_area_name_for_bedrock(bbox: &LLBBox) -> String {
    let center_lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
    let center_lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;

    match retrieve_data::fetch_area_name(center_lat, center_lon) {
        Ok(Some(name)) => name,
        _ => "Unknown Location".to_string(),
    }
}

/// Replaces characters that are invalid on Windows/macOS/Linux with `_`, trims
/// whitespace, and limits length to prevent excessively long filenames.
/// Unlike [`sanitize_for_filename`], an all-invalid/empty input is returned
/// as an empty string rather than masked behind a fallback label, so callers
/// that need to distinguish "nothing usable survived" from a real sanitized
/// value (e.g. a custom world name) can do so unambiguously.
fn sanitize_chars_and_trim(name: &str) -> String {
    sanitize_chars_and_trim_capped(name, MAX_FILENAME_BYTES)
}

/// Byte budget for a name derived from an area/place lookup. Kept well under
/// the 255-byte limit filesystems put on a single path component, because
/// these names get wrapped in longer strings (`Arnis {name}.mcworld`).
const MAX_FILENAME_BYTES: usize = 64;

/// Longest custom world name a user may set, counted in characters (not
/// bytes) so the limit means the same thing in every script. Mirrored by the
/// world-name editor's `maxlength` and character counter in the GUI, so what
/// the user is allowed to type is exactly what lands on disk.
pub const MAX_CUSTOM_WORLD_NAME_CHARS: usize = 48;

/// Byte budget for a custom world name. Sized so the character cap above is
/// always the binding limit - even at UTF-8's 4 bytes per character, plus
/// room for a `" (99)"` de-duplication suffix - while still leaving the
/// directory name inside the filesystem's 255-byte component limit.
const MAX_CUSTOM_WORLD_NAME_BYTES: usize = MAX_CUSTOM_WORLD_NAME_CHARS * 4 + 5;

/// Sanitizes a user-supplied world name, capping it by characters so a
/// non-Latin name isn't silently cut to a fraction of what was typed the way
/// a byte cap would (48 Japanese characters are 144 bytes).
fn sanitize_custom_world_name(name: &str) -> String {
    let capped: String = name.chars().take(MAX_CUSTOM_WORLD_NAME_CHARS).collect();
    sanitize_chars_and_trim_capped(&capped, MAX_CUSTOM_WORLD_NAME_BYTES)
}

/// Shared body of [`sanitize_chars_and_trim`], with the byte budget supplied
/// by the caller.
fn sanitize_chars_and_trim_capped(name: &str, max_bytes: usize) -> String {
    // Windows forbids directory/file names ending in '.' or ' ' (trailing
    // dots/spaces are silently stripped by the OS, which would make our
    // sanitized name mismatch the actual created directory). Strip any
    // trailing run of either so what we return is what actually lands on
    // disk on every platform.
    let strip_trailing_unsafe = |s: &str| -> String {
        s.trim_end_matches(|c: char| c == '.' || c.is_whitespace())
            .to_string()
    };

    let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let mut sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_control() || invalid_chars.contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    sanitized = strip_trailing_unsafe(sanitized.trim());

    // Limit length to avoid excessively long filenames
    if sanitized.len() > max_bytes {
        // Find a valid UTF-8 char boundary at or before max_bytes
        let cutoff = sanitized
            .char_indices()
            .take_while(|(idx, _)| *idx < max_bytes)
            .last()
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or(0);
        sanitized.truncate(cutoff);
        // Truncation can newly expose a trailing dot/space/whitespace run.
        sanitized = strip_trailing_unsafe(&sanitized);
    }

    sanitized
}

pub fn world_folder_name(raw: &str) -> Option<String> {
    Some(sanitize_custom_world_name(raw)).filter(|n| !n.is_empty())
}

/// Sanitizes an area name for safe use in filesystem paths.
/// Replaces characters that are invalid on Windows/macOS/Linux, trims whitespace,
/// and limits length to prevent excessively long filenames.
pub fn sanitize_for_filename(name: &str) -> String {
    let sanitized = sanitize_chars_and_trim(name);
    if sanitized.is_empty() {
        "Unknown Location".to_string()
    } else {
        sanitized
    }
}

/// Builds the Bedrock output path and level name for a given bounding box.
/// Combines area name lookup, sanitization, and path construction.
pub fn build_bedrock_output(bbox: &LLBBox, output_dir: PathBuf) -> (PathBuf, String) {
    let area_name = get_area_name_for_bedrock(bbox);
    let safe_name = sanitize_for_filename(&area_name);
    let filename = format!("Arnis {safe_name}.mcworld");
    let lvl_name = format!("Arnis World: {safe_name}");
    (output_dir.join(&filename), lvl_name)
}

/// Creates a new Java Edition world in the given base directory.
///
/// Generates a unique "Arnis World N" name, creates the directory structure
/// (with a `region/` subdirectory), writes the region template, level.dat
/// (with updated name, timestamp, and spawn position), and icon.png.
///
/// Returns the full path to the newly created world directory.
pub fn create_new_world(base_path: &Path) -> Result<String, String> {
    create_new_world_with_name(base_path, None)
}

/// Same as [`create_new_world`], but lets the caller request a specific world
/// name instead of the auto-generated "Arnis World N" scheme. `custom_name` is
/// sanitized for filesystem safety and de-duplicated against existing worlds
/// in `base_path` (appending " (2)", " (3)", ... on collision). A `None`,
/// empty/whitespace-only, or entirely-invalid custom name falls back to the
/// default "Arnis World N" scheme.
///
/// Returns the full path to the newly created world directory.
pub fn create_new_world_with_name(
    base_path: &Path,
    custom_name: Option<&str>,
) -> Result<String, String> {
    let unique_name: String = match custom_name.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw_name) => generate_unique_custom_world_name(base_path, raw_name),
        None => generate_unique_default_world_name(base_path),
    };

    let new_world_path: PathBuf = base_path.join(&unique_name);
    write_world_skeleton(&new_world_path, &unique_name, true)?;
    Ok(new_world_path.display().to_string())
}

/// Flat template chunks the skeleton writes as `region/r.0.0.mca`.
const REGION_TEMPLATE: &[u8] = include_bytes!("../assets/minecraft/region.template");

/// Deletes `region/r.0.0.mca` while it is still exactly the skeleton's template, so a void
/// world keeps no flat chunks where its area never reaches. A file anything has written to
/// is left alone.
pub fn remove_untouched_template_region(world_path: &Path) {
    let path = world_path.join("region").join("r.0.0.mca");
    let untouched = fs::metadata(&path).is_ok_and(|m| m.len() == REGION_TEMPLATE.len() as u64)
        && fs::read(&path).is_ok_and(|bytes| bytes == REGION_TEMPLATE);
    if untouched {
        let _ = fs::remove_file(&path);
    }
}

/// Writes `level.dat`, the icon and `region/` for a new Java world. One World
/// skips the region template, whose placeholder chunks it would not overwrite.
pub fn write_world_skeleton(
    new_world_path: &Path,
    level_name: &str,
    with_template_region: bool,
) -> Result<(), String> {
    let unique_name = level_name.to_string();

    // Create the new world directory structure
    fs::create_dir_all(new_world_path.join("region"))
        .map_err(|e| format!("Failed to create world directory: {e}"))?;

    // Copy the region template file
    if with_template_region {
        let region_path = new_world_path.join("region").join("r.0.0.mca");
        fs::write(&region_path, REGION_TEMPLATE)
            .map_err(|e| format!("Failed to create region file: {e}"))?;
    }

    // Add the level.dat file
    const LEVEL_TEMPLATE: &[u8] = include_bytes!("../assets/minecraft/level.dat");

    // Decompress the gzipped level.template
    let mut decoder = GzDecoder::new(LEVEL_TEMPLATE);
    let mut decompressed_data = Vec::new();
    decoder
        .read_to_end(&mut decompressed_data)
        .map_err(|e| format!("Failed to decompress level.template: {e}"))?;

    // Parse the decompressed NBT data
    let mut level_data: Value = fastnbt::from_bytes(&decompressed_data)
        .map_err(|e| format!("Failed to parse level.dat template: {e}"))?;

    // Modify the LevelName, LastPlayed and player position fields
    if let Value::Compound(ref mut root) = level_data {
        if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
            // Update LevelName
            data.insert("LevelName".to_string(), Value::String(unique_name.clone()));

            // Update LastPlayed to the current Unix time in milliseconds
            let current_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| format!("Failed to get current time: {e}"))?;
            let current_time_millis = current_time.as_millis() as i64;
            data.insert("LastPlayed".to_string(), Value::Long(current_time_millis));

            // Update player position and rotation
            if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
                if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
                    if pos.len() < 3 {
                        return Err(
                            "Invalid level.dat template: Player Pos list has fewer than 3 elements"
                                .to_string(),
                        );
                    }
                    if let Value::Double(ref mut x) = pos[0] {
                        *x = -5.0;
                    }
                    if let Value::Double(ref mut y) = pos[1] {
                        *y = -61.0;
                    }
                    if let Value::Double(ref mut z) = pos[2] {
                        *z = -5.0;
                    }
                }

                if let Some(Value::List(ref mut rot)) = player.get_mut("Rotation") {
                    if rot.is_empty() {
                        return Err(
                            "Invalid level.dat template: Player Rotation list is empty".to_string()
                        );
                    }
                    if let Value::Float(ref mut x) = rot[0] {
                        *x = -45.0;
                    }
                }
            }
        }
    }

    // Serialize the updated NBT data back to bytes
    let serialized_level_data: Vec<u8> = fastnbt::to_bytes(&level_data)
        .map_err(|e| format!("Failed to serialize updated level.dat: {e}"))?;

    // Compress the serialized data back to gzip
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized_level_data)
        .map_err(|e| format!("Failed to compress updated level.dat: {e}"))?;
    let compressed_level_data = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize compression for level.dat: {e}"))?;

    // Write the level.dat file
    fs::write(new_world_path.join("level.dat"), compressed_level_data)
        .map_err(|e| format!("Failed to create level.dat file: {e}"))?;

    // Add the icon.png file
    const ICON_TEMPLATE: &[u8] = include_bytes!("../assets/minecraft/icon.png");
    fs::write(new_world_path.join("icon.png"), ICON_TEMPLATE)
        .map_err(|e| format!("Failed to create icon.png file: {e}"))?;

    Ok(())
}

/// Holds Minecraft's `session.lock` while Arnis writes a world.
pub struct SessionLock {
    // On Unix it is only held: closing it releases the fcntl lock.
    #[cfg_attr(unix, allow(dead_code))]
    file: fs::File,
    path: PathBuf,
}

/// Locks this process holds. Probing one would close a handle to it, and on
/// Unix closing any handle drops the process's lock.
static HELD_LOCKS: std::sync::Mutex<Vec<LockId>> = std::sync::Mutex::new(Vec::new());

#[derive(Clone, PartialEq)]
struct LockId {
    path: PathBuf,
    #[cfg(unix)]
    inode: (u64, u64),
}

impl LockId {
    fn of(path: &Path) -> Self {
        LockId {
            path: path.to_path_buf(),
            #[cfg(unix)]
            inode: unix_inode(path).unwrap_or_default(),
        }
    }

    fn is_held(path: &Path) -> bool {
        #[cfg(unix)]
        let inode = unix_inode(path);
        HELD_LOCKS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|h| {
                #[cfg(unix)]
                if inode.is_some() && inode == Some(h.inode) {
                    return true;
                }
                h.path == path
            })
    }
}

#[cfg(unix)]
fn unix_inode(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

impl SessionLock {
    pub fn acquire(world_path: &Path) -> Result<Self, String> {
        let session_lock_path = world_path.join("session.lock");
        if LockId::is_held(&session_lock_path) {
            return Err("Failed to acquire lock on session.lock file: already held".to_string());
        }

        // Not truncated before the lock is ours: the holder's file stays intact.
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&session_lock_path)
            .map_err(|e| format!("Failed to create session.lock file: {e}"))?;

        // Java locks with fcntl. On Unix only that lock is taken: flock does not
        // see it on Linux, and on macOS the two would block each other.
        #[cfg(unix)]
        posix_lock(&file)
            .map_err(|e| format!("Failed to acquire lock on session.lock file: {e}"))?;
        #[cfg(not(unix))]
        fs2::FileExt::try_lock_exclusive(&file)
            .map_err(|e| format!("Failed to acquire lock on session.lock file: {e}"))?;

        file.set_len(0)
            .and_then(|_| (&file).write_all("\u{2603}".as_bytes()))
            .map_err(|e| format!("Failed to write to session.lock file: {e}"))?;

        HELD_LOCKS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(LockId::of(&session_lock_path));
        Ok(SessionLock {
            file,
            path: session_lock_path,
        })
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        #[cfg(not(unix))]
        let _ = fs2::FileExt::unlock(&self.file);
        HELD_LOCKS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|h| h.path != self.path);
    }
}

#[cfg(unix)]
fn posix_lock(file: &fs::File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: flock is plain data; zeroed is a valid whole-file request.
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = libc::F_WRLCK as _;
    fl.l_whence = libc::SEEK_SET as _;
    // SAFETY: the descriptor is open for the lifetime of `file`.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &fl) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn posix_lock_held_elsewhere(file: &fs::File) -> bool {
    use std::os::unix::io::AsRawFd;
    // SAFETY: as in `posix_lock`; F_GETLK only writes into `fl`.
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = libc::F_WRLCK as _;
    fl.l_whence = libc::SEEK_SET as _;
    let r = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut fl) };
    // F_UNLCK is an i32 on Linux and an i16 on macOS; l_type is always c_short.
    let unlocked: libc::c_short = libc::F_UNLCK as _;
    r == 0 && fl.l_type != unlocked
}

/// Whether another process holds the world's `session.lock`.
pub fn world_is_locked(world_path: &Path) -> bool {
    let path = world_path.join("session.lock");
    if !path.exists() || LockId::is_held(&path) {
        return false;
    }
    let file = match fs::OpenOptions::new().read(true).write(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            return e.kind() == std::io::ErrorKind::PermissionDenied || is_sharing_violation(&e)
        }
    };
    #[cfg(unix)]
    {
        posix_lock_held_elsewhere(&file)
    }
    #[cfg(not(unix))]
    {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => {
                let _ = fs2::FileExt::unlock(&file);
                false
            }
            Err(_) => true,
        }
    }
}

fn is_sharing_violation(e: &std::io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(32) | Some(33))
}

/// Generates a unique "Arnis World N" name.
/// Checks for both "Arnis World X" and "Arnis World X: Location" patterns.
fn generate_unique_default_world_name(base_path: &Path) -> String {
    let mut counter: i32 = 1;
    loop {
        let candidate_name: String = format!("Arnis World {counter}");
        let candidate_path: PathBuf = base_path.join(&candidate_name);

        // Check for exact match (no location suffix)
        let exact_match_exists = candidate_path.exists();

        // Check for worlds with location suffix (Arnis World X: Location)
        let location_pattern = format!("Arnis World {counter}: ");
        let location_match_exists = fs::read_dir(base_path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .any(|name| name.starts_with(&location_pattern))
            })
            .unwrap_or(false);

        if !exact_match_exists && !location_match_exists {
            return candidate_name;
        }
        counter += 1;
    }
}

/// Builds a unique world name from a user-supplied custom name: sanitizes it
/// for filesystem safety, then de-duplicates against existing worlds in
/// `base_path` by appending " (2)", " (3)", etc. Falls back to the default
/// "Arnis World N" scheme if nothing usable survives sanitization (e.g. the
/// input was only invalid characters).
fn generate_unique_custom_world_name(base_path: &Path, raw_name: &str) -> String {
    let sanitized = sanitize_custom_world_name(raw_name);

    // Nothing usable survived sanitization (e.g. the input was only invalid
    // characters), so fall back to the default naming scheme instead of
    // creating a world named after an empty string.
    if sanitized.is_empty() {
        return generate_unique_default_world_name(base_path);
    }

    if !base_path.join(&sanitized).exists() {
        return sanitized;
    }

    let mut counter: i32 = 2;
    loop {
        let candidate = format!("{sanitized} ({counter})");
        if !base_path.join(&candidate).exists() {
            return candidate;
        }
        counter += 1;
    }
}

/// Name of the bundled Java datapack that extends the Overworld build height.
pub const TALL_DATAPACK_NAME: &str = "arnis_tall";

/// Install the bundled tall-world datapack into a Java world and register it
/// in `level.dat`'s `Data.DataPacks.Enabled` so it auto-activates on first
/// load. The base `data/` tree uses the legacy flat dimension_type schema
/// (formats below 90, so up to 1.21.10); overlays carry the attributes schema
/// for formats 90-100 and the clock/`default_clock` schema for 101 and up,
/// since the schema is mutually incompatible across those eras.
/// Overlays must declare their range only via
/// `min_format`/`max_format`; the deprecated `formats` key makes 1.21.9+ drop
/// the whole overlays section and fall back to the legacy tree.
pub fn install_tall_datapack(world_path: &Path) -> Result<(), String> {
    const PACK_MCMETA: &[u8] = include_bytes!("../assets/minecraft/datapack_tall/pack.mcmeta");
    const OVERWORLD_JSON: &[u8] = include_bytes!(
        "../assets/minecraft/datapack_tall/data/minecraft/dimension_type/overworld.json"
    );
    const OVERLAY_ATTRIBUTES_JSON: &[u8] = include_bytes!(
        "../assets/minecraft/datapack_tall/overlay_attributes/data/minecraft/dimension_type/overworld.json"
    );
    const OVERLAY_2601_JSON: &[u8] = include_bytes!(
        "../assets/minecraft/datapack_tall/overlay_2601/data/minecraft/dimension_type/overworld.json"
    );

    let dp_root = world_path.join("datapacks").join(TALL_DATAPACK_NAME);

    // (overlay directory, embedded bytes); empty directory = base data/ tree.
    let dim_files: [(&str, &[u8]); 3] = [
        ("", OVERWORLD_JSON),
        ("overlay_attributes", OVERLAY_ATTRIBUTES_JSON),
        ("overlay_2601", OVERLAY_2601_JSON),
    ];
    for (overlay, bytes) in dim_files {
        let mut dim_dir = dp_root.clone();
        if !overlay.is_empty() {
            dim_dir.push(overlay);
        }
        let dim_dir = dim_dir
            .join("data")
            .join("minecraft")
            .join("dimension_type");
        fs::create_dir_all(&dim_dir)
            .map_err(|e| format!("Failed to create datapack directories: {e}"))?;
        fs::write(dim_dir.join("overworld.json"), bytes)
            .map_err(|e| format!("Failed to write overworld.json: {e}"))?;
    }

    fs::write(dp_root.join("pack.mcmeta"), PACK_MCMETA)
        .map_err(|e| format!("Failed to write pack.mcmeta: {e}"))?;

    enable_datapack_in_level_dat(world_path, TALL_DATAPACK_NAME)?;

    Ok(())
}

/// Appends `file/<pack_dir_name>` to `Data.DataPacks.Enabled` if missing, so the
/// folder pack in `<world>/datapacks/<pack_dir_name>` loads when the world opens.
/// Expected to run on a fresh level.dat template whose Enabled list starts with
/// `["vanilla"]`, so the appended entry naturally lands after vanilla and the
/// pack's overrides win.
pub fn enable_datapack_in_level_dat(world_path: &Path, pack_dir_name: &str) -> Result<(), String> {
    let level_path = world_path.join("level.dat");
    if !level_path.exists() {
        return Err(format!("level.dat not found at {level_path:?}"));
    }

    let raw = fs::read(&level_path).map_err(|e| format!("Failed to read level.dat: {e}"))?;
    let mut decoder = GzDecoder::new(raw.as_slice());
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| format!("Failed to decompress level.dat: {e}"))?;

    let mut root: Value = fastnbt::from_bytes(&decompressed)
        .map_err(|e| format!("Failed to parse level.dat NBT: {e}"))?;

    let entry = format!("file/{pack_dir_name}");

    {
        let data = match root {
            Value::Compound(ref mut r) => match r.get_mut("Data") {
                Some(Value::Compound(ref mut d)) => d,
                _ => return Err("level.dat missing Data compound".to_string()),
            },
            _ => return Err("level.dat root is not a compound".to_string()),
        };

        let data_packs = data
            .entry("DataPacks".to_string())
            .or_insert_with(|| Value::Compound(Default::default()));
        let Value::Compound(ref mut dp) = data_packs else {
            return Err("level.dat Data.DataPacks is not a compound".to_string());
        };

        let enabled = dp
            .entry("Enabled".to_string())
            .or_insert_with(|| Value::List(Vec::new()));
        let Value::List(ref mut list) = enabled else {
            return Err("level.dat Data.DataPacks.Enabled is not a list".to_string());
        };

        let already_enabled = list
            .iter()
            .any(|v| matches!(v, Value::String(s) if s == &entry));
        if !already_enabled {
            list.push(Value::String(entry));
        }
    }

    let serialized =
        fastnbt::to_bytes(&root).map_err(|e| format!("Failed to serialize level.dat: {e}"))?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized)
        .map_err(|e| format!("Failed to compress level.dat: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize level.dat compression: {e}"))?;
    replace_file_atomically(&level_path, &compressed)
        .map_err(|e| format!("Failed to write level.dat: {e}"))?;

    Ok(())
}

/// Turns the overworld's flat generator into the vanilla "The Void" preset: one layer of air
/// over the void biome, without the start platform, lakes or structures, so everything the
/// area does not cover stays empty. No-op when the overworld is not a flat generator.
fn make_void_generator(root: &mut Value) {
    let Value::Compound(root_map) = root else {
        return;
    };
    let Some(Value::Compound(data)) = root_map.get_mut("Data") else {
        return;
    };
    let Some(Value::Compound(settings)) = data.get_mut("WorldGenSettings") else {
        return;
    };
    let Some(Value::Compound(dimensions)) = settings.get_mut("dimensions") else {
        return;
    };
    let Some(Value::Compound(overworld)) = dimensions.get_mut("minecraft:overworld") else {
        return;
    };
    let Some(Value::Compound(generator)) = overworld.get_mut("generator") else {
        return;
    };
    if !matches!(generator.get("type"), Some(Value::String(t)) if t == "minecraft:flat") {
        return;
    }
    let Some(Value::Compound(flat)) = generator.get_mut("settings") else {
        return;
    };
    let mut air = std::collections::HashMap::new();
    air.insert(
        "block".to_string(),
        Value::String("minecraft:air".to_string()),
    );
    air.insert("height".to_string(), Value::Int(1));
    flat.insert(
        "layers".to_string(),
        Value::List(vec![Value::Compound(air)]),
    );
    flat.insert(
        "biome".to_string(),
        Value::String("minecraft:the_void".to_string()),
    );
    // The void biome's one feature is the stone start platform at the origin.
    flat.insert("features".to_string(), Value::Byte(0));
    flat.insert("lakes".to_string(), Value::Byte(0));
    flat.insert("structure_overrides".to_string(), Value::List(Vec::new()));
}

/// Lifts the superflat generator plane to `base_y` by prepending an air layer.
/// Flat layers stack up from the dimension floor, so with the tall datapack's -2032 floor the
/// terrain outside the written regions would sit up to ~2000 blocks above the generated plane.
/// No-op when the plane already lands on `base_y` (a vanilla floor with an unlifted base) and
/// on worlds whose overworld is not a flat generator.
fn raise_superflat_floor(root: &mut Value, base_y: i32, min_y: i32) {
    let air_height = base_y - min_y - 2;
    if air_height <= 0 {
        return;
    }

    let Value::Compound(root_map) = root else {
        return;
    };
    let Some(Value::Compound(data)) = root_map.get_mut("Data") else {
        return;
    };
    let Some(Value::Compound(settings)) = data.get_mut("WorldGenSettings") else {
        return;
    };
    let Some(Value::Compound(dimensions)) = settings.get_mut("dimensions") else {
        return;
    };
    let Some(Value::Compound(overworld)) = dimensions.get_mut("minecraft:overworld") else {
        return;
    };
    let Some(Value::Compound(generator)) = overworld.get_mut("generator") else {
        return;
    };
    if !matches!(generator.get("type"), Some(Value::String(t)) if t == "minecraft:flat") {
        return;
    }
    let Some(Value::Compound(flat)) = generator.get_mut("settings") else {
        return;
    };
    let Some(Value::List(layers)) = flat.get_mut("layers") else {
        return;
    };

    match layers.first_mut() {
        Some(Value::Compound(bottom)) if matches!(bottom.get("block"), Some(Value::String(b)) if b == "minecraft:air") =>
        {
            bottom.insert("height".to_string(), Value::Int(air_height));
        }
        _ => {
            let mut air = std::collections::HashMap::new();
            air.insert(
                "block".to_string(),
                Value::String("minecraft:air".to_string()),
            );
            air.insert("height".to_string(), Value::Int(air_height));
            layers.insert(0, Value::Compound(air));
        }
    }
}

/// Sets `LastPlayed` to now, which lists the world first in Minecraft.
pub fn touch_last_played(world_path: &Path) -> Result<(), String> {
    let level_path = world_path.join("level.dat");
    let raw = fs::read(&level_path).map_err(|e| format!("Failed to read level.dat: {e}"))?;
    let mut decompressed = Vec::new();
    GzDecoder::new(raw.as_slice())
        .read_to_end(&mut decompressed)
        .map_err(|e| format!("Failed to decompress level.dat: {e}"))?;
    let mut root: Value = fastnbt::from_bytes(&decompressed)
        .map_err(|e| format!("Failed to parse level.dat NBT: {e}"))?;
    let Value::Compound(ref mut top) = root else {
        return Err("level.dat root is not a compound".to_string());
    };
    let Some(Value::Compound(data)) = top.get_mut("Data") else {
        return Err("level.dat missing Data compound".to_string());
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("Failed to read the clock: {e}"))?
        .as_millis() as i64;
    data.insert("LastPlayed".to_string(), Value::Long(now_ms));

    let serialized =
        fastnbt::to_bytes(&root).map_err(|e| format!("Failed to serialize level.dat: {e}"))?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized)
        .map_err(|e| format!("Failed to compress level.dat: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Failed to compress level.dat: {e}"))?;
    replace_file_atomically(&level_path, &compressed)
}

/// DataVersion of 26.1, which keeps dimensions under `dimensions/` and maps under
/// `data/minecraft/maps/`. The game moves an older world there when it first opens it.
pub const DIMENSION_FOLDERS_DATA_VERSION: i32 = 4772;

/// Where a Java world keeps its overworld chunks and its maps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorldLayout {
    /// Before 26.1, and every world Arnis creates.
    #[default]
    Legacy,
    /// 26.1 and later.
    Dimensions,
}

impl WorldLayout {
    /// The layout `level.dat` declares. An unreadable one counts as legacy.
    pub fn of(world: &Path) -> Self {
        match level_data_version(world) {
            Some(v) if v >= DIMENSION_FOLDERS_DATA_VERSION => WorldLayout::Dimensions,
            _ => WorldLayout::Legacy,
        }
    }

    /// Folder holding the overworld's `region/`, `entities/` and `poi/`.
    pub fn overworld_dir(self, world: &Path) -> PathBuf {
        match self {
            WorldLayout::Legacy => world.to_path_buf(),
            WorldLayout::Dimensions => world.join("dimensions").join("minecraft").join("overworld"),
        }
    }

    /// Folder holding the map files and the map id counter.
    pub fn maps_dir(self, world: &Path) -> PathBuf {
        match self {
            WorldLayout::Legacy => world.join("data"),
            WorldLayout::Dimensions => world.join("data").join("minecraft").join("maps"),
        }
    }
}

fn read_gzip_nbt(path: &Path) -> Result<Value, String> {
    let raw = fs::read(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    let mut decompressed = Vec::new();
    GzDecoder::new(raw.as_slice())
        .read_to_end(&mut decompressed)
        .map_err(|e| format!("Failed to decompress {}: {e}", path.display()))?;
    fastnbt::from_bytes(&decompressed)
        .map_err(|e| format!("Failed to parse {}: {e}", path.display()))
}

fn write_gzip_nbt(path: &Path, value: &Value) -> Result<(), String> {
    let serialized = fastnbt::to_bytes(value)
        .map_err(|e| format!("Failed to serialize {}: {e}", path.display()))?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized)
        .map_err(|e| format!("Failed to compress {}: {e}", path.display()))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Failed to compress {}: {e}", path.display()))?;
    replace_file_atomically(path, &compressed)
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

/// `Data.DataVersion` of a world's `level.dat`.
pub(crate) fn level_data_version(world: &Path) -> Option<i32> {
    let Ok(Value::Compound(root)) = read_gzip_nbt(&world.join("level.dat")) else {
        return None;
    };
    match root.get("Data") {
        Some(Value::Compound(data)) => match data.get("DataVersion") {
            Some(Value::Int(v)) => Some(*v),
            _ => None,
        },
        _ => None,
    }
}

/// The singleplayer player's file in a 26.1+ world, named by `Data.singleplayer_uuid`.
fn singleplayer_file(
    world: &Path,
    data: &std::collections::HashMap<String, Value>,
) -> Option<PathBuf> {
    let Some(Value::IntArray(uuid)) = data.get("singleplayer_uuid") else {
        return None;
    };
    if uuid.len() != 4 {
        return None;
    }
    let hex: String = uuid.iter().map(|w| format!("{:08x}", *w as u32)).collect();
    let name = format!(
        "{}-{}-{}-{}-{}.dat",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    );
    Some(world.join("players").join("data").join(name))
}

// Writes GameType, DayTime, the player's game mode and what the game generates around the
// area into an existing level.dat.
pub fn apply_java_world_settings(
    world_path: &Path,
    game_mode: crate::args::GameMode,
    world_time: i64,
    world_type: crate::args::WorldType,
) -> Result<(), String> {
    let level_path = world_path.join("level.dat");
    if !level_path.exists() {
        return Err(format!("level.dat not found at {level_path:?}"));
    }

    let raw = fs::read(&level_path).map_err(|e| format!("Failed to read level.dat: {e}"))?;
    let mut decoder = GzDecoder::new(raw.as_slice());
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| format!("Failed to decompress level.dat: {e}"))?;

    let mut root: Value = fastnbt::from_bytes(&decompressed)
        .map_err(|e| format!("Failed to parse level.dat NBT: {e}"))?;

    {
        let data = match root {
            Value::Compound(ref mut r) => match r.get_mut("Data") {
                Some(Value::Compound(ref mut d)) => d,
                _ => return Err("level.dat missing Data compound".to_string()),
            },
            _ => return Err("level.dat root is not a compound".to_string()),
        };

        let game_type = game_mode.java_game_type();
        data.insert("GameType".to_string(), Value::Int(game_type));
        data.insert("DayTime".to_string(), Value::Long(world_time));
        if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
            player.insert("playerGameType".to_string(), Value::Int(game_type));
        }
    }

    // Folded into this rewrite rather than a second read/write: both need the post-generation
    // base, which is only known once the terrain has been scaled.
    match world_type {
        crate::args::WorldType::Void => make_void_generator(&mut root),
        crate::args::WorldType::Flat => raise_superflat_floor(
            &mut root,
            crate::world_editor::base_chunk_y(),
            crate::world_editor::min_y(),
        ),
    }

    let serialized =
        fastnbt::to_bytes(&root).map_err(|e| format!("Failed to serialize level.dat: {e}"))?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized)
        .map_err(|e| format!("Failed to compress level.dat: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize level.dat compression: {e}"))?;
    replace_file_atomically(&level_path, &compressed)
        .map_err(|e| format!("Failed to write level.dat: {e}"))?;

    Ok(())
}

/// Sets the player spawn point in an existing Java Edition level.dat file.
///
/// Updates both the world spawn point (SpawnX/SpawnY/SpawnZ) and the player
/// position if a Player compound exists. Callers derive `spawn_y` from the
/// generated terrain so the player spawns above ground even in extended-height
/// worlds where terrain may reach Y≈2000.
pub fn set_spawn_in_level_dat(
    world_path: &Path,
    spawn_x: i32,
    spawn_y: i32,
    spawn_z: i32,
) -> Result<(), String> {
    let level_path = world_path.join("level.dat");
    if !level_path.exists() {
        return Err(format!("level.dat not found at {level_path:?}"));
    }

    // Read and decompress
    let level_data = fs::read(&level_path).map_err(|e| format!("Failed to read level.dat: {e}"))?;

    let mut decoder = GzDecoder::new(level_data.as_slice());
    let mut decompressed_data = Vec::new();
    decoder
        .read_to_end(&mut decompressed_data)
        .map_err(|e| format!("Failed to decompress level.dat: {e}"))?;

    let mut nbt_data: Value = fastnbt::from_bytes(&decompressed_data)
        .map_err(|e| format!("Failed to parse level.dat NBT data: {e}"))?;

    // Update spawn point
    let data = match nbt_data {
        Value::Compound(ref mut root) => match root.get_mut("Data") {
            Some(Value::Compound(ref mut data)) => data,
            _ => {
                return Err(
                    "Invalid level.dat structure: missing or non-compound \"Data\" section"
                        .to_string(),
                );
            }
        },
        _ => {
            return Err(
                "Invalid level.dat structure: root NBT value is not a compound".to_string(),
            );
        }
    };

    // 1.21.9+ keeps the spawn in a `spawn` compound.
    if let Some(Value::Compound(spawn)) = data.get_mut("spawn") {
        spawn.insert(
            "pos".to_string(),
            Value::IntArray(fastnbt::IntArray::new(vec![spawn_x, spawn_y, spawn_z])),
        );
        spawn.insert(
            "dimension".to_string(),
            Value::String("minecraft:overworld".to_string()),
        );
    } else {
        data.insert("SpawnX".to_string(), Value::Int(spawn_x));
        data.insert("SpawnY".to_string(), Value::Int(spawn_y));
        data.insert("SpawnZ".to_string(), Value::Int(spawn_z));
    }

    // 26.1+ keeps the player in a file of their own.
    let player_file = if data.contains_key("Player") {
        None
    } else {
        singleplayer_file(world_path, data)
    };
    if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
        move_player(player, spawn_x, spawn_y, spawn_z);
    }

    // Serialize, compress, and write back
    let serialized_data = fastnbt::to_bytes(&nbt_data)
        .map_err(|e| format!("Failed to serialize updated level.dat: {e}"))?;

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized_data)
        .map_err(|e| format!("Failed to compress updated level.dat: {e}"))?;
    let compressed_data = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize compression for level.dat: {e}"))?;

    replace_file_atomically(&level_path, &compressed_data)
        .map_err(|e| format!("Failed to write updated level.dat: {e}"))?;

    if let Some(path) = player_file.filter(|p| p.is_file()) {
        let mut player = read_gzip_nbt(&path)?;
        if let Value::Compound(ref mut player) = player {
            move_player(player, spawn_x, spawn_y, spawn_z);
        }
        write_gzip_nbt(&path, &player)?;
    }

    Ok(())
}

/// Puts a player at the spawn, in the overworld wherever they logged out.
fn move_player(
    player: &mut std::collections::HashMap<String, Value>,
    spawn_x: i32,
    spawn_y: i32,
    spawn_z: i32,
) {
    if player.contains_key("Dimension") {
        player.insert(
            "Dimension".to_string(),
            Value::String("minecraft:overworld".to_string()),
        );
    }
    if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
        for (slot, v) in pos.iter_mut().zip([spawn_x, spawn_y, spawn_z]) {
            if let Value::Double(ref mut p) = slot {
                *p = v as f64;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn a_spawn_move_follows_a_world_minecraft_has_upgraded() {
        let dir = tempfile::tempdir().unwrap();
        let world = PathBuf::from(create_new_world(dir.path()).unwrap());
        assert_eq!(WorldLayout::of(&world), WorldLayout::Legacy);

        // What 26.3 leaves: the spawn in a compound and the player in a file of their own.
        let mut level = read_gzip_nbt(&world.join("level.dat")).unwrap();
        let Value::Compound(ref mut root) = level else {
            panic!("level root");
        };
        let Some(Value::Compound(data)) = root.get_mut("Data") else {
            panic!("level data");
        };
        for key in ["Player", "SpawnX", "SpawnY", "SpawnZ"] {
            data.remove(key);
        }
        data.insert("DataVersion".to_string(), Value::Int(5023));
        let spawn = HashMap::from([
            (
                "pos".to_string(),
                Value::IntArray(fastnbt::IntArray::new(vec![0, 0, 0])),
            ),
            (
                "dimension".to_string(),
                Value::String("minecraft:overworld".to_string()),
            ),
        ]);
        data.insert("spawn".to_string(), Value::Compound(spawn));
        let uuid = vec![-993932354, 1062356475, -1621421405, -1681526930];
        data.insert(
            "singleplayer_uuid".to_string(),
            Value::IntArray(fastnbt::IntArray::new(uuid)),
        );
        write_gzip_nbt(&world.join("level.dat"), &level).unwrap();
        let player_path = world.join("players/data/c4c1cbbe-3f52-45fb-9f5b-12a39bc5ef6e.dat");
        fs::create_dir_all(player_path.parent().unwrap()).unwrap();
        let player = HashMap::from([
            (
                "Pos".to_string(),
                Value::List(vec![
                    Value::Double(1.0),
                    Value::Double(2.0),
                    Value::Double(3.0),
                ]),
            ),
            (
                "Dimension".to_string(),
                Value::String("minecraft:the_nether".to_string()),
            ),
        ]);
        write_gzip_nbt(&player_path, &Value::Compound(player)).unwrap();
        assert_eq!(WorldLayout::of(&world), WorldLayout::Dimensions);
        assert_eq!(
            WorldLayout::Dimensions.overworld_dir(&world),
            world.join("dimensions/minecraft/overworld")
        );

        set_spawn_in_level_dat(&world, 100, 64, -200).unwrap();

        let Value::Compound(root) = read_gzip_nbt(&world.join("level.dat")).unwrap() else {
            panic!("level root");
        };
        let Some(Value::Compound(data)) = root.get("Data") else {
            panic!("level data");
        };
        let Some(Value::Compound(spawn)) = data.get("spawn") else {
            panic!("spawn");
        };
        let Some(Value::IntArray(pos)) = spawn.get("pos") else {
            panic!("spawn pos");
        };
        assert_eq!(&pos[..], &[100, 64, -200]);
        assert!(!data.contains_key("SpawnX"));

        let Value::Compound(player) = read_gzip_nbt(&player_path).unwrap() else {
            panic!("player");
        };
        assert_eq!(
            player.get("Pos"),
            Some(&Value::List(vec![
                Value::Double(100.0),
                Value::Double(64.0),
                Value::Double(-200.0)
            ]))
        );
        assert_eq!(
            player.get("Dimension"),
            Some(&Value::String("minecraft:overworld".to_string()))
        );
    }

    fn level_name(world: &Path) -> String {
        let raw = fs::read(world.join("level.dat")).unwrap();
        let mut decompressed = Vec::new();
        GzDecoder::new(raw.as_slice())
            .read_to_end(&mut decompressed)
            .unwrap();
        let root: Value = fastnbt::from_bytes(&decompressed).unwrap();
        let Value::Compound(root) = root else {
            panic!("root not a compound");
        };
        let Some(Value::Compound(data)) = root.get("Data") else {
            panic!("missing Data");
        };
        let Some(Value::String(name)) = data.get("LevelName") else {
            panic!("missing LevelName");
        };
        name.clone()
    }

    #[test]
    fn create_new_world_with_name_uses_custom_name() {
        let tmp = tempfile::tempdir().unwrap();
        let world =
            PathBuf::from(create_new_world_with_name(tmp.path(), Some("My Cool World")).unwrap());
        assert_eq!(world.file_name().unwrap(), "My Cool World");
        assert_eq!(level_name(&world), "My Cool World");
    }

    #[test]
    fn create_new_world_with_name_dedupes_on_collision() {
        let tmp = tempfile::tempdir().unwrap();
        let first =
            PathBuf::from(create_new_world_with_name(tmp.path(), Some("Metropolis")).unwrap());
        let second =
            PathBuf::from(create_new_world_with_name(tmp.path(), Some("Metropolis")).unwrap());
        let third =
            PathBuf::from(create_new_world_with_name(tmp.path(), Some("Metropolis")).unwrap());
        assert_eq!(first.file_name().unwrap(), "Metropolis");
        assert_eq!(second.file_name().unwrap(), "Metropolis (2)");
        assert_eq!(third.file_name().unwrap(), "Metropolis (3)");
    }

    #[test]
    fn create_new_world_with_name_sanitizes_invalid_characters() {
        let tmp = tempfile::tempdir().unwrap();
        let world =
            PathBuf::from(create_new_world_with_name(tmp.path(), Some("My:World/Name?")).unwrap());
        assert_eq!(world.file_name().unwrap(), "My_World_Name_");
    }

    #[test]
    fn create_new_world_with_name_falls_back_when_blank() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(create_new_world_with_name(tmp.path(), Some("   ")).unwrap());
        assert_eq!(world.file_name().unwrap(), "Arnis World 1");
    }

    #[test]
    fn create_new_world_with_name_accepts_literal_unknown_location() {
        // Regression guard: must not be confused with sanitize_for_filename's
        // internal fallback label for a genuinely empty/invalid name.
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(
            create_new_world_with_name(tmp.path(), Some("Unknown Location")).unwrap(),
        );
        assert_eq!(world.file_name().unwrap(), "Unknown Location");
    }

    #[test]
    fn create_new_world_with_name_none_uses_default_scheme() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(create_new_world_with_name(tmp.path(), None).unwrap());
        assert_eq!(world.file_name().unwrap(), "Arnis World 1");
    }

    #[test]
    fn sanitize_for_filename_strips_trailing_dots() {
        // Windows silently drops trailing dots/spaces from directory names,
        // so a sanitized name must not end in one or the created directory
        // would end up named differently than what we return.
        assert_eq!(sanitize_for_filename("My World..."), "My World");
        assert_eq!(sanitize_for_filename("My World. "), "My World");
        assert_eq!(sanitize_for_filename("Trailing.dot."), "Trailing.dot");
    }

    #[test]
    fn a_full_length_custom_name_survives_in_any_script() {
        // The editor caps input at MAX_CUSTOM_WORLD_NAME_CHARS characters, so
        // a name of exactly that many characters must come back untouched -
        // in every script, not just Latin-1. A byte-based cap would quietly
        // cut a Japanese or Cyrillic name to a third of what was typed.
        let tmp = tempfile::tempdir().unwrap();
        for name in [
            "a".repeat(MAX_CUSTOM_WORLD_NAME_CHARS),
            "あ".repeat(MAX_CUSTOM_WORLD_NAME_CHARS),
            "Мир"
                .chars()
                .cycle()
                .take(MAX_CUSTOM_WORLD_NAME_CHARS)
                .collect(),
        ] {
            let world = PathBuf::from(create_new_world_with_name(tmp.path(), Some(&name)).unwrap());
            assert_eq!(
                world.file_name().unwrap().to_str().unwrap().chars().count(),
                MAX_CUSTOM_WORLD_NAME_CHARS,
                "{name} was truncated"
            );
            assert_eq!(world.file_name().unwrap(), name.as_str());
        }
    }

    #[test]
    fn area_names_keep_the_tighter_byte_budget() {
        // Area names are wrapped in longer strings ("Arnis {name}.mcworld"),
        // so they stay on the 64-byte budget, while a custom world name the
        // user typed is capped by characters. Same input, deliberately
        // different results - roughly 22 characters is all a 64-byte budget
        // buys in a 3-bytes-per-character script (the cut keeps the character
        // that starts inside the budget, so it can spill a couple of bytes
        // past it), and that is what custom names used to be cut down to.
        let long = "あ".repeat(MAX_CUSTOM_WORLD_NAME_CHARS);
        assert_eq!(sanitize_for_filename(&long).chars().count(), 22);
        assert_eq!(
            sanitize_custom_world_name(&long).chars().count(),
            MAX_CUSTOM_WORLD_NAME_CHARS
        );
    }

    #[test]
    fn an_over_length_custom_name_is_capped_by_characters() {
        let tmp = tempfile::tempdir().unwrap();
        let long = "あ".repeat(MAX_CUSTOM_WORLD_NAME_CHARS + 20);
        let world = PathBuf::from(create_new_world_with_name(tmp.path(), Some(&long)).unwrap());
        assert_eq!(
            world.file_name().unwrap(),
            "あ".repeat(MAX_CUSTOM_WORLD_NAME_CHARS).as_str()
        );
    }

    #[test]
    fn only_an_untouched_template_region_is_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(create_new_world(tmp.path()).unwrap());
        let region = world.join("region").join("r.0.0.mca");
        assert!(region.is_file());
        remove_untouched_template_region(&world);
        assert!(!region.exists(), "the template goes");

        let mut written = REGION_TEMPLATE.to_vec();
        written[8192] ^= 1;
        fs::write(&region, &written).unwrap();
        remove_untouched_template_region(&world);
        assert!(region.exists(), "a region something wrote to stays");
    }

    #[test]
    fn apply_java_world_settings_writes_gametype_and_daytime() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(create_new_world(tmp.path()).unwrap());
        apply_java_world_settings(
            &world,
            crate::args::GameMode::Survival,
            13000,
            crate::args::WorldType::Void,
        )
        .unwrap();

        let raw = fs::read(world.join("level.dat")).unwrap();
        let mut decompressed = Vec::new();
        GzDecoder::new(raw.as_slice())
            .read_to_end(&mut decompressed)
            .unwrap();
        let root: Value = fastnbt::from_bytes(&decompressed).unwrap();
        let Value::Compound(root) = root else {
            panic!("root not a compound");
        };
        let Some(Value::Compound(data)) = root.get("Data") else {
            panic!("missing Data");
        };
        assert_eq!(data.get("GameType"), Some(&Value::Int(0)));
        assert_eq!(data.get("DayTime"), Some(&Value::Long(13000)));
        if let Some(Value::Compound(player)) = data.get("Player") {
            assert_eq!(player.get("playerGameType"), Some(&Value::Int(0)));
        }
    }

    fn flat_layers(root: &Value) -> Vec<(String, i32)> {
        let mut node = root;
        for key in [
            "Data",
            "WorldGenSettings",
            "dimensions",
            "minecraft:overworld",
            "generator",
            "settings",
            "layers",
        ] {
            let Value::Compound(map) = node else {
                panic!("{key} parent not a compound");
            };
            node = map.get(key).unwrap_or_else(|| panic!("missing {key}"));
        }
        let Value::List(layers) = node else {
            panic!("layers not a list");
        };
        layers
            .iter()
            .map(|l| {
                let Value::Compound(l) = l else {
                    panic!("layer not a compound");
                };
                match (l.get("block"), l.get("height")) {
                    (Some(Value::String(b)), Some(Value::Int(h))) => (b.clone(), *h),
                    _ => panic!("malformed layer"),
                }
            })
            .collect()
    }

    #[test]
    fn superflat_floor_follows_the_extended_world_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(create_new_world(tmp.path()).unwrap());
        let raw = fs::read(world.join("level.dat")).unwrap();
        let mut decompressed = Vec::new();
        GzDecoder::new(raw.as_slice())
            .read_to_end(&mut decompressed)
            .unwrap();
        let mut root: Value = fastnbt::from_bytes(&decompressed).unwrap();
        let vanilla = flat_layers(&root);

        raise_superflat_floor(&mut root, -62, crate::world_editor::DEFAULT_MIN_Y);
        assert_eq!(flat_layers(&root), vanilla);

        raise_superflat_floor(&mut root, -1876, -2032);
        let layers = flat_layers(&root);
        assert_eq!(layers[0], ("minecraft:air".to_string(), 154));
        assert_eq!(layers[1..], vanilla[..]);
        // grass lands exactly on the terrain base
        assert_eq!(-2032 + layers.iter().map(|l| l.1).sum::<i32>() - 1, -1876);

        raise_superflat_floor(&mut root, -1876, -2032);
        assert_eq!(flat_layers(&root), layers);
    }

    fn level_dat_root(world: &Path) -> Value {
        let raw = fs::read(world.join("level.dat")).unwrap();
        let mut decompressed = Vec::new();
        GzDecoder::new(raw.as_slice())
            .read_to_end(&mut decompressed)
            .unwrap();
        fastnbt::from_bytes(&decompressed).unwrap()
    }

    #[test]
    fn applying_world_settings_lifts_the_superflat_plane_to_the_terrain_base() {
        let _g = crate::world_editor::FLOOR_TEST_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();

        let vanilla = PathBuf::from(create_new_world(tmp.path()).unwrap());
        crate::world_editor::set_world_bounds(
            crate::world_editor::DEFAULT_MIN_Y,
            crate::world_editor::DEFAULT_MAX_Y,
        );
        crate::world_editor::set_base_chunk_y(-62);
        apply_java_world_settings(
            &vanilla,
            crate::args::GameMode::Creative,
            6000,
            crate::args::WorldType::Flat,
        )
        .unwrap();
        let vanilla_layers = flat_layers(&level_dat_root(&vanilla));

        let tall = PathBuf::from(create_new_world(tmp.path()).unwrap());
        crate::world_editor::set_world_bounds(-2032, 2031);
        crate::world_editor::set_base_chunk_y(-1876);
        apply_java_world_settings(
            &tall,
            crate::args::GameMode::Creative,
            6000,
            crate::args::WorldType::Flat,
        )
        .unwrap();
        let tall_layers = flat_layers(&level_dat_root(&tall));

        crate::world_editor::set_world_bounds(
            crate::world_editor::DEFAULT_MIN_Y,
            crate::world_editor::DEFAULT_MAX_Y,
        );
        crate::world_editor::set_base_chunk_y(-62);

        assert_eq!(vanilla_layers[0].0, "minecraft:dirt");
        assert_eq!(tall_layers[0], ("minecraft:air".to_string(), 154));
        assert_eq!(tall_layers[1..], vanilla_layers[..]);
    }

    #[test]
    fn a_void_world_gets_the_void_preset_whatever_the_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(create_new_world(tmp.path()).unwrap());
        apply_java_world_settings(
            &world,
            crate::args::GameMode::Creative,
            6000,
            crate::args::WorldType::Void,
        )
        .unwrap();
        let root = level_dat_root(&world);
        assert_eq!(
            flat_layers(&root),
            vec![("minecraft:air".to_string(), 1)],
            "nothing but air past the area"
        );

        let mut node = &root;
        for key in [
            "Data",
            "WorldGenSettings",
            "dimensions",
            "minecraft:overworld",
            "generator",
            "settings",
        ] {
            let Value::Compound(map) = node else {
                panic!("{key} parent not a compound");
            };
            node = map.get(key).unwrap();
        }
        let Value::Compound(settings) = node else {
            panic!("settings not a compound");
        };
        assert_eq!(
            settings.get("biome"),
            Some(&Value::String("minecraft:the_void".to_string()))
        );
        assert_eq!(settings.get("features"), Some(&Value::Byte(0)));
        assert_eq!(
            settings.get("structure_overrides"),
            Some(&Value::List(vec![]))
        );
    }

    /// Highest format that still allows the deprecated `formats` key.
    const LAST_PRE_MINOR_DATA_FORMAT: u64 = 81;

    fn install_pack_for_test(tmp: &std::path::Path) -> PathBuf {
        let world = PathBuf::from(create_new_world(tmp).unwrap());
        install_tall_datapack(&world).unwrap();
        world.join("datapacks").join(TALL_DATAPACK_NAME)
    }

    fn read_json(path: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Picks the overlay covering `format`, or the base `data/` tree if none does.
    fn resolve_overworld(dp_root: &Path, format: u64) -> serde_json::Value {
        let mcmeta = read_json(&dp_root.join("pack.mcmeta"));
        let dir = mcmeta["overlays"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| {
                let min = e["min_format"][0].as_u64().unwrap();
                let max = e["max_format"][0].as_u64().unwrap();
                (min..=max).contains(&format)
            })
            .map(|e| e["directory"].as_str().unwrap().to_string())
            .unwrap_or_default();

        let mut path = dp_root.to_path_buf();
        if !dir.is_empty() {
            path.push(dir);
        }
        read_json(&path.join("data/minecraft/dimension_type/overworld.json"))
    }

    #[test]
    fn tall_datapack_overlays_omit_deprecated_formats_key() {
        let tmp = tempfile::tempdir().unwrap();
        let dp_root = install_pack_for_test(tmp.path());
        let mcmeta = read_json(&dp_root.join("pack.mcmeta"));

        let entries = mcmeta["overlays"]["entries"].as_array().unwrap();
        assert!(!entries.is_empty());

        let lowest = entries
            .iter()
            .map(|e| e["min_format"][0].as_u64().unwrap())
            .min()
            .unwrap();
        assert!(
            lowest > LAST_PRE_MINOR_DATA_FORMAT,
            "an overlay reaching <= {LAST_PRE_MINOR_DATA_FORMAT} would make `formats` mandatory again"
        );

        for entry in entries {
            let dir = entry["directory"].as_str().unwrap();
            assert!(
                entry.get("formats").is_none(),
                "overlay {dir} carries the deprecated `formats` key, 1.21.9+ drops all overlays"
            );
            assert!(
                entry.get("min_format").is_some(),
                "overlay {dir} lacks min_format"
            );
            assert!(
                entry.get("max_format").is_some(),
                "overlay {dir} lacks max_format"
            );
            assert!(
                dp_root.join(dir).is_dir(),
                "overlay dir {dir} was not installed"
            );
        }

        // Root declares support down to 61, so it has to keep the old keys.
        assert_eq!(mcmeta["pack"]["min_format"][0].as_u64().unwrap(), 61);
        assert!(mcmeta["pack"]["pack_format"].is_number());
        assert!(mcmeta["pack"]["supported_formats"].is_object());
    }

    #[test]
    fn tall_datapack_resolves_correct_schema_per_pack_format() {
        let tmp = tempfile::tempdir().unwrap();
        let dp_root = install_pack_for_test(tmp.path());

        // (data pack format, Minecraft version)
        for (format, label) in [(61, "1.21.4"), (88, "1.21.10")] {
            let dim = resolve_overworld(&dp_root, format);
            assert!(
                dim["natural"].is_boolean(),
                "{label}: legacy schema needs `natural`"
            );
            assert!(
                dim["bed_works"].is_boolean(),
                "{label}: legacy schema needs `bed_works`"
            );
        }

        // Without `timelines` the day/night cycle freezes on 1.21.11.
        let dim = resolve_overworld(&dp_root, 94);
        assert_eq!(dim["timelines"], "#minecraft:in_overworld");
        assert!(dim["attributes"].is_object());

        // 26.1 drives `/time` through `default_clock` and requires `has_ender_dragon_fight`.
        for (format, label) in [(101, "26.1"), (107, "26.2")] {
            let dim = resolve_overworld(&dp_root, format);
            assert_eq!(dim["default_clock"], "minecraft:overworld", "{label}");
            assert_eq!(dim["timelines"], "#minecraft:in_overworld", "{label}");
            assert!(
                dim["has_ender_dragon_fight"].is_boolean(),
                "{label}: required field missing, dimension_type won't parse"
            );
        }

        // Every era still needs the extended build height.
        for format in [61, 88, 94, 101, 107, 999] {
            let dim = resolve_overworld(&dp_root, format);
            assert_eq!(dim["min_y"], -2032, "format {format}");
            assert_eq!(dim["height"], 4064, "format {format}");
            assert_eq!(dim["logical_height"], 4064, "format {format}");
        }
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn a_world_without_a_lock_file_is_free() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!world_is_locked(dir.path()));
    }

    #[test]
    fn a_session_lock_excludes_a_second_one_and_goes_away_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let lock = SessionLock::acquire(dir.path()).unwrap();
        assert!(dir.path().join("session.lock").is_file());
        assert!(!world_is_locked(dir.path()));
        assert!(SessionLock::acquire(dir.path()).is_err());
        drop(lock);
        assert!(!dir.path().join("session.lock").exists());
        assert!(!world_is_locked(dir.path()));
    }

    #[cfg(windows)]
    #[test]
    fn a_lock_held_through_another_handle_is_seen() {
        use fs2::FileExt;
        let dir = tempfile::tempdir().unwrap();
        let file = fs::File::create(dir.path().join("session.lock")).unwrap();
        file.try_lock_exclusive().unwrap();
        assert!(world_is_locked(dir.path()));
        file.unlock().unwrap();
        assert!(!world_is_locked(dir.path()));
    }

    /// Another process holds the lock, the way Minecraft does. The test binary
    /// runs itself as that process.
    #[test]
    fn a_lock_held_by_another_process_is_seen_and_kept() {
        const HOLD: &str = "ARNIS_TEST_HOLD_SESSION_LOCK";
        const TEST: &str =
            "world_utils::lock_tests::a_lock_held_by_another_process_is_seen_and_kept";
        if let Ok(dir) = std::env::var(HOLD) {
            let dir = PathBuf::from(dir);
            let _lock = SessionLock::acquire(&dir).unwrap();
            fs::write(dir.join("ready"), b"").unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while !dir.join("done").exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([TEST, "--exact", "--test-threads=1"])
            .env(HOLD, dir.path())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !dir.path().join("ready").exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the holder never started"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let seen = world_is_locked(dir.path());
        let taken = SessionLock::acquire(dir.path()).is_ok();
        // Windows also blocks reads of a locked range; the in-process test covers it there.
        #[cfg(unix)]
        let content = fs::read(dir.path().join("session.lock")).unwrap_or_default();
        fs::write(dir.path().join("done"), b"").unwrap();
        assert!(child.wait().unwrap().success());

        assert!(seen, "a lock held by another process must be reported");
        assert!(!taken, "a lock held by another process must not be taken");
        #[cfg(unix)]
        assert_eq!(
            content,
            "\u{2603}".as_bytes(),
            "the holder's file must stay as it was"
        );
        assert!(!world_is_locked(dir.path()));
    }

    #[test]
    fn world_folder_names_are_sanitized() {
        assert_eq!(world_folder_name("  My City  ").as_deref(), Some("My City"));
        assert_eq!(world_folder_name("a/b:c").as_deref(), Some("a_b_c"));
        assert_eq!(world_folder_name(".."), None);
        assert_eq!(world_folder_name("   "), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_held_lock_file_is_left_as_it_was() {
        use fs2::FileExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.lock");
        fs::write(&path, "held").unwrap();
        let holder = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        holder.try_lock_exclusive().unwrap();
        assert!(SessionLock::acquire(dir.path()).is_err());
        holder.unlock().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "held");
    }

    #[test]
    fn a_stale_lock_file_nobody_holds_is_free() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("session.lock"), "\u{2603}").unwrap();
        assert!(!world_is_locked(dir.path()));
        let lock = SessionLock::acquire(dir.path()).unwrap();
        drop(lock);
    }

    #[test]
    fn last_played_is_moved_to_now() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(create_new_world(tmp.path()).unwrap());
        let read = || {
            let raw = fs::read(world.join("level.dat")).unwrap();
            let mut plain = Vec::new();
            GzDecoder::new(raw.as_slice())
                .read_to_end(&mut plain)
                .unwrap();
            let Value::Compound(root) = fastnbt::from_bytes::<Value>(&plain).unwrap() else {
                panic!("root not a compound");
            };
            let Some(Value::Compound(data)) = root.get("Data") else {
                panic!("no Data");
            };
            match data.get("LastPlayed") {
                Some(Value::Long(t)) => *t,
                other => panic!("LastPlayed is {other:?}"),
            }
        };
        let before = read();
        std::thread::sleep(std::time::Duration::from_millis(5));
        touch_last_played(&world).unwrap();
        assert!(read() > before);
    }

    #[test]
    fn the_skeleton_is_a_complete_world() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("Named World");
        write_world_skeleton(&world, "Named World", true).unwrap();
        assert!(world.join("level.dat").is_file());
        assert!(world.join("icon.png").is_file());
        assert!(world.join("region").join("r.0.0.mca").is_file());

        let bare = dir.path().join("One");
        write_world_skeleton(&bare, "One", false).unwrap();
        assert!(bare.join("level.dat").is_file());
        assert!(bare.join("region").is_dir());
        assert!(!bare.join("region").join("r.0.0.mca").exists());
    }
}
