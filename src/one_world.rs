//! One World: one persistent Java world that every generation extends.
//!
//! The world pins a Web Mercator frame (origin and scale in the manifest next
//! to `level.dat`). Every requested area is snapped outward to whole chunks in
//! that frame and written into the existing region files, so areas generated
//! at different times line up block for block. See `docs/one_world.md`.

use crate::args::Args;
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::ElevationAffine;
use crate::projection::{snap_bbox_to_chunks, WebMercatorProjection};
use crate::world_utils::{world_is_locked, SessionLock};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "arnis_one_world.json";
pub const PREVIEW_DIR: &str = "arnis_one_world/previews";
pub const DEFAULT_WORLD_NAME: &str = "Arnis One World";
/// 2: merges write into empty region files. Version 1 worlds still hold
/// region template chunks outside their areas and are repaired once.
pub const MANIFEST_VERSION: u32 = 2;

/// Geometry kept past the area edge, so an element straddling a seam is
/// built whole on both sides.
pub const CLIP_PAD_BLOCKS: i32 = 64;

const MAX_ABS_LAT: f64 = 85.0;

/// Ground data is fetched this far past the area and cropped again, so the
/// smoothing passes (widest: built-up Gaussian, ~90 m) agree across seams.
pub fn ground_pad_blocks(scale: f64) -> i32 {
    ((100.0 * scale).ceil() as i32).max(96)
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GeneratedArea {
    pub id: u32,
    /// Unix seconds.
    pub generated_at: u64,
    pub arnis_version: String,
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    pub max_lon: f64,
    pub preview: Option<String>,
}

impl GeneratedArea {
    fn is_inside(&self, other: &XZBBox) -> bool {
        other.min_x() <= self.min_x
            && other.min_z() <= self.min_z
            && other.max_x() >= self.max_x
            && other.max_z() >= self.max_z
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Manifest {
    pub version: u32,
    pub created_with: String,
    /// Unix seconds.
    pub created_at: u64,
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub scale: f64,
    pub ground_level: i32,
    pub terrain: bool,
    pub disable_height_limit: bool,
    pub aws_only_elevation: bool,
    /// Metre to Y mapping shared by every area, set by the first terrain area.
    pub elevation: Option<ElevationAffine>,
    pub next_area_id: u32,
    pub areas: Vec<GeneratedArea>,
}

impl Manifest {
    fn new(args: &Args, origin_lat: f64, origin_lon: f64) -> Self {
        Self {
            version: MANIFEST_VERSION,
            created_with: format!("arnis {}", env!("CARGO_PKG_VERSION")),
            created_at: unix_now(),
            origin_lat,
            origin_lon,
            scale: args.scale,
            ground_level: args.ground_level,
            terrain: args.terrain(),
            disable_height_limit: args.disable_height_limit,
            aws_only_elevation: args.aws_only_elevation,
            elevation: None,
            next_area_id: 1,
            areas: Vec::new(),
        }
    }

    pub fn projection(&self) -> WebMercatorProjection {
        WebMercatorProjection::new(self.origin_lat, self.origin_lon, self.scale)
    }

    pub fn path_in(world_dir: &Path) -> PathBuf {
        world_dir.join(MANIFEST_FILE)
    }

    pub fn load(world_dir: &Path) -> Result<Option<Self>, String> {
        let path = Self::path_in(world_dir);
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        let manifest: Manifest = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not a valid One World manifest: {e}", path.display()))?;
        if manifest.version > MANIFEST_VERSION {
            return Err(format!(
                "{} was written by a newer Arnis (manifest version {}); update Arnis to extend this world.",
                path.display(),
                manifest.version
            ));
        }
        manifest
            .validate()
            .map_err(|e| format!("{} is damaged: {e}", path.display()))?;
        Ok(Some(manifest))
    }

    fn validate(&self) -> Result<(), String> {
        if !(self.origin_lat.is_finite() && self.origin_lat.abs() <= MAX_ABS_LAT) {
            return Err(format!("origin latitude {}", self.origin_lat));
        }
        if !(self.origin_lon.is_finite() && self.origin_lon.abs() <= 180.0) {
            return Err(format!("origin longitude {}", self.origin_lon));
        }
        crate::args::validate_scale(self.scale)?;
        if let Some(e) = &self.elevation {
            if !(e.min_height_m.is_finite()
                && e.blocks_per_meter.is_finite()
                && e.blocks_per_meter >= 0.0)
            {
                return Err("elevation mapping".to_string());
            }
        }
        if let Some(a) = self
            .areas
            .iter()
            .find(|a| a.min_x > a.max_x || a.min_z > a.max_z)
        {
            return Err(format!("area {} has an empty rectangle", a.id));
        }
        Ok(())
    }

    pub fn save(&self, world_dir: &Path) -> Result<(), String> {
        let path = Self::path_in(world_dir);
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize the One World manifest: {e}"))?;
        crate::world_utils::replace_file_atomically(&path, text.as_bytes())
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))
    }

    pub fn extent(&self) -> Option<XZBBox> {
        let first = self.areas.first()?;
        let mut min_x = first.min_x;
        let mut min_z = first.min_z;
        let mut max_x = first.max_x;
        let mut max_z = first.max_z;
        for a in &self.areas[1..] {
            min_x = min_x.min(a.min_x);
            min_z = min_z.min(a.min_z);
            max_x = max_x.max(a.max_x);
            max_z = max_z.max(a.max_z);
        }
        XZBBox::rect_from_min_max(min_x, min_z, max_x, max_z).ok()
    }
}

/// Chunks inside `rect` that already exist in the world's region files,
/// read from the region headers alone.
pub fn existing_chunks(world_dir: &Path, rect: &XZBBox) -> u64 {
    let (cx0, cz0) = (rect.min_x() >> 4, rect.min_z() >> 4);
    let (cx1, cz1) = (rect.max_x() >> 4, rect.max_z() >> 4);
    let mut count = 0;
    for rz in (cz0 >> 5)..=(cz1 >> 5) {
        for rx in (cx0 >> 5)..=(cx1 >> 5) {
            let path = world_dir.join("region").join(format!("r.{rx}.{rz}.mca"));
            let mut header = [0u8; 4096];
            let read = std::fs::File::open(&path).and_then(|mut f| f.read_exact(&mut header));
            if read.is_err() {
                continue;
            }
            for lz in 0..32 {
                for lx in 0..32 {
                    let (cx, cz) = (rx * 32 + lx, rz * 32 + lz);
                    let i = 4 * (lx + lz * 32) as usize;
                    if (cx0..=cx1).contains(&cx)
                        && (cz0..=cz1).contains(&cz)
                        && header[i..i + 4] != [0; 4]
                    {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

/// The One World a run lands in, carried in `Args::one_world_run`.
#[derive(Clone, Debug)]
pub struct RunContext {
    pub world_dir: PathBuf,
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub extending: bool,
    pub elevation: Option<ElevationAffine>,
    /// Chunks of this area that already exist and are replaced.
    pub replaced_chunks: u64,
    pub area_id: u32,
}

impl RunContext {
    pub fn preview_path(&self) -> PathBuf {
        self.world_dir
            .join(PREVIEW_DIR)
            .join(format!("area-{}.png", self.area_id))
    }
}

/// A resolved run. Holds the world's session lock until dropped.
pub struct Session {
    pub created: bool,
    pub llbbox: LLBBox,
    pub lock: SessionLock,
}

fn compatibility_errors(manifest: &Manifest, args: &Args) -> Vec<String> {
    let mut errors = Vec::new();
    if (manifest.scale - args.scale).abs() > 1e-9 {
        errors.push(format!(
            "world scale {:.2} does not match the world's {:.2}",
            args.scale, manifest.scale
        ));
    }
    if manifest.ground_level != args.ground_level {
        errors.push(format!(
            "ground level {} does not match the world's {}",
            args.ground_level, manifest.ground_level
        ));
    }
    if manifest.terrain != args.terrain() {
        errors.push(format!(
            "the world was generated {} terrain, so this area must be too",
            if manifest.terrain { "with" } else { "without" }
        ));
    }
    if manifest.disable_height_limit != args.disable_height_limit {
        errors.push(format!(
            "the world was generated {} the extended build height",
            if manifest.disable_height_limit {
                "with"
            } else {
                "without"
            }
        ));
    }
    errors
}

fn open_in_minecraft(world_dir: &Path) -> String {
    format!(
        "The One World at {} is open in Minecraft. Leave the world (or close the game) and try again.",
        world_dir.display()
    )
}

/// Opens or creates the One World at `world_dir`, locks it, and points `args`
/// at the world's frame.
pub fn prepare(world_dir: &Path, requested: &LLBBox, args: &mut Args) -> Result<Session, String> {
    if args.bedrock || args.luanti {
        return Err("One World is available for Java Edition worlds only.".to_string());
    }
    if !args.body.is_earth() {
        return Err("One World is available for Earth only.".to_string());
    }
    if args.rotation.abs() > f64::EPSILON {
        return Err(
            "One World keeps the world aligned to real-world coordinates, so rotation must be 0."
                .to_string(),
        );
    }
    if requested.min().lat() < -MAX_ABS_LAT || requested.max().lat() > MAX_ABS_LAT {
        return Err(format!(
            "One World covers latitudes up to {MAX_ABS_LAT} degrees north and south."
        ));
    }

    let fresh_dir = !world_dir.exists();
    if !fresh_dir && Manifest::load(world_dir)?.is_none() {
        let empty = std::fs::read_dir(world_dir)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);
        if !empty {
            return Err(format!(
                "{} exists but is not a One World (no {}). Choose another world name or delete the folder.",
                world_dir.display(),
                MANIFEST_FILE
            ));
        }
    }
    if world_is_locked(world_dir) {
        return Err(open_in_minecraft(world_dir));
    }
    std::fs::create_dir_all(world_dir)
        .map_err(|e| format!("Failed to create {}: {e}", world_dir.display()))?;
    let lock = SessionLock::acquire(world_dir).map_err(|_| open_in_minecraft(world_dir))?;
    let resolved = resolve(world_dir, requested, args, lock);
    if resolved.is_err() && fresh_dir {
        // Only while still empty, so nothing another process put there is lost.
        let _ = std::fs::remove_dir(world_dir);
    }
    resolved
}

/// The part of `prepare` that runs under the world's lock.
fn resolve(
    world_dir: &Path,
    requested: &LLBBox,
    args: &mut Args,
    lock: SessionLock,
) -> Result<Session, String> {
    let (mut manifest, created) = match Manifest::load(world_dir)? {
        Some(manifest) => {
            if !world_dir.join("level.dat").is_file() {
                return Err(format!(
                    "{} has a One World manifest but no level.dat; the world seems damaged.",
                    world_dir.display()
                ));
            }
            let errors = compatibility_errors(&manifest, args);
            if !errors.is_empty() {
                return Err(format!(
                    "This area cannot be added to the One World at {}: {}. Change the setting, or use another world name to start a new One World.",
                    world_dir.display(),
                    errors.join("; ")
                ));
            }
            args.scale = manifest.scale;
            (manifest, false)
        }
        None => {
            // Checked again under the lock: the folder may have filled up since.
            let foreign = std::fs::read_dir(world_dir)
                .map_err(|e| format!("Failed to read {}: {e}", world_dir.display()))?
                .filter_map(Result::ok)
                .any(|entry| entry.file_name() != "session.lock");
            if foreign {
                return Err(format!(
                    "{} exists but is not a One World (no {}). Choose another world name or delete the folder.",
                    world_dir.display(),
                    MANIFEST_FILE
                ));
            }
            let origin_lat = (requested.min().lat() + requested.max().lat()) / 2.0;
            let origin_lon = (requested.min().lng() + requested.max().lng()) / 2.0;
            (Manifest::new(args, origin_lat, origin_lon), true)
        }
    };

    let (xzbbox, llbbox) = snap_bbox_to_chunks(&manifest.projection(), requested)?;

    if created {
        let name = world_dir
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(DEFAULT_WORLD_NAME)
            .to_string();
        crate::world_utils::write_world_skeleton(world_dir, &name, false)?;
        manifest.save(world_dir)?;
    }

    if manifest.version < 2 {
        let removed = crate::world_editor::java::drop_misplaced_chunks(world_dir).map_err(|e| {
            format!(
                "Failed to repair the One World at {}: {e}",
                world_dir.display()
            )
        })?;
        println!(
            "One World: repaired a world from an earlier build ({removed} stray chunks dropped)."
        );
        manifest.version = MANIFEST_VERSION;
        manifest.save(world_dir)?;
    }

    if manifest.aws_only_elevation != args.aws_only_elevation {
        println!(
            "Note: One World keeps the elevation source it was created with ({}).",
            if manifest.aws_only_elevation {
                "legacy AWS terrain"
            } else {
                "high-resolution terrain"
            }
        );
        args.aws_only_elevation = manifest.aws_only_elevation;
    }

    let replaced = if created {
        0
    } else {
        existing_chunks(world_dir, &xzbbox)
    };
    let extending = !manifest.areas.is_empty();
    let area_id = manifest.next_area_id;

    args.one_world_run = Some(RunContext {
        world_dir: world_dir.to_path_buf(),
        origin_lat: manifest.origin_lat,
        origin_lon: manifest.origin_lon,
        extending,
        elevation: manifest.elevation,
        replaced_chunks: replaced,
        area_id,
    });
    args.projection = crate::projection::ProjectionKind::WebMercator;
    args.bbox = Some(llbbox);
    // Voxy rebuilds the LOD database from the regions of one run, and both
    // facade sources replace the world's resource pack on every run.
    if args.voxy_lod {
        println!("Note: the Voxy LOD cache is off in One World mode.");
        args.voxy_lod = false;
    }
    if args.mapillary_facade_mode.places_displays() && args.mapillary_facades_wanted() {
        println!("Note: One World builds Mapillary facades as blocks; photo panels are off.");
        args.mapillary_facade_mode = crate::args::FacadeMode::Blocks;
    }
    if args.building_facades {
        println!("Note: the preset building facades are off in One World mode.");
        args.building_facades = false;
    }
    args.map_preview = true;

    println!(
        "One World: {} {} at {}",
        if created {
            "created"
        } else if extending {
            "extending"
        } else {
            "using"
        },
        world_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(DEFAULT_WORLD_NAME),
        world_dir.display()
    );
    println!(
        "  area #{area_id}: blocks x {}..={} z {}..={} ({} x {} chunks){}",
        xzbbox.min_x(),
        xzbbox.max_x(),
        xzbbox.min_z(),
        xzbbox.max_z(),
        (xzbbox.max_x() - xzbbox.min_x() + 1) / 16,
        (xzbbox.max_z() - xzbbox.min_z() + 1) / 16,
        if replaced > 0 {
            format!(", {replaced} existing chunks will be replaced")
        } else {
            String::new()
        }
    );
    // Ground scale changes with latitude; far from the origin a request
    // becomes a much larger world than its size on the map suggests.
    let centre_lat = (llbbox.min().lat() + llbbox.max().lat()) / 2.0;
    let density = manifest.origin_lat.to_radians().cos() / centre_lat.to_radians().cos();
    if !(0.8..=1.25).contains(&density) {
        println!(
            "Note: this far from the world's origin one metre is {:.2} blocks instead of {:.2}.",
            args.scale * density,
            args.scale
        );
    }

    Ok(Session {
        created,
        llbbox,
        lock,
    })
}

/// Records a finished area and drops the ones it fully covers.
pub fn record_area(
    run: &RunContext,
    llbbox: &LLBBox,
    xzbbox: &XZBBox,
    elevation: Option<ElevationAffine>,
    preview: Option<&Path>,
) -> Result<(), String> {
    let mut manifest = Manifest::load(&run.world_dir)?.ok_or_else(|| {
        format!(
            "{} vanished during generation",
            Manifest::path_in(&run.world_dir).display()
        )
    })?;
    if manifest.elevation.is_none() {
        manifest.elevation = elevation;
    }
    let preview_rel = preview.and_then(|p| {
        p.strip_prefix(&run.world_dir)
            .ok()
            .map(|r| r.to_string_lossy().replace('\\', "/"))
    });
    let (covered, kept): (Vec<GeneratedArea>, Vec<GeneratedArea>) =
        std::mem::take(&mut manifest.areas)
            .into_iter()
            .partition(|a| a.is_inside(xzbbox));
    manifest.areas = kept;
    manifest.areas.push(GeneratedArea {
        id: run.area_id,
        generated_at: unix_now(),
        arnis_version: env!("CARGO_PKG_VERSION").to_string(),
        min_x: xzbbox.min_x(),
        min_z: xzbbox.min_z(),
        max_x: xzbbox.max_x(),
        max_z: xzbbox.max_z(),
        min_lat: llbbox.min().lat(),
        min_lon: llbbox.min().lng(),
        max_lat: llbbox.max().lat(),
        max_lon: llbbox.max().lng(),
        preview: preview_rel.clone(),
    });
    manifest.next_area_id = manifest.next_area_id.max(run.area_id.saturating_add(1));
    manifest.save(&run.world_dir)?;
    for old in covered {
        if let Some(p) = old.preview.filter(|p| Some(p) != preview_rel.as_ref()) {
            if let Some(path) = safe_preview_path(&run.world_dir, &p) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(())
}

/// Stores the elevation mapping as soon as the first terrain area has one,
/// so a run that fails later cannot leave chunks behind on another mapping.
pub fn remember_elevation(run: &RunContext, elevation: ElevationAffine) -> Result<(), String> {
    if run.elevation.is_some() {
        return Ok(());
    }
    let Some(mut manifest) = Manifest::load(&run.world_dir)? else {
        return Ok(());
    };
    if manifest.elevation.is_none() {
        manifest.elevation = Some(elevation);
        manifest.save(&run.world_dir)?;
    }
    Ok(())
}

/// Manifests can come with downloaded worlds, so their preview paths are
/// only followed inside the preview folder.
pub fn safe_preview_path(world_dir: &Path, rel: &str) -> Option<PathBuf> {
    let name = rel.strip_prefix(PREVIEW_DIR)?.strip_prefix('/')?;
    let valid = !name.is_empty()
        && name.ends_with(".png")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !name.contains("..");
    valid.then(|| world_dir.join(PREVIEW_DIR).join(name))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn args_for(bbox: &str, extra: &[&str]) -> Args {
        let mut cmd = vec!["arnis", "--output-dir", ".", "--bbox", bbox];
        cmd.extend_from_slice(extra);
        Args::parse_from(cmd)
    }

    const MUNICH: &str = "48.130,11.560,48.145,11.590";

    fn rect(args: &Args, s: &Session) -> XZBBox {
        crate::projection::ProjectionSpec::from_args(args)
            .transformer(&s.llbbox)
            .unwrap()
            .1
    }

    fn record(args: &Args, s: &Session, elevation: Option<ElevationAffine>) {
        let run = args.one_world_run.clone().unwrap();
        record_area(&run, &s.llbbox, &rect(args, s), elevation, None).unwrap();
    }

    #[test]
    fn a_new_world_gets_a_manifest_and_a_chunk_aligned_area() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("My One World");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &[]);
        let session = prepare(&world, &req, &mut args).unwrap();
        assert!(session.created);
        assert!(world.join("level.dat").is_file());
        assert!(!world.join("region").join("r.0.0.mca").exists());
        let manifest = Manifest::load(&world).unwrap().unwrap();
        assert!(manifest.areas.is_empty());
        let area = rect(&args, &session);
        assert_eq!(area.min_x().rem_euclid(16), 0);
        assert_eq!((area.max_z() + 1).rem_euclid(16), 0);
        let run = args.one_world_run.as_ref().unwrap();
        assert_eq!(run.area_id, 1);
        assert!(!run.extending);
        assert_eq!(
            args.projection,
            crate::projection::ProjectionKind::WebMercator
        );
        assert_eq!(args.bbox.unwrap(), session.llbbox);
    }

    #[test]
    fn recorded_areas_survive_and_a_second_run_extends() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let mut args = args_for(MUNICH, &[]);
        let session = prepare(&world, &LLBBox::from_str(MUNICH).unwrap(), &mut args).unwrap();
        let affine = ElevationAffine {
            min_height_m: 500.0,
            blocks_per_meter: 1.0,
            ground_level: -62,
        };
        record(&args, &session, Some(affine));
        drop(session);
        let run = args.one_world_run.clone().unwrap();

        let east = "48.130,11.585,48.145,11.610";
        let mut args2 = args_for(east, &[]);
        let session2 = prepare(&world, &LLBBox::from_str(east).unwrap(), &mut args2).unwrap();
        assert!(!session2.created);
        let run2 = args2.one_world_run.as_ref().unwrap();
        assert_eq!(run2.area_id, 2);
        assert!(run2.extending);
        assert_eq!(run2.elevation, Some(affine));
        assert_eq!(run2.origin_lat, run.origin_lat);
        assert_eq!(run2.origin_lon, run.origin_lon);
        assert_eq!(run2.replaced_chunks, 0, "nothing was written yet");
    }

    #[test]
    fn an_incompatible_setting_is_refused_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        drop(prepare(&world, &req, &mut args_for(MUNICH, &[])).unwrap());

        let err = prepare(&world, &req, &mut args_for(MUNICH, &["--scale", "2"]))
            .err()
            .unwrap();
        assert!(err.contains("world scale 2.00"), "{err}");

        let err = prepare(&world, &req, &mut args_for(MUNICH, &["--mode", "geo-only"]))
            .err()
            .unwrap();
        assert!(err.contains("with terrain"), "{err}");
    }

    #[test]
    fn a_locked_world_is_refused_and_the_session_holds_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let session = prepare(&world, &req, &mut args_for(MUNICH, &[])).unwrap();
        let err = prepare(&world, &req, &mut args_for(MUNICH, &[]))
            .err()
            .unwrap();
        assert!(err.contains("open in Minecraft"), "{err}");
        drop(session);

        let held = SessionLock::acquire(&world).unwrap();
        assert!(prepare(&world, &req, &mut args_for(MUNICH, &[])).is_err());
        drop(held);
        assert!(prepare(&world, &req, &mut args_for(MUNICH, &[])).is_ok());
    }

    #[test]
    fn a_refused_first_run_leaves_no_folder_behind() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        // Wider than the world border at scale 4, refused after the lock is taken.
        let huge = "-10.0,-179.0,10.0,179.0";
        let err = prepare(
            &world,
            &LLBBox::from_str(huge).unwrap(),
            &mut args_for(huge, &["--scale", "4"]),
        )
        .err()
        .unwrap();
        assert!(err.contains("world border"), "{err}");
        assert!(!world.exists());
    }

    #[test]
    fn a_foreign_folder_is_not_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        std::fs::create_dir_all(&world).unwrap();
        std::fs::write(world.join("level.dat"), b"x").unwrap();
        let err = prepare(
            &world,
            &LLBBox::from_str(MUNICH).unwrap(),
            &mut args_for(MUNICH, &[]),
        )
        .err()
        .unwrap();
        assert!(err.contains("not a One World"), "{err}");
    }

    #[test]
    fn refused_runs_create_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        assert!(prepare(&world, &req, &mut args_for(MUNICH, &["--rotation", "10"])).is_err());
        assert!(prepare(&world, &req, &mut args_for(MUNICH, &["--bedrock"])).is_err());
        let polar = "86.0,10.0,86.1,10.1";
        assert!(prepare(
            &world,
            &LLBBox::from_str(polar).unwrap(),
            &mut args_for(polar, &[])
        )
        .is_err());
        assert!(!world.exists());
    }

    #[test]
    fn an_area_past_the_world_border_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &["--scale", "4"]);
        let session = prepare(&world, &req, &mut args).unwrap();
        record(&args, &session, None);
        drop(session);
        let far = "-33.88,151.19,-33.85,151.23";
        let err = prepare(
            &world,
            &LLBBox::from_str(far).unwrap(),
            &mut args_for(far, &["--scale", "4"]),
        )
        .err()
        .unwrap();
        assert!(err.contains("world border"), "{err}");
    }

    #[test]
    fn covered_areas_are_dropped_and_an_identical_rerun_replaces_itself() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &[]);
        let s1 = prepare(&world, &req, &mut args).unwrap();
        record(&args, &s1, None);
        drop(s1);
        let mut again = args_for(MUNICH, &[]);
        let s1b = prepare(&world, &req, &mut again).unwrap();
        record(&again, &s1b, None);
        drop(s1b);
        let m = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(m.areas.len(), 1);
        assert_eq!(m.areas[0].id, 2);

        let big = "48.120,11.550,48.155,11.600";
        let mut args2 = args_for(big, &[]);
        let s2 = prepare(&world, &LLBBox::from_str(big).unwrap(), &mut args2).unwrap();
        record(&args2, &s2, None);
        let m = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(m.areas.len(), 1);
        assert_eq!(m.areas[0].id, 3);
        assert_eq!(m.extent().unwrap().min_x(), rect(&args2, &s2).min_x());
    }

    #[test]
    fn a_damaged_manifest_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        drop(prepare(&world, &req, &mut args_for(MUNICH, &[])).unwrap());
        let mut m = Manifest::load(&world).unwrap().unwrap();
        m.scale = 0.0;
        m.save(&world).unwrap();
        let err = Manifest::load(&world).unwrap_err();
        assert!(err.contains("damaged"), "{err}");
    }

    #[test]
    fn preview_paths_stay_inside_the_preview_folder() {
        let w = Path::new("/w");
        assert_eq!(
            safe_preview_path(w, "arnis_one_world/previews/area-3.png"),
            Some(w.join(PREVIEW_DIR).join("area-3.png"))
        );
        assert_eq!(
            safe_preview_path(w, "arnis_one_world/previews/../level.dat"),
            None
        );
        assert_eq!(safe_preview_path(w, "../../etc/passwd"), None);
        assert_eq!(
            safe_preview_path(w, "arnis_one_world/previews/a/b.png"),
            None
        );
        assert_eq!(safe_preview_path(w, "arnis_one_world/previews/x.txt"), None);
    }

    #[test]
    fn existing_chunks_are_read_from_the_region_headers() {
        let dir = tempfile::tempdir().unwrap();
        let region = dir.path().join("region");
        std::fs::create_dir_all(&region).unwrap();
        let mut header = vec![0u8; 8192];
        // Chunks (0, 0) and (1, 0) of region (0, 0), and (31, 31) of region (-1, -1).
        header[0..4].copy_from_slice(&[0, 0, 2, 1]);
        header[4..8].copy_from_slice(&[0, 0, 3, 1]);
        std::fs::write(region.join("r.0.0.mca"), &header).unwrap();
        let mut other = vec![0u8; 8192];
        let i = 4 * (31 + 31 * 32);
        other[i..i + 4].copy_from_slice(&[0, 0, 2, 1]);
        std::fs::write(region.join("r.-1.-1.mca"), &other).unwrap();

        let rect = XZBBox::rect_from_min_max(-16, -16, 15, 15).unwrap();
        assert_eq!(existing_chunks(dir.path(), &rect), 2);
        let rect = XZBBox::rect_from_min_max(16, 0, 31, 15).unwrap();
        assert_eq!(existing_chunks(dir.path(), &rect), 1);
        let rect = XZBBox::rect_from_min_max(512, 512, 527, 527).unwrap();
        assert_eq!(existing_chunks(dir.path(), &rect), 0);
    }

    #[test]
    fn a_manifest_without_areas_has_no_extent() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        drop(
            prepare(
                &world,
                &LLBBox::from_str(MUNICH).unwrap(),
                &mut args_for(MUNICH, &[]),
            )
            .unwrap(),
        );
        assert!(Manifest::load(&world).unwrap().unwrap().extent().is_none());
    }
}
