use clap::{ArgGroup, Parser};
use colored::Colorize;
use std::path::Path;
use std::process::exit;
use std::time::Duration;

/// Command-line arguments parser
#[derive(Parser, Debug)]
#[command(author, version, about)]
#[command(group(
    ArgGroup::new("location")
        .required(true)
        .args(&["bbox", "file"])
))]
pub struct Args {
    /// Bounding box of the area (min_lng,min_lat,max_lng,max_lat) (required)
    #[arg(long, allow_hyphen_values = true)]
    pub bbox: Option<String>,

    /// JSON file containing OSM data (optional)
    #[arg(long)]
    pub file: Option<String>,

    /// Path to the Minecraft world (required)
    #[arg(long, required = true)]
    pub path: String,

    /// Downloader method (requests/curl/wget) (optional)
    #[arg(long, default_value = "requests")]
    pub downloader: String,

    /// World scale to use, in blocks per meter
    #[arg(long, default_value = "1.0")]
    pub scale: f64,

    /// Ground level to use in the Minecraft world
    #[arg(long, default_value_t = -62)]
    pub ground_level: i32,

    /// Enable winter mode (default: false)
    #[arg(long, default_value_t = false)]
    pub winter: bool,

    /// Enable terrain (optional)
    #[arg(long, default_value_t = false, action = clap::ArgAction::SetFalse)]
    pub terrain: bool,
    /// Enable filling ground (optional)
    #[arg(long, default_value_t = false, action = clap::ArgAction::SetFalse)]
    pub fillground: bool,
    /// Enable debug mode (optional)
    #[arg(long, default_value_t = false, action = clap::ArgAction::SetTrue)]
    pub debug: bool,

    /// Set floodfill timeout (seconds) (optional)
    #[arg(long, value_parser = parse_duration)]
    pub timeout: Option<Duration>,

    /// Set spawn coordinates (optional)
    #[arg(long)]
    pub spawn_point: Option<String>,
}

impl Args {
    pub fn run(&self) {
        // Validating the world path
        let mc_world_path: &Path = Path::new(&self.path);
        if !mc_world_path.join("region").exists() {
            eprintln!(
                "{}",
                "Error! No Minecraft world found at the given path"
                    .red()
                    .bold()
            );
            exit(1);
        }

        // Validating bbox if provided
        if let Some(bbox) = &self.bbox {
            if !validate_bounding_box(bbox) {
                eprintln!("{}", "Error! Invalid bbox input".red().bold());
                exit(1);
            }
        }

        // Validating spawn if provided
        if let Some(spawn_point) = &self.spawn_point {
            if !validate_spawn_point(spawn_point) {
                eprintln!("{}", "Error! Invalid spawn input".red().bold());
                exit(1000); // idk what exit code to put in so hopefully this one dont get used already
            }
        }
    }
}

/// Validates the bounding box string
fn validate_bounding_box(bbox: &str) -> bool {
    let parts: Vec<&str> = bbox.split(',').collect();
    if parts.len() != 4 {
        return false;
    }

    let min_lng: f64 = parts[0].parse().ok().unwrap_or(0.0);
    let min_lat: f64 = parts[1].parse().ok().unwrap_or(0.0);
    let max_lng: f64 = parts[2].parse().ok().unwrap_or(0.0);
    let max_lat: f64 = parts[3].parse().ok().unwrap_or(0.0);

    if !(-180.0..=180.0).contains(&min_lng) || !(-180.0..=180.0).contains(&max_lng) {
        return false;
    }

    if !(-90.0..=90.0).contains(&min_lat) || !(-90.0..=90.0).contains(&max_lat) {
        return false;
    }

    min_lng < max_lng && min_lat < max_lat
}

/// Validates the spawn point
fn validate_spawn_point(spawn_point: &str) -> bool {
    let parts: Vec<&str> = spawn_point .split(',').collect();
    if parts.len() != 2 {
        return false;
    }
    let spawn_lng: f64 = parts[0].parse().ok().unwrap_or(0.0);
    let spawn_lat: f64 = parts[1].parse().ok().unwrap_or(0.0);

    if !(-180.0..=180.0).contains(&spawn_lng) || !(-90.0..=90.0).contains(&spawn_lat) {
        return false;
    }

    true
}

fn parse_duration(arg: &str) -> Result<std::time::Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(std::time::Duration::from_secs(seconds))
}
