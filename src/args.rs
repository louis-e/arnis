use crate::bbox::BBox;
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
    #[arg(long, allow_hyphen_values = true, group = "location", value_parser = BBox::from_str)]
    pub bbox: BBox,

    /// JSON file containing OSM data (optional)
    #[arg(long, group = "location")]
    pub file: Option<String>,

    /// Path to the Minecraft world (required)
    #[arg(long)]
    pub path: String,

    /// Downloader method (requests/curl/wget) (optional)
    #[arg(long, default_value = "requests")]
    pub downloader: String,

    /// World scale to use, in blocks per meter
    #[arg(long, default_value_t = 1.0)]
    pub scale: f64,

    /// Ground level to use in the Minecraft world
    #[arg(long, default_value_t = -62)]
    pub ground_level: i32,

    /// Enable winter mode (default: false)
    #[arg(long)]
    pub winter: bool,

    /// Enable terrain (optional)
    #[arg(long)]
    pub terrain: bool,
    /// Enable filling ground (optional)
    #[arg(long, default_value_t = false, action = clap::ArgAction::SetFalse)]
    pub fillground: bool,
    /// Enable debug mode (optional)
    #[arg(long)]
    pub debug: bool,

    /// Set floodfill timeout (seconds) (optional)
    #[arg(long, value_parser = parse_duration)]
    pub timeout: Option<Duration>,
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
    }
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
        // Test that winter/terrain/debug are SetTrue
        let cmd = [
            "arnis",
            "--path",
            "",
            "--bbox",
            "54.627053 9.927928 54.634902 9.937563",
            "--winter",
            "--terrain",
            "--debug",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(args.winter);
        assert!(args.debug);
        assert!(args.terrain);

        let cmd = ["arnis", "--path", "", "--bbox", "54.627053 9.927928 54.634902 9.937563"];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.winter);
        assert!(!args.debug);
        assert!(!args.terrain);
    }

    #[test]
    fn test_required_options() {
        let cmd = ["arnis"];
        assert!(Args::try_parse_from(cmd.iter()).is_err());

        let cmd = ["arnis", "--path", "", "--bbox", "54.627053 9.927928 54.634902 9.937563"];
        assert!(Args::try_parse_from(cmd.iter()).is_ok());

        // let cmd = [
        //     "arnis",
        //     "--gui",
        // ];
        // assert!(Args::try_parse_from(cmd.iter()).is_ok());

        let cmd = [
            "arnis", // "--gui",
            "--path", "", "--bbox", "54.627053 9.927928 54.634902 9.937563",
        ];
        assert!(Args::try_parse_from(cmd.iter()).is_ok());
    }
}
