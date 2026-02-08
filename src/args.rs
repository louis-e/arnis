use crate::coordinate_system::geographic::LLBBox;
use clap::Parser;
use std::path::{Path, PathBuf};
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

    /// Path to the Minecraft world (required for Java, optional for Bedrock)
    #[arg(long)]
    pub path: Option<PathBuf>,

    /// Generate a Bedrock Edition world (.mcworld) instead of Java Edition
    #[arg(long)]
    pub bedrock: bool,

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
    #[arg(long, default_value_t = true)]
    pub interior: bool,

    /// Enable roof generation (optional)
    #[arg(long, default_value_t = true)]
    pub roof: bool,

    /// Enable filling ground (optional)
    #[arg(long, default_value_t = false)]
    pub fillground: bool,

    /// Enable city boundary ground generation (optional)
    /// When enabled, detects building clusters and places stone ground in urban areas.
    /// Isolated buildings in rural areas will keep grass around them.
    #[arg(long, default_value_t = true)]
    pub city_boundaries: bool,

    /// Enable debug mode (optional)
    #[arg(long)]
    pub debug: bool,

    /// Set floodfill timeout (seconds) (optional)
    #[arg(long, value_parser = parse_duration)]
    pub timeout: Option<Duration>,
}

fn validate_minecraft_world_path(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("Path does not exist: {}", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("Path is not a directory: {}", path.display()));
    }
    let region = path.join("region");
    if !region.is_dir() {
        return Err(format!("No Minecraft world found at {}", region.display()));
    }
    Ok(())
}

/// Validates CLI arguments after parsing.
/// For Java Edition: `--path` is required and must point to an existing Minecraft world with a `region` subdirectory.
/// For Bedrock Edition (`--bedrock`): `--path` is optional (defaults to Desktop output).
pub fn validate_args(args: &Args) -> Result<(), String> {
    if args.bedrock {
        // Bedrock: path is optional; if provided, it must be an existing directory
        if let Some(ref path) = args.path {
            if !path.is_dir() {
                return Err(format!("Path is not a directory: {}", path.display()));
            }
        }
    } else {
        // Java: path is required and must be a valid Minecraft world
        match &args.path {
            None => {
                return Err(
                    "The --path argument is required for Java Edition. Use --bedrock for Bedrock Edition output."
                        .to_string(),
                );
            }
            Some(ref path) => {
                validate_minecraft_world_path(path)?;
            }
        }
    }
    Ok(())
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
        assert!(!args.bedrock);
    }

    #[test]
    fn test_bedrock_flag() {
        // Bedrock mode doesn't require --path
        let cmd = ["arnis", "--bedrock", "--bbox", "1,2,3,4"];
        let args = Args::parse_from(cmd.iter());
        assert!(args.bedrock);
        assert!(args.path.is_none());
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn test_java_requires_path() {
        let cmd = ["arnis", "--bbox", "1,2,3,4"];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.bedrock);
        assert!(args.path.is_none());
        assert!(validate_args(&args).is_err());
    }

    #[test]
    fn test_required_options() {
        let tmpdir = minecraft_tmpdir();
        let tmp_path = tmpdir.path().to_str().unwrap();

        let cmd = ["arnis"];
        assert!(Args::try_parse_from(cmd.iter()).is_err());

        let cmd = ["arnis", "--path", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&args).is_ok());

        let cmd = ["arnis", "--path", tmp_path, "--file", ""];
        assert!(Args::try_parse_from(cmd.iter()).is_err());

        // The --gui flag isn't used here, ugh. TODO clean up main.rs and its argparse usage.
        // let cmd = ["arnis", "--gui"];
        // assert!(Args::try_parse_from(cmd.iter()).is_ok());
    }
}
