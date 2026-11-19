use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_WORKSPACE_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) struct WorldPainterRun {
    workspace: PathBuf,
    export_world: PathBuf,
    features_world: PathBuf,
    keep_export: bool,
}

impl WorldPainterRun {
    pub(crate) fn prepare(
        project: &Path,
        executable: &Path,
        output_world: &Path,
        keep_export: bool,
    ) -> Result<Self, String> {
        if !project.is_file()
            || project
                .extension()
                .is_none_or(|ext| !ext.eq_ignore_ascii_case("world"))
        {
            return Err(format!(
                "WorldPainter project must be an existing .world file: {}",
                project.display()
            ));
        }
        if !executable.is_file() {
            return Err(format!(
                "WorldPainter executable does not exist: {}",
                executable.display()
            ));
        }
        let parent = output_world.parent().ok_or_else(|| {
            format!(
                "World output path has no parent: {}",
                output_world.display()
            )
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("create output directory {}: {error}", parent.display()))?;
        let workspace = create_workspace(parent)?;
        let prepared = (|| {
            let export_directory = workspace.join("export");
            fs::create_dir_all(&export_directory).map_err(|error| {
                format!(
                    "create WorldPainter export directory {}: {error}",
                    export_directory.display()
                )
            })?;
            let script = workspace.join("worldpainter-export.js");
            fs::write(&script, include_str!("worldpainter_export.js"))
                .map_err(|error| format!("write WorldPainter export script: {error}"))?;

            crate::progress::emit_gui_progress_update(5.0, "Exporting WorldPainter project...");
            println!("Exporting WorldPainter project {}", project.display());
            let mut command = Command::new(executable);
            command
                .arg("-script")
                .arg(&script)
                .arg(project)
                .arg(&export_directory);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000);
            }
            let output = command.output().map_err(|error| {
                format!(
                    "start WorldPainter executable {}: {error}",
                    executable.display()
                )
            })?;
            if !output.status.success() {
                return Err(format!(
                    "WorldPainter export failed with {}.\n{}{}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let export_world = find_exported_world(&export_directory)?;
            let features_world = workspace.join("features");
            fs::create_dir_all(&features_world)
                .map_err(|error| format!("create sparse feature world: {error}"))?;
            Ok(Self {
                workspace: workspace.clone(),
                export_world,
                features_world,
                keep_export,
            })
        })();
        if prepared.is_err() {
            if let Err(error) = fs::remove_dir_all(&workspace) {
                eprintln!(
                    "Warning: failed to remove WorldPainter staging directory {}: {error}",
                    workspace.display()
                );
            }
        }
        prepared
    }

    pub(crate) fn features_world(&self) -> &Path {
        &self.features_world
    }

    pub(crate) fn spawn_y_at(
        &self,
        world_path: &Path,
        block_x: i32,
        block_z: i32,
    ) -> Result<i32, String> {
        crate::world_editor::region_overlay::ground_surface_at(world_path, block_x, block_z)?
            .checked_add(3)
            .ok_or_else(|| "WorldPainter spawn height is out of range".to_string())
    }

    pub(crate) fn cleanup(&self) {
        if let Err(error) = fs::remove_dir_all(&self.workspace) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "Warning: failed to remove WorldPainter staging directory {}: {error}",
                    self.workspace.display()
                );
            }
        }
    }

    pub(crate) fn publish(&self, output_world: &Path) -> Result<usize, String> {
        crate::progress::emit_gui_progress_update(
            crate::progress::MESSAGE_ONLY,
            "Merging features into WorldPainter export...",
        );
        crate::world_utils::remove_untouched_template_region(output_world);
        if self.keep_export {
            let unmerged = output_world.join("WorldPainter Export");
            copy_directory_contents(&self.export_world, &unmerged)?;
        }
        copy_directory_contents(&self.export_world, output_world)?;

        for directory in ["data", "datapacks"] {
            let source = self.features_world.join(directory);
            if source.is_dir() {
                copy_directory_contents(&source, &output_world.join(directory))?;
            }
        }

        let feature_regions = self.features_world.join("region");
        if !feature_regions.is_dir() {
            return Ok(0);
        }
        let terrain_regions = output_world.join("region");
        let entries = fs::read_dir(&feature_regions).map_err(|error| {
            format!(
                "read sparse feature regions {}: {error}",
                feature_regions.display()
            )
        })?;
        let mut applied = 0;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "read sparse feature region entry in {}: {error}",
                    feature_regions.display()
                )
            })?;
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if !is_region_file(name) {
                continue;
            }
            let terrain_path = terrain_regions.join(name);
            if !terrain_path.is_file() {
                return Err(format!(
                    "WorldPainter export is missing matching region {}",
                    terrain_path.display()
                ));
            }
            crate::world_editor::region_overlay::validate_ground_metadata(&entry.path())?;
            applied += crate::world_editor::region_overlay::merge_feature_region(
                &terrain_path,
                &entry.path(),
            )?;
        }
        println!("Merged {applied} Arnis feature blocks into the WorldPainter export.");
        Ok(applied)
    }
}

