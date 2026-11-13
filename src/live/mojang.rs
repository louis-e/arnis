//! The server jar and Java runtime for a version, from Mojang's launcher metadata.

use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const VERSION_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
// The launcher's own fixed index; the launcher's runtime and the system Java cover its loss.
const RUNTIME_INDEX: &str = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
/// The oldest version an Arnis world opens in.
pub const OLDEST_VERSION: &str = "1.21.4";

/// Reports a step and how far it is; returning false cancels it.
pub type Progress<'a> = &'a (dyn Fn(&str, Option<f64>) -> bool + Sync);

const CANCELLED: &str = "Cancelled";

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(900))
        .user_agent(concat!("arnis/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

/// The game folder: the parent of the saves folder when that is one, else the default.
pub fn game_dir(saves_dir: &Path) -> Option<PathBuf> {
    if let Some(parent) = saves_dir.parent() {
        if parent.join("launcher_profiles.json").is_file() || parent.join("versions").is_dir() {
            return Some(parent.to_path_buf());
        }
    }
    default_game_dir().filter(|d| d.is_dir())
}

fn default_game_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(".minecraft"))
    } else if cfg!(target_os = "macos") {
        dirs::home_dir().map(|h| h.join("Library/Application Support/minecraft"))
    } else {
        dirs::home_dir().map(|h| {
            let flatpak = h.join(".var/app/com.mojang.Minecraft/.minecraft");
            if flatpak.is_dir() {
                flatpak
            } else {
                h.join(".minecraft")
            }
        })
    }
}

pub struct Manifest {
    json: Value,
}

impl Manifest {
    /// Mojang's version list, refreshed hourly; a stale copy serves when offline.
    pub fn load(cache_dir: &Path) -> Result<Self, String> {
        let path = cache_dir.join("version_manifest_v2.json");
        let fresh = fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(3600));
        if !fresh {
            match client().and_then(|c| get_text(&c, VERSION_MANIFEST)) {
                Ok(text) => {
                    let _ = fs::create_dir_all(cache_dir);
                    let _ = crate::world_utils::replace_file_atomically(&path, text.as_bytes());
                }
                Err(e) if !path.is_file() => {
                    return Err(format!("Could not reach Mojang's version list: {e}"))
                }
                Err(_) => {}
            }
        }
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let json: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        Ok(Self { json })
    }

    fn versions(&self) -> &[Value] {
        self.json["versions"]
            .as_array()
            .map_or(&[], |v| v.as_slice())
    }

    fn entry(&self, id: &str) -> Option<&Value> {
        self.versions().iter().find(|v| v["id"] == id)
    }

    pub fn latest_release(&self) -> Option<String> {
        self.json["latest"]["release"].as_str().map(str::to_string)
    }

    /// Releases from the newest down to the oldest Arnis supports.
    pub fn supported_releases(&self) -> Vec<String> {
        let mut out = Vec::new();
        for v in self.versions() {
            let Some(id) = v["id"].as_str() else { continue };
            if v["type"] == "release" {
                out.push(id.to_string());
            }
            if id == OLDEST_VERSION {
                break;
            }
        }
        out
    }

    /// Whether `id` is a version Arnis worlds open in, snapshots included.
    pub fn is_supported(&self, id: &str) -> bool {
        let oldest = self
            .entry(OLDEST_VERSION)
            .and_then(|v| v["releaseTime"].as_str());
        let this = self.entry(id).and_then(|v| v["releaseTime"].as_str());
        matches!((this, oldest), (Some(t), Some(o)) if t >= o)
    }

    /// When `id` came out, for ordering versions.
    pub fn release_time(&self, id: &str) -> Option<&str> {
        self.entry(id).and_then(|v| v["releaseTime"].as_str())
    }

    /// Where the server jar and Java runtime for `id` come from.
    pub fn version_info(&self, id: &str, cache_dir: &Path) -> Result<VersionInfo, String> {
        let entry = self
            .entry(id)
            .ok_or_else(|| format!("Minecraft {id} is not in Mojang's version list"))?;
        let url = entry["url"].as_str().ok_or("version entry without url")?;
        let path = cache_dir.join("versions").join(id).join("version.json");
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => {
                let text = get_text(&client()?, url)?;
                if let Some(parent) = path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = crate::world_utils::replace_file_atomically(&path, text.as_bytes());
                text
            }
        };
        let json: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let server = &json["downloads"]["server"];
        Ok(VersionInfo {
            id: id.to_string(),
            server_url: server["url"]
                .as_str()
                .ok_or_else(|| format!("Mojang offers no server for {id}"))?
                .to_string(),
            server_sha1: server["sha1"].as_str().unwrap_or_default().to_string(),
            server_size: server["size"].as_u64().unwrap_or(0),
            java_component: json["javaVersion"]["component"]
                .as_str()
                .unwrap_or("java-runtime-delta")
                .to_string(),
            java_major: json["javaVersion"]["majorVersion"].as_u64().unwrap_or(21) as u32,
        })
    }
}

