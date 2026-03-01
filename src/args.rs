use crate::coordinate_system::geographic::LLBBox;
use clap::{ArgAction, Parser};
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

    /// Output directory for the generated world (required for Java, optional for Bedrock).
    /// Use --output-dir (or the deprecated --path alias) to specify where the world is created.
    #[arg(long = "output-dir", alias = "path")]
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
    #[arg(long, default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub interior: bool,

    /// Enable roof generation (optional)
    #[arg(long, default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub roof: bool,

    /// Enable filling ground (optional)
    #[arg(long, default_value_t = false)]
    pub fillground: bool,

    /// Enable city ground generation (optional)
    /// When enabled, detects building clusters and places stone ground in urban areas.
    /// Isolated buildings in rural areas will keep grass around them.
    #[arg(long, default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub city_boundaries: bool,

    /// Enable debug mode (optional)
    #[arg(long)]
    pub debug: bool,

    /// Set floodfill timeout (seconds) (optional)
    #[arg(long, value_parser = parse_duration)]
    pub timeout: Option<Duration>,

    /// Spawn point latitude (optional, must be within bbox)
    #[arg(long, allow_hyphen_values = true)]
    pub spawn_lat: Option<f64>,

    /// Spawn point longitude (optional, must be within bbox)
    #[arg(long, allow_hyphen_values = true)]
    pub spawn_lng: Option<f64>,
}

/// Validates CLI arguments after parsing.
/// For Java Edition: `--path` is required and must point to an existing directory
/// where a new world will be created automatically.
/// For Bedrock Edition (`--bedrock`): `--path` is optional (defaults to Desktop output).
pub fn validate_args(args: &Args) -> Result<(), String> {
    if args.bedrock {
        // Bedrock: path is optional; if provided, it must be an existing directory
        if let Some(ref path) = args.path {
            if !path.exists() {
                return Err(format!("Path does not exist: {}", path.display()));
            }
            if !path.is_dir() {
                return Err(format!("Path is not a directory: {}", path.display()));
            }
        }
    } else {
        // Java: path is required and must be an existing directory
        match &args.path {
            None => {
                return Err(
                    "The --output-dir argument is required for Java Edition. Provide the directory where the world should be created. Use --bedrock for Bedrock Edition output."
                        .to_string(),
                );
            }
            Some(ref path) => {
                if !path.exists() {
                    return Err(format!("Path does not exist: {}", path.display()));
                }
                if !path.is_dir() {
                    return Err(format!("Path is not a directory: {}", path.display()));
                }
            }
        }
    }

    // Validate spawn point: both or neither must be provided
    match (args.spawn_lat, args.spawn_lng) {
        (Some(_), None) | (None, Some(_)) => {
            return Err(
                "Both --spawn-lat and --spawn-lng must be provided together.".to_string(),
            );
        }
        (Some(lat), Some(lng)) => {
            // Validate that spawn point is within the bounding box
            if lat < args.bbox.min().lat()
                || lat > args.bbox.max().lat()
                || lng < args.bbox.min().lng()
                || lng > args.bbox.max().lng()
            {
                return Err(
                    "Spawn point (--spawn-lat, --spawn-lng) must be within the bounding box."
                        .to_string(),
                );
            }
        }
        _ => {}
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

    #[test]
    fn test_flags() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        // Test that terrain/debug are SetTrue
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--terrain",
            "--debug",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(args.debug);
        assert!(args.terrain);

        let cmd = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.debug);
        assert!(!args.terrain);
        assert!(!args.bedrock);
        // interior, roof, city_boundaries default to true
        assert!(args.interior);
        assert!(args.roof);
        assert!(args.city_boundaries);
    }

    #[test]
    fn test_bool_flags_can_be_disabled() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        // Test disabling interior/roof/city-boundaries with =false
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--interior=false",
            "--roof=false",
            "--city-boundaries=false",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.interior);
        assert!(!args.roof);
        assert!(!args.city_boundaries);

        // Test enabling with bare flag (no value)
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--interior",
            "--roof",
            "--city-boundaries",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(args.interior);
        assert!(args.roof);
        assert!(args.city_boundaries);
    }

    #[test]
    fn test_bedrock_flag() {
        // Bedrock mode doesn't require --output-dir
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
    fn test_java_path_must_exist() {
        let cmd = [
            "arnis",
            "--output-dir",
            "/nonexistent/path",
            "--bbox",
            "1,2,3,4",
        ];
        let args = Args::parse_from(cmd.iter());
        let result = validate_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_bedrock_path_must_exist() {
        let cmd = [
            "arnis",
            "--bedrock",
            "--output-dir",
            "/nonexistent/path",
            "--bbox",
            "1,2,3,4",
        ];
        let args = Args::parse_from(cmd.iter());
        let result = validate_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_required_options() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        let cmd = ["arnis"];
        assert!(Args::try_parse_from(cmd.iter()).is_err());

        let cmd = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&args).is_ok());

        // Verify --path still works as a deprecated alias
        let cmd = ["arnis", "--path", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&args).is_ok());

        let cmd = ["arnis", "--output-dir", tmp_path, "--file", ""];
        assert!(Args::try_parse_from(cmd.iter()).is_err());

        // The --gui flag isn't used here, ugh. TODO clean up main.rs and its argparse usage.
        // let cmd = ["arnis", "--gui"];
        // assert!(Args::try_parse_from(cmd.iter()).is_ok());
    }

    #[test]
    fn test_spawn_point_both_required() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        // Only spawn-lat without spawn-lng should fail validation
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--spawn-lat",
            "2.0",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(validate_args(&args).is_err());

        // Only spawn-lng without spawn-lat should fail validation
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--spawn-lng",
            "3.0",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(validate_args(&args).is_err());

        // Both provided and within bbox should pass
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--spawn-lat",
            "2.0",
            "--spawn-lng",
            "3.0",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(validate_args(&args).is_ok());

        // Spawn point outside bbox should fail
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--spawn-lat",
            "5.0",
            "--spawn-lng",
            "3.0",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(validate_args(&args).is_err());
    }
}
