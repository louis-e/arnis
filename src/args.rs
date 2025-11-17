use crate::coordinate_system::geographic::LLBBox;
use clap::Parser;
use std::path::PathBuf;
use std::time::Duration;

/// Command-line arguments parser
#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Args {
    /// Bounding box of the area (min_lat,min_lng,max_lat,max_lng) (required)
    #[arg(long, allow_hyphen_values = true, value_parser = LLBBox::from_str)]
    pub bbox: LLBBox,

    /// JSON file containing OSM data (optional)
    #[arg(long, group = "location")]
    pub file: Option<String>,

    /// JSON file to save OSM data to (optional)
    #[arg(long, group = "location")]
    pub save_json_file: Option<String>,

    /// Path to the Minecraft world (required)
    #[arg(long, value_parser = validate_minecraft_world_path)]
    pub path: PathBuf,

    /// Downloader method (requests/curl/wget) (optional)
    #[arg(long, default_value = "requests")]
    pub downloader: String,

    /// World scale to use, in blocks per meter
    #[arg(long, default_value_t = 1.0)]
    pub scale: f64,

    /// Ground level to use in the Minecraft world
    #[arg(long, default_value_t = -62)]
    pub ground_level: i32,

    /// Enable terrain (optional)
    #[arg(long)]
    pub terrain: bool,

    /// Enable interior generation (optional)
    #[arg(long, default_value_t = true, action = clap::ArgAction::SetTrue)]
    pub interior: bool,

    /// Enable roof generation (optional)
    #[arg(long, default_value_t = true, action = clap::ArgAction::SetTrue)]
    pub roof: bool,

    /// Enable filling ground (optional)
    #[arg(long, default_value_t = false, action = clap::ArgAction::SetFalse)]
    pub fillground: bool,

    /// Enable debug mode (optional)
    #[arg(long)]
    pub debug: bool,

    /// Set floodfill timeout (seconds) (optional)
    #[arg(long, value_parser = parse_duration)]
    pub timeout: Option<Duration>,

    /// Spawn point coordinates (lat, lng)
    #[arg(skip)]
    pub spawn_point: Option<(f64, f64)>,
}

fn validate_minecraft_world_path(path: &str) -> Result<PathBuf, String> {
    let mc_world_path = PathBuf::from(path);
    if !mc_world_path.exists() {
        return Err(format!("Path does not exist: {path}"));
    }
    if !mc_world_path.is_dir() {
        return Err(format!("Path is not a directory: {path}"));
    }
    let region = mc_world_path.join("region");
    if !region.is_dir() {
        return Err(format!("No Minecraft world found at {region:?}"));
    }
    Ok(mc_world_path)
}

fn parse_duration(arg: &str) -> Result<std::time::Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(std::time::Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minecraft_tmpdir() -> tempfile::TempDir {
        let tmpdir = tempfile::tempdir().unwrap();
        // create a `region` directory in the tempdir
        let region_path = tmpdir.path().join("region");
        std::fs::create_dir(&region_path).unwrap();
        tmpdir
    }
    #[test]
    fn test_flags() {
        let tmpdir = minecraft_tmpdir();
        let tmp_path = tmpdir.path().to_str().unwrap();

        // Test that terrain/debug are SetTrue
        let cmd = [
            "arnis",
            "--path",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--terrain",
            "--debug",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(args.debug);
        assert!(args.terrain);

        let cmd = ["arnis", "--path", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.debug);
        assert!(!args.terrain);
    }

    #[test]
    fn test_required_options() {
        let tmpdir = minecraft_tmpdir();
        let tmp_path = tmpdir.path().to_str().unwrap();

        let cmd = ["arnis"];
        assert!(Args::try_parse_from(cmd.iter()).is_err());

        let cmd = ["arnis", "--path", tmp_path, "--bbox", "1,2,3,4"];
        assert!(Args::try_parse_from(cmd.iter()).is_ok());

        let cmd = ["arnis", "--path", tmp_path, "--file", ""];
        assert!(Args::try_parse_from(cmd.iter()).is_err());

        // The --gui flag isn't used here, ugh. TODO clean up main.rs and its argparse usage.
        // let cmd = ["arnis", "--gui"];
        // assert!(Args::try_parse_from(cmd.iter()).is_ok());
    }
}