impl Drop for WorldPainterRun {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn create_workspace(parent: &Path) -> Result<PathBuf, String> {
    for _ in 0..100 {
        let id = NEXT_WORKSPACE_ID.fetch_add(1, Ordering::Relaxed);
        let workspace = parent.join(format!(".arnis-worldpainter-{}-{id}", std::process::id()));
        match fs::create_dir(&workspace) {
            Ok(()) => return Ok(workspace),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "create WorldPainter staging directory {}: {error}",
                    workspace.display()
                ));
            }
        }
    }
    Err(format!(
        "could not allocate a unique WorldPainter staging directory in {}",
        parent.display()
    ))
}

fn find_exported_world(root: &Path) -> Result<PathBuf, String> {
    fn visit(path: &Path, depth: usize, found: &mut Vec<PathBuf>) -> Result<(), String> {
        if path.join("level.dat").is_file() && path.join("region").is_dir() {
            found.push(path.to_path_buf());
            return Ok(());
        }
        if depth == 4 {
            return Ok(());
        }
        let entries = fs::read_dir(path)
            .map_err(|error| format!("read WorldPainter export {}: {error}", path.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "read WorldPainter export entry in {}: {error}",
                    path.display()
                )
            })?;
            if entry
                .file_type()
                .map_err(|error| {
                    format!(
                        "read WorldPainter export entry type {}: {error}",
                        entry.path().display()
                    )
                })?
                .is_dir()
            {
                visit(&entry.path(), depth + 1, found)?;
            }
        }
        Ok(())
    }

    let mut found = Vec::new();
    visit(root, 0, &mut found)?;
    match found.as_slice() {
        [world] => Ok(world.clone()),
        [] => Err(format!(
            "WorldPainter did not export a Java world with level.dat and region/: {}",
            root.display()
        )),
        _ => Err(format!(
            "WorldPainter export contains multiple world folders; expected one in {}",
            root.display()
        )),
    }
}

fn is_region_file(name: &str) -> bool {
    let parts: Vec<_> = name.split('.').collect();
    parts.len() == 4
        && parts[0] == "r"
        && parts[1].parse::<i32>().is_ok()
        && parts[2].parse::<i32>().is_ok()
        && parts[3] == "mca"
}

fn copy_directory_contents(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("create directory {}: {error}", destination.display()))?;
    let entries = fs::read_dir(source)
        .map_err(|error| format!("read directory {}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("read directory entry: {error}"))?;
        let name = entry.file_name();
        if name == "session.lock" {
            continue;
        }
        let source_path = entry.path();
        let destination_path = destination.join(&name);
        let file_type = entry
            .file_type()
            .map_err(|error| format!("read file type {}: {error}", source_path.display()))?;
        if file_type.is_dir() {
            copy_directory_contents(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &destination_path).map_err(|error| {
                format!(
                    "copy {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_world_discovery_accepts_worldpainter_nested_folder() {
        let temp = tempfile::tempdir().unwrap();
        let world = temp.path().join("export").join("dem_modified");
        fs::create_dir_all(world.join("region")).unwrap();
        fs::write(world.join("level.dat"), []).unwrap();
        assert_eq!(
            find_exported_world(&temp.path().join("export")).unwrap(),
            world
        );
    }

    #[test]
    fn region_file_filter_rejects_non_region_files() {
        assert!(is_region_file("r.-1.2.mca"));
        assert!(!is_region_file("level.dat"));
        assert!(!is_region_file("r.1.2.mca_old"));
    }
}