#[derive(Clone, Debug)]
pub struct VersionInfo {
    pub id: String,
    pub server_url: String,
    pub server_sha1: String,
    pub server_size: u64,
    pub java_component: String,
    pub java_major: u32,
}

/// The version the launcher started last, through a modded profile to the game it runs.
pub fn last_played(game_dir: &Path, manifest: &Manifest) -> Option<String> {
    // The Microsoft Store launcher keeps its profiles in a file of its own.
    let files: Vec<Value> = [
        "launcher_profiles.json",
        "launcher_profiles_microsoft_store.json",
    ]
    .iter()
    .filter_map(|f| fs::read_to_string(game_dir.join(f)).ok())
    .filter_map(|t| serde_json::from_str(&t).ok())
    .collect();
    let latest = files
        .iter()
        .filter_map(|json| json["profiles"].as_object())
        .flat_map(|profiles| profiles.values())
        .filter(|p| p["lastVersionId"].is_string())
        .max_by(|a, b| {
            let a = a["lastUsed"].as_str().unwrap_or("");
            let b = b["lastUsed"].as_str().unwrap_or("");
            a.cmp(b)
        })?;
    let mut id = latest["lastVersionId"].as_str()?.to_string();
    match id.as_str() {
        "latest-release" => return manifest.latest_release(),
        "latest-snapshot" => {
            return manifest.json["latest"]["snapshot"]
                .as_str()
                .map(str::to_string)
        }
        _ => {}
    }
    for _ in 0..4 {
        if manifest.entry(&id).is_some() {
            return Some(id);
        }
        let file = game_dir
            .join("versions")
            .join(&id)
            .join(format!("{id}.json"));
        let json: Value = serde_json::from_str(&fs::read_to_string(file).ok()?).ok()?;
        id = json["inheritsFrom"].as_str()?.to_string();
    }
    None
}

/// The server jar for `info`, downloaded and checked once.
pub fn server_jar(
    info: &VersionInfo,
    cache_dir: &Path,
    progress: Progress,
) -> Result<PathBuf, String> {
    let path = cache_dir.join("versions").join(&info.id).join("server.jar");
    if path.is_file() && (info.server_sha1.is_empty() || sha1_of(&path)? == info.server_sha1) {
        return Ok(path);
    }
    let label = format!("Downloading the Minecraft {} server", info.id);
    if !progress(&label, Some(0.0)) {
        return Err(CANCELLED.to_string());
    }
    let bytes = download(&client()?, &info.server_url, info.server_size, &|f| {
        progress(&label, Some(f))
    })?;
    if !info.server_sha1.is_empty() && sha1_hex(&bytes) != info.server_sha1 {
        return Err(
            "The downloaded server did not match Mojang's checksum. Try again.".to_string(),
        );
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::world_utils::replace_file_atomically(&path, &bytes)?;
    Ok(path)
}

fn platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("mac-os-arm64"),
        ("macos", _) => Some("mac-os"),
        ("windows", "aarch64") => Some("windows-arm64"),
        ("windows", "x86") => Some("windows-x86"),
        ("windows", _) => Some("windows-x64"),
        ("linux", "x86") => Some("linux-i386"),
        ("linux", "x86_64") => Some("linux"),
        _ => None,
    }
}

