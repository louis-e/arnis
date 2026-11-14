//! Types and checks behind the output flags (region format, origin, border).

use crate::args::Args;

/// Java region container; the chunk NBT is the same in both.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum RegionFormat {
    /// Anvil r.X.Z.mca, read by every server and the client.
    Mca,
    /// Leaf B_Linear v3 r.X.Z.b_linear (experimental, Leaf servers only).
    Blinear,
}

/// `--origin`: a point a Web Mercator frame can be centred on.
pub fn parse_origin(s: &str) -> Result<(f64, f64), String> {
    let bad = || format!("{s}: expected LAT,LON with |LAT| <= 85 and |LON| <= 180");
    let (lat, lon) = s.split_once(',').ok_or_else(bad)?;
    let (lat, lon): (f64, f64) = (
        lat.trim().parse().map_err(|_| bad())?,
        lon.trim().parse().map_err(|_| bad())?,
    );
    if lat.abs() <= 85.0 && lon.abs() <= 180.0 {
        Ok((lat, lon))
    } else {
        Err(bad())
    }
}

impl Args {
    /// The zstd level when regions are written as B_Linear.
    pub fn blinear_level(&self) -> Option<i32> {
        (self.region_format == RegionFormat::Blinear).then_some(self.blinear_level)
    }
}

/// Checks for the output flags. `--map-item-only` and `--dh-lod-only` run before the general
/// validation (they need no area), so `main` calls this directly for them.
pub fn validate(args: &Args) -> Result<(), String> {
    if (args.map_item_only || args.dh_lod_only) && !args.one_world {
        return Err(
            "--map-item-only and --dh-lod-only need --one-world (the world at --output-dir and --world-name)."
                .to_string(),
        );
    }
    if args.origin.is_some() && !args.one_world {
        return Err("--origin only applies to --one-world.".to_string());
    }
    if args.world_border && !args.java() {
        return Err("--world-border applies to Java worlds only.".to_string());
    }
    if args.region_format == RegionFormat::Blinear {
        if !args.java() {
            return Err("--region-format blinear applies to Java worlds only.".to_string());
        }
        // One World merges into existing .mca files and nothing reads B_Linear back; the LOD
        // builders and the map preview read the Anvil regions.
        if args.one_world {
            return Err("--region-format blinear does not combine with --one-world.".to_string());
        }
        if args.voxy_lod || args.dh_lod {
            return Err(
                "--region-format blinear does not combine with --voxy-lod or --dh-lod.".to_string(),
            );
        }
    }
    Ok(())
}
