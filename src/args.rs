use clap::{ArgGroup, Parser};
use colored::Colorize;
use std::path::Path;
use std::process::exit;

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
    #[arg(long)]
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

    /// Enable debug mode (optional)
    #[arg(long, default_value_t = false, action = clap::ArgAction::SetTrue)]
    pub debug: bool,

    /// Set floodfill timeout (seconds) (optional) // TODO
    #[arg(long, default_value_t = 2)]
    pub timeout: u64,
}

impl Args {
    pub fn run(&self) {
        // Validating the world path
        let mc_world_path: &Path = Path::new(&self.path);
        if !mc_world_path.join("region").exists() {
            eprintln!("{}", "Error! No Minecraft world found at the given path".red().bold());
            exit(1);
        }

        // Validating bbox if provided
        if let Some(bbox) = &self.bbox {
            if !validate_bounding_box(bbox) {
                eprintln!("{}", "Error! Invalid bbox input".red().bold());
                exit(1);
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

    if !(min_lng >= -180.0 && min_lng <= 180.0) || !(max_lng >= -180.0 && max_lng <= 180.0) {
        return false;
    }

    if !(min_lat >= -90.0 && min_lat <= 90.0) || !(max_lat >= -90.0 && max_lat <= 90.0) {
        return false;
    }

    min_lng < max_lng && min_lat < max_lat
}