fn java_in(root: &Path) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    [
        root.join("bin").join(exe),
        root.join("jre.bundle/Contents/Home/bin").join(exe),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// The launcher's copy of the runtime, else one Arnis downloaded earlier.
pub fn find_java(game_dir: Option<&Path>, component: &str, cache_dir: &Path) -> Option<PathBuf> {
    let platform = platform()?;
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(g) = game_dir {
        roots.push(g.join("runtime"));
    }
    // The Microsoft Store launcher, then the older installer's.
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(
            PathBuf::from(local)
                .join("Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime"),
        );
    }
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(dir) = std::env::var_os(var) {
            roots.push(PathBuf::from(dir).join("Minecraft Launcher/runtime"));
        }
    }
    roots
        .iter()
        .find_map(|r| java_in(&r.join(component).join(platform).join(component)))
        .or_else(|| java_in(&cache_dir.join("runtime").join(component)))
}

/// Downloads the runtime the launcher would use for `component`.
pub fn download_java(
    component: &str,
    cache_dir: &Path,
    progress: Progress,
) -> Result<PathBuf, String> {
    let platform = platform().ok_or("No Java runtime from Mojang for this computer")?;
    let client = client()?;
    let index: Value =
        serde_json::from_str(&get_text(&client, RUNTIME_INDEX)?).map_err(|e| e.to_string())?;
    let manifest_url = index[platform][component][0]["manifest"]["url"]
        .as_str()
        .ok_or_else(|| format!("Mojang has no {component} runtime for {platform}"))?;
    let files: Value =
        serde_json::from_str(&get_text(&client, manifest_url)?).map_err(|e| e.to_string())?;
    let files = files["files"]
        .as_object()
        .ok_or("runtime manifest without files")?;

    let root = cache_dir.join("runtime").join(component);
    let staging = cache_dir
        .join("runtime")
        .join(format!("{component}.partial"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;

    let wanted: Vec<(&String, &Value)> =
        files.iter().filter(|(_, f)| f["type"] == "file").collect();
    let total = wanted.len().max(1);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let label = "Downloading Java for the server";
    if !progress(label, Some(0.0)) {
        return Err(CANCELLED.to_string());
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .map_err(|e| e.to_string())?;
    let results: Vec<Result<(), String>> = pool.install(|| {
        use rayon::prelude::*;
        wanted
            .par_iter()
            .map(|(rel, f)| {
                let dest = safe_join(&staging, rel)?;
                let raw = &f["downloads"]["raw"];
                let url = raw["url"].as_str().ok_or("runtime file without url")?;
                let n = done.load(std::sync::atomic::Ordering::Relaxed);
                if !progress(label, Some(n as f64 / total as f64)) {
                    return Err(CANCELLED.to_string());
                }
                let bytes = download(&client, url, raw["size"].as_u64().unwrap_or(0), &|_| true)?;
                if let Some(sha) = raw["sha1"].as_str() {
                    if sha1_hex(&bytes) != sha {
                        return Err(format!("checksum mismatch for {rel}"));
                    }
                }
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
                #[cfg(unix)]
                if f["executable"].as_bool() == Some(true) {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o755));
                }
                done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            })
            .collect()
    });
    results.into_iter().collect::<Result<Vec<_>, _>>()?;
    #[cfg(unix)]
    for (rel, f) in files.iter().filter(|(_, f)| f["type"] == "link") {
        let Some(target) = f["target"].as_str().filter(|t| link_stays_inside(rel, t)) else {
            continue;
        };
        if let Ok(dest) = safe_join(&staging, rel) {
            if let Some(parent) = dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = std::os::unix::fs::symlink(target, dest);
        }
    }
    let _ = fs::remove_dir_all(&root);
    fs::rename(&staging, &root).map_err(|e| e.to_string())?;
    java_in(&root).ok_or_else(|| "The downloaded Java runtime has no java executable".to_string())
}

/// Whether a link at `rel` pointing at `target` resolves inside the runtime folder.
#[cfg(any(unix, test))]
fn link_stays_inside(rel: &str, target: &str) -> bool {
    use std::path::Component;
    if Path::new(target).is_absolute() {
        return false;
    }
    let mut depth = Path::new(rel).components().count() as i32 - 1;
    for c in Path::new(target).components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

/// A Java on this computer, for where Mojang ships no runtime: `JAVA_HOME`, then `PATH`.
pub fn system_java(min_major: u32) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        candidates.push(PathBuf::from(home).join("bin").join(exe));
    }
    candidates.push(PathBuf::from(exe));
    candidates.into_iter().find(|java| {
        let mut cmd = std::process::Command::new(java);
        cmd.arg("-version");
        super::server::detach(&mut cmd);
        let Ok(out) = cmd.output() else {
            return false;
        };
        java_major(&String::from_utf8_lossy(&out.stderr)).is_some_and(|m| m >= min_major)
    })
}

/// The major version in `java -version` output: `"21.0.2"` is 21, `"1.8.0_402"` is 8.
fn java_major(text: &str) -> Option<u32> {
    let quoted = text.split('"').nth(1)?;
    let mut parts = quoted.split(['.', '-', '_', '+']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// `rel` under `root`, refusing paths that climb out of it.
fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute()
        || rel_path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!("unsafe path in runtime manifest: {rel}"));
    }
    Ok(root.join(rel_path))
}

/// A player's face, with the hat layer over it, as a PNG data URL. None for a name without a
/// Mojang account or when Mojang cannot be reached.
pub fn player_head(name: &str) -> Option<String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let valid = (1..=16).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return None;
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("arnis/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let json = |url: &str| -> Option<serde_json::Value> {
        serde_json::from_str(&get_text(&client, url).ok()?).ok()
    };
    let id = [
        format!("https://api.mojang.com/users/profiles/minecraft/{name}"),
        format!("https://api.minecraftservices.com/minecraft/profile/lookup/name/{name}"),
    ]
    .iter()
    .find_map(|url| Some(json(url)?.get("id")?.as_str()?.to_string()))?;
    let session = json(&format!(
        "https://sessionserver.mojang.com/session/minecraft/profile/{id}"
    ))?;
    let value = session
        .get("properties")?
        .as_array()?
        .iter()
        .find(|p| p.get("name").and_then(|n| n.as_str()) == Some("textures"))?
        .get("value")?
        .as_str()?;
    let textures: serde_json::Value = serde_json::from_slice(&b64.decode(value).ok()?).ok()?;
    let url = textures.pointer("/textures/SKIN/url")?.as_str()?;
    let path = url
        .strip_prefix("http://textures.minecraft.net/")
        .or_else(|| url.strip_prefix("https://textures.minecraft.net/"))?;
    let png = client
        .get(format!("https://textures.minecraft.net/{path}"))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .ok()?;
    Some(format!(
        "data:image/png;base64,{}",
        b64.encode(skin_head(&png)?)
    ))
}

/// The 8×8 face of a skin with its hat layer on top, as PNG bytes.
fn skin_head(png: &[u8]) -> Option<Vec<u8>> {
    let skin = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    if skin.width() < 64 || skin.height() < 32 {
        return None;
    }
    let mut face = image::RgbaImage::new(8, 8);
    for y in 0..8 {
        for x in 0..8 {
            let hat = *skin.get_pixel(40 + x, 8 + y);
            let mut px = if hat[3] >= 128 {
                hat
            } else {
                *skin.get_pixel(8 + x, 8 + y)
            };
            px[3] = 255;
            face.put_pixel(x, y, px);
        }
    }
    let mut out = Vec::new();
    face.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

fn get_text(client: &reqwest::blocking::Client, url: &str) -> Result<String, String> {
    client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .map_err(|e| e.to_string())
}

fn download(
    client: &reqwest::blocking::Client,
    url: &str,
    size_hint: u64,
    on_progress: &dyn Fn(f64) -> bool,
) -> Result<Vec<u8>, String> {
    let mut resp = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    let total = resp.content_length().unwrap_or(size_hint).max(1);
    let mut out = Vec::with_capacity(total as usize);
    let mut buf = [0u8; 64 * 1024];
    let mut last = 0.0;
    if !on_progress(0.0) {
        return Err(CANCELLED.to_string());
    }
    loop {
        let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        let f = (out.len() as f64 / total as f64).min(1.0);
        if f - last >= 0.01 {
            last = f;
            if !on_progress(f) {
                return Err(CANCELLED.to_string());
            }
        }
    }
    Ok(out)
}

fn sha1_hex(bytes: &[u8]) -> String {
    sha1_smol::Sha1::from(bytes).digest().to_string()
}

fn sha1_of(path: &Path) -> Result<String, String> {
    fs::read(path)
        .map(|b| sha1_hex(&b))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_head_is_the_face_with_the_hat_over_it() {
        let mut skin = image::RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 0]));
        for y in 8..16 {
            for x in 8..16 {
                skin.put_pixel(x, y, image::Rgba([200, 150, 100, 255]));
            }
        }
        skin.put_pixel(41, 9, image::Rgba([10, 20, 30, 255]));
        let mut png = Vec::new();
        skin.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let head = image::load_from_memory(&skin_head(&png).unwrap())
            .unwrap()
            .to_rgba8();
        assert_eq!(head.dimensions(), (8, 8));
        assert_eq!(*head.get_pixel(0, 0), image::Rgba([200, 150, 100, 255]));
        assert_eq!(*head.get_pixel(1, 1), image::Rgba([10, 20, 30, 255]));
        assert!(skin_head(b"not a png").is_none());
    }

    fn manifest(json: Value) -> Manifest {
        Manifest { json }
    }

    fn sample() -> Manifest {
        manifest(serde_json::json!({
            "latest": { "release": "26.3", "snapshot": "26.4-snapshot-1" },
            "versions": [
                { "id": "26.4-snapshot-1", "type": "snapshot", "releaseTime": "2026-10-01T00:00:00+00:00" },
                { "id": "26.3", "type": "release", "releaseTime": "2026-09-01T00:00:00+00:00" },
                { "id": "1.21.5", "type": "release", "releaseTime": "2025-03-25T00:00:00+00:00" },
                { "id": "1.21.4", "type": "release", "releaseTime": "2024-12-03T00:00:00+00:00" },
                { "id": "1.21.3", "type": "release", "releaseTime": "2024-10-23T00:00:00+00:00" }
            ]
        }))
    }

    #[test]
    fn releases_stop_at_the_oldest_supported() {
        assert_eq!(
            sample().supported_releases(),
            vec!["26.3", "1.21.5", "1.21.4"]
        );
        assert!(sample().is_supported("26.4-snapshot-1"));
        assert!(!sample().is_supported("1.21.3"));
    }

    #[test]
    fn the_last_played_profile_resolves_through_mods() {
        let dir = tempfile::tempdir().unwrap();
        let fabric = "fabric-loader-0.20.1-26.3";
        fs::create_dir_all(dir.path().join("versions").join(fabric)).unwrap();
        fs::write(
            dir.path()
                .join("versions")
                .join(fabric)
                .join(format!("{fabric}.json")),
            r#"{"inheritsFrom":"26.3"}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("launcher_profiles.json"),
            format!(
                r#"{{"profiles":{{"a":{{"lastVersionId":"latest-release","lastUsed":"2026-01-01T00:00:00Z"}},
                    "b":{{"lastVersionId":"{fabric}","lastUsed":"2026-11-01T00:00:00Z"}}}}}}"#
            ),
        )
        .unwrap();
        assert_eq!(last_played(dir.path(), &sample()).as_deref(), Some("26.3"));
    }

    #[test]
    fn runtime_paths_cannot_escape() {
        let root = Path::new("/tmp/rt");
        assert!(safe_join(root, "bin/java").is_ok());
        assert!(safe_join(root, "../evil").is_err());
        assert!(safe_join(root, "/etc/passwd").is_err());
        assert!(link_stays_inside(
            "legal/java.base/LICENSE",
            "../../conf/LICENSE"
        ));
        assert!(!link_stays_inside("bin/java", "../../../usr/bin/evil"));
        assert!(!link_stays_inside("bin/java", "/usr/bin/evil"));
    }

    #[test]
    fn java_versions_are_read_from_both_schemes() {
        assert_eq!(
            java_major("openjdk version \"25.0.1\" 2025-10-21 LTS"),
            Some(25)
        );
        assert_eq!(java_major("java version \"1.8.0_402\""), Some(8));
        assert_eq!(java_major("no version here"), None);
    }
}
