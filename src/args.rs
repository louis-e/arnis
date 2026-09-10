use crate::coordinate_system::geographic::LLBBox;
use clap::{ArgAction, Parser};
use std::path::PathBuf;
use std::time::Duration;

/// Command-line arguments parser
#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Args {
    /// Bounding box of the area (min_lat,min_lng,max_lat,max_lng).
    /// Required unless --file supplies a local .osm/.xml file to derive it from
    /// (terrain-only mode ignores --file, so it always requires --bbox).
    #[arg(long, allow_hyphen_values = true, value_parser = LLBBox::from_str)]
    pub bbox: Option<LLBBox>,

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

    /// Generate a Luanti/Minetest world (map.sqlite) instead of Java Edition
    #[arg(long)]
    pub luanti: bool,

    /// Downloader method (requests/curl/wget) (optional)
    #[arg(long, default_value = "requests")]
    pub downloader: String,

    /// World scale to use, in blocks per meter (1.0 = real size).
    /// Ignored for --body moon/mars, which use their own fixed scale.
    #[arg(long, default_value_t = 1.0, allow_hyphen_values = true, value_parser = parse_scale)]
    pub scale: f64,

    /// Celestial body to generate. moon and mars use NASA PDS elevation at a fixed
    /// low scale and have no OSM data, so every object option is ignored.
    #[arg(long, value_enum, default_value_t = crate::celestial::CelestialBody::Earth)]
    pub body: crate::celestial::CelestialBody,

    /// Projection mode for coordinate mapping.
    /// local: each generation starts at Minecraft (0,0). The only supported mode.
    #[arg(long, default_value = "local")]
    pub projection: crate::projection::ProjectionKind,

    /// Ground level to use in the Minecraft world
    #[arg(long, default_value_t = -62, allow_hyphen_values = true)]
    pub ground_level: i32,

    /// What to generate, mirroring the GUI's generation mode dropdown:
    /// geo-terrain: OSM objects on real elevation terrain (default)
    /// geo-only: OSM objects on flat ground
    /// terrain-only: real elevation terrain, no OSM or Overture objects (--overture has no effect)
    #[arg(long, value_enum, default_value_t = GenerationMode::GeoTerrain)]
    pub mode: GenerationMode,

    /// Deprecated: terrain is on by default, use --mode geo-only for flat ground.
    /// Accepted as a no-op so existing scripts keep working.
    #[arg(long = "terrain", hide = true)]
    pub legacy_terrain: bool,

    /// Enable interior generation (optional, off unless requested)
    #[arg(long, default_value_t = false, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub interior: bool,

    /// Enable filling ground (optional)
    #[arg(long, default_value_t = false)]
    pub fillground: bool,

    /// Use the legacy procedural trees instead of the bundled schematic tree pack.
    /// Schematic trees are on by default; this flag opts out.
    #[arg(long, default_value_t = false)]
    pub legacy_trees: bool,

    /// Largest schematic tree to place: small (<=6 blocks), medium (<=12),
    /// big (<=20), tall (<=28) or giant. Oversized picks fall back to a smaller
    /// species in the same community where there is one.
    #[arg(long, value_enum, default_value_t = crate::trees::tree_library::TreeSize::Giant)]
    pub max_tree_size: crate::trees::tree_library::TreeSize,

    /// Place trees from the Meta/WRI global canopy height map instead of assuming
    /// every tree-cover cell is forest. Land cover still decides the surface.
    #[arg(long = "canopy-height", default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub canopy_height: bool,

    /// Add building footprints from Overture Maps that are missing in OpenStreetMap.
    /// Helps sparsely mapped areas; may occasionally add a satellite-detected false positive.
    #[arg(long = "overture", default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub overture: bool,

    /// Which Overture transport to read. Both carry the same buildings.
    /// auto: vector tiles, falling back to the Parquet partitions (default)
    /// tiles: vector tiles only, so a fallback cannot hide a broken archive
    /// parquet: the GeoParquet partitions only
    #[arg(long = "overture-source", value_enum, default_value_t = OvertureSource::Auto)]
    pub overture_source: OvertureSource,

    /// Disable both external 3D models (3DMR + Wikimedia) and bundled schematic
    /// props (cars, boats, cranes, ...) with a single toggle.
    #[arg(long = "no-3d", default_value_t = true, action = ArgAction::SetFalse)]
    pub use_3d: bool,

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

    /// Clockwise rotation angle in degrees (optional, range: -90 to 90)
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub rotation: f64,

    /// Extend build height via a bundled pack (Java 1.21.4+: Y=-2032..2031;
    /// Bedrock 1.21.40+: Y=-512..512). Both are experimental.
    #[arg(long, default_value_t = false)]
    pub disable_height_limit: bool,

    /// Use only the legacy AWS Terrain Tiles source (~30m) instead of
    /// Mapterhorn and the regional high-resolution providers.
    #[arg(long, default_value_t = false)]
    pub aws_only_elevation: bool,

    /// Print generation-only timing to stderr (excludes data fetching)
    #[arg(long, hide = true)]
    pub benchmark: bool,

    /// Bake per-chunk lighting so distant chunks render lit in LOD mods
    /// (Voxy/Chunky) without visiting them. Slower; off by default.
    #[arg(long, default_value_t = false)]
    pub bake_lighting: bool,

    /// Pre-generate the Voxy mod's LOD cache so the world renders to the horizon
    /// on first join, instead of needing `/voxy import current`. Java only;
    /// implies --bake-lighting, since unlit LOD terrain renders black.
    #[arg(long, default_value_t = false)]
    pub voxy_lod: bool,

    /// Render a top-down PNG map preview of the generated world (Java and Bedrock)
    #[arg(long, default_value_t = false)]
    pub map_preview: bool,

    /// Give the player a locked map item showing the whole world (Java only)
    #[arg(long = "map-item", default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub map_item: bool,

    /// Player game mode for the generated world
    #[arg(long, value_enum, default_value_t = GameMode::Creative)]
    pub gamemode: GameMode,

    /// Initial time of day in ticks (0 = dawn, 6000 = noon, 18000 = midnight)
    #[arg(long, default_value_t = DEFAULT_WORLD_TIME, value_parser = clap::value_parser!(i64).range(0..24000))]
    pub world_time: i64,

    /// Readable image signs, Java only. `basic` covers public signage: street names,
    /// traffic signs, transit stops, information boards and billboards. `full` adds
    /// building signage: shop name plates, house numbers and crossing signs.
    #[arg(long, value_enum, default_value_t = SignageLevel::Basic)]
    pub signage: SignageLevel,

    /// Mapillary API token, from https://www.mapillary.com/developer. Required by
    /// --mapillary-facades and --mapillary-probe.
    ///
    /// `hide_env_values` because clap prints `[env: NAME=value]` in `--help` by
    /// default, and this value is a credential: `arnis --help` with the variable
    /// set would put the token on stdout, which is where a bug report or a CI log
    /// picks it up. The variable's name still shows, so the help still says where
    /// the token can come from.
    #[arg(long, env = "MAPILLARY_TOKEN", hide_env_values = true)]
    pub mapillary_token: Option<String>,

    /// Build building facades from Mapillary street-level photographs: wall colours
    /// per floor band everywhere, and windows and doors from the photographs from
    /// two blocks per metre up. Explicit OSM material and colour tags still win.
    /// On by default once a token is available, so the token alone is enough; pass
    /// `--mapillary-facades false` to keep the token and skip the work. Imagery and
    /// finished walls are cached, so a second world over the same area is quick.
    #[arg(long, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub mapillary_facades: Option<bool>,

    /// Run the Mapillary facade sampler, print what it found and exit without
    /// generating a world. Use this to check coverage for an area.
    #[arg(long, default_value_t = false)]
    pub mapillary_probe: bool,

    /// With --mapillary-probe, write one PNG per sampled building showing the
    /// reconstructed facade grids. Implies the sampler keeps its per-wall grids.
    #[arg(long)]
    pub mapillary_debug_dir: Option<PathBuf>,

    /// Directory written by orthofacade's export_arnis.py: facade textures keyed
    /// by OSM way. Buildings found in it are built as plain walls carrying the
    /// texture; every other building is generated as usual. Overrides the fetch,
    /// which is how the Python review loop is driven; without it the pipeline
    /// runs for the world's own bbox and uses its own export.
    #[arg(long)]
    pub mapillary_facades_dir: Option<PathBuf>,

    /// Dump what each wall was built from (the loose crops, the per-view
    /// textures, the fused texture, the block grid and the gate table) into this
    /// directory, so a wall can be inspected instead of guessed at.
    #[arg(long)]
    pub mapillary_facade_debug_dir: Option<PathBuf>,

    /// With --mapillary-facade-debug-dir: the wall keys to dump, comma separated
    /// (for example w81190155_3). Empty dumps every wall that gets a texture,
    /// which is hundreds of megabytes on a city.
    #[arg(long, default_value = "")]
    pub mapillary_facade_debug_walls: String,

    /// How facade textures are applied: `photos` builds colour-matched blocks
    /// and hangs the photograph itself as one item display entity per wall
    /// face on the wall's true line, so a diagonal wall gets one flat quad
    /// rather than a staircase of axis-aligned panels (Java 1.21.4+ only,
    /// carried by the world's resource pack); `blocks` picks the palette block
    /// nearest each cell's colour and places windows and doors from the
    /// classes, and works on every world format.
    #[arg(long, value_enum, default_value_t = FacadeMode::Photos)]
    pub mapillary_facade_mode: FacadeMode,

    /// Hang a premade facade photograph on every building, picked by what kind
    /// of building it is. Needs no token and no download, so it covers the
    /// buildings street photography never reached. The two facade sources are
    /// alternatives: this one turns the Mapillary facades off, since both hang
    /// panels on the same walls. Java 1.21.4+ only (item display entities).
    #[arg(long, default_value_t = false)]
    pub building_facades: bool,

    /// How sharp the facade panels may be, by how large an atlas they may fill.
    /// `standard` fits any GPU the game runs on; `high` is sharper and wants a
    /// modern one on whatever machine opens the world.
    #[arg(long, value_enum, default_value_t = FacadeDetail::Standard)]
    pub facade_detail: FacadeDetail,

    /// Texture resolution of the facade panels in pixels per block (4, 8, 16 or
    /// 32), for the Mapillary photos and the preset facades alike. Lowered
    /// automatically when the panels would not fit the atlas --facade-detail
    /// allows.
    #[arg(long, default_value_t = 16, value_parser = parse_facade_px)]
    pub facade_px: u32,

    /// Directory holding the preset facade set: a manifest.json and the images
    /// it names. The set is compiled in, so this only has to be given to run a
    /// replacement one.
    #[arg(long)]
    pub building_facades_dir: Option<PathBuf>,
}

/// Accepts the panel resolutions the atlas budget logic can halve cleanly.
fn parse_facade_px(s: &str) -> Result<u32, String> {
    match s.trim().parse::<u32>() {
        Ok(v) if matches!(v, 4 | 8 | 16 | 32) => Ok(v),
        _ => Err(format!("{s}: --facade-px must be 4, 8, 16 or 32")),
    }
}

/// Which transport to read Overture Maps buildings through.
///
/// Both carry the same release's data; they differ in cost. The tile archive is
/// one range request per z14 tile and is cached per release, so a city is about
/// 1.3 MB and a repeat run is free. The Parquet partitions need a ~233 KB
/// catalogue and a ~1.3 MB footer per partition before any building is read, but
/// they win on continental areas, where whole row groups beat one request per
/// square kilometre.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum OvertureSource {
    /// Tiles where they are cheaper and available, Parquet otherwise.
    #[default]
    Auto,
    /// Vector tiles only. Fails rather than falling back, so a broken archive is
    /// visible instead of merely slow.
    Tiles,
    /// GeoParquet partitions only.
    Parquet,
}

/// How much of the graphics card the facade panels are allowed to ask for.
///
/// Minecraft stitches every block texture into one atlas, and overflowing it
/// makes the game drop the world's whole resource pack and switch off the
/// player's other packs with it. `Standard` budgets for an 8192 atlas, which
/// every card the game runs on can hold. `High` budgets for 16384, which about
/// nine cards in ten manage, and spends up to 716 MB of video memory on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum FacadeDetail {
    #[default]
    Standard,
    High,
}

impl FacadeDetail {
    /// The atlas side this detail level budgets against, in pixels.
    pub fn atlas_side(self) -> u32 {
        match self {
            FacadeDetail::Standard => crate::mapillary::atlas::ATLAS_SIDE_STANDARD,
            FacadeDetail::High => crate::mapillary::atlas::ATLAS_SIDE_HIGH,
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "high" => FacadeDetail::High,
            _ => FacadeDetail::Standard,
        }
    }
}

/// How much image signage to place.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum SignageLevel {
    None,
    #[default]
    Basic,
    Full,
}

impl SignageLevel {
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "none" => SignageLevel::None,
            "full" => SignageLevel::Full,
            _ => SignageLevel::Basic,
        }
    }

    pub fn enabled(self) -> bool {
        self != SignageLevel::None
    }

    pub fn full(self) -> bool {
        self == SignageLevel::Full
    }
}

/// Generation mode, matching the GUI's dropdown (src/gui/js/main.js).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum GenerationMode {
    /// OSM objects on real elevation terrain
    #[default]
    GeoTerrain,
    /// OSM objects on flat ground
    GeoOnly,
    /// Real elevation terrain without any OSM or Overture objects
    TerrainOnly,
}

impl GenerationMode {
    /// Whether real elevation is fetched and applied instead of flat ground.
    pub fn terrain(self) -> bool {
        !matches!(self, GenerationMode::GeoOnly)
    }

    /// Whether OSM and Overture objects are skipped entirely.
    pub fn skip_objects(self) -> bool {
        matches!(self, GenerationMode::TerrainOnly)
    }
}

/// Below this scale, OSM objects stop being representable: road half-widths floor at 1
/// (so every road is >= 3 blocks = 10 m across at 0.3), buildings collapse into 1x1x3
/// pillars, and fixed-size props come to dominate the scene. Objects are skipped instead.
pub const OBJECT_SKIP_SCALE: f64 = 0.3;

/// Smallest usable scale. Below this a country-sized bbox degenerates into a few hundred
/// blocks and terrain detail is lost entirely.
pub const MIN_SCALE: f64 = 0.05;
/// Largest usable scale. Beyond this a single square kilometer already costs GBs and hours.
pub const MAX_SCALE: f64 = 4.0;

/// Rejects NaN, infinities and out-of-range scales. Used by both the CLI parser and the
/// GUI entry point so an invalid scale can never reach the (expensive) fetch stage.
pub fn validate_scale(scale: f64) -> Result<(), String> {
    if !scale.is_finite() {
        return Err("World scale must be a finite number.".to_string());
    }
    if !(MIN_SCALE..=MAX_SCALE).contains(&scale) {
        return Err(format!(
            "World scale must be between {MIN_SCALE} and {MAX_SCALE} (got {scale})."
        ));
    }
    Ok(())
}

fn parse_scale(arg: &str) -> Result<f64, String> {
    let scale: f64 = arg
        .parse()
        .map_err(|_| format!("`{arg}` is not a number"))?;
    validate_scale(scale)?;
    Ok(scale)
}

impl Args {
    /// Whether this run uses real elevation terrain rather than flat ground.
    pub fn terrain(&self) -> bool {
        self.mode.terrain()
    }

    /// Whether this run skips OSM/Overture objects (terrain-only, or too small a scale).
    pub fn skip_objects(&self) -> bool {
        self.mode.skip_objects() || self.scale < OBJECT_SKIP_SCALE
    }

    /// Whether objects are being skipped only because the scale is below `OBJECT_SKIP_SCALE`.
    pub fn skip_objects_due_to_scale(&self) -> bool {
        !self.mode.skip_objects() && self.scale < OBJECT_SKIP_SCALE
    }

    /// The Mapillary token, if one was given and is not blank.
    ///
    /// Blank is treated as absent because the flag reads `MAPILLARY_TOKEN` from
    /// the environment, where an empty variable is a common way of unsetting one.
    pub fn mapillary_api_token(&self) -> Option<&str> {
        self.mapillary_token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
    }

    /// Whether this run builds facades from Mapillary photographs.
    ///
    /// A token alone turns the feature on: there is nothing else to configure,
    /// and asking for a second flag would only be a way to get it wrong.
    /// The preset facades take the walls: the two sources are alternatives, not
    /// layers, so asking for the presets switches the download off rather than
    /// letting the pair fight over a wall and a resource pack. A token in the
    /// environment is often ambient rather than deliberate, so this is quiet
    /// precedence and not a refusal; `validate_args` says so once when both
    /// were actually asked for.
    pub fn mapillary_facades_on(&self) -> bool {
        !self.building_facades
            && self.mapillary_facades.unwrap_or(true)
            && self.mapillary_api_token().is_some()
    }

    /// Whether the facade pipeline itself runs for this world.
    ///
    /// A folder given on the command line overrides the fetch: it is how the
    /// Python review loop puts its own export in front of the generator.
    pub fn mapillary_pipeline_on(&self) -> bool {
        self.mapillary_facades_on() && self.mapillary_facades_dir.is_none()
    }

    /// Whether facades are wanted at all, from either source.
    pub fn mapillary_facades_wanted(&self) -> bool {
        self.mapillary_facades_on() || self.mapillary_facades_dir.is_some()
    }
}

/// How an orthofacade texture is put onto a building.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum FacadeMode {
    /// Colour-matched wall blocks, with windows and doors from the texture.
    Blocks,
    /// Blocks plus the photograph itself, one item display entity per wall
    /// face hung on the wall's true line rather than on the block grid. Java
    /// 1.21.4+ only.
    ///
    /// The aliases are the names this mode and its predecessor went by while
    /// a `paintings` mode hung painting entities beside it. A script written
    /// back then should still get photographs, not an error.
    #[value(alias = "paintings-v2", alias = "paintings2", alias = "paintings")]
    Photos,
}

impl FacadeMode {
    /// Reads the mode the GUI stored or a settings file carries.
    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "blocks" => FacadeMode::Blocks,
            // The photo panels were `paintings-v2` while a `paintings` mode
            // hung painting entities beside them. Both names are gone from
            // the choices, but a setting saved by an earlier build still says
            // one of them, and it should keep giving photographs.
            "paintings" | "paintings-v2" | "paintings_v2" | "paintings2" | "paintingsv2" => {
                FacadeMode::Photos
            }
            // `photos`, and anything else an older build left behind, which
            // takes the default.
            _ => FacadeMode::Photos,
        }
    }

    /// Whether the texture is hung as item display entities.
    pub fn places_displays(self) -> bool {
        matches!(self, FacadeMode::Photos)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum GameMode {
    Survival,
    Creative,
    Spectator,
}

impl GameMode {
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "survival" => GameMode::Survival,
            "spectator" => GameMode::Spectator,
            _ => GameMode::Creative,
        }
    }

    pub fn java_game_type(self) -> i32 {
        match self {
            GameMode::Survival => 0,
            GameMode::Creative => 1,
            GameMode::Spectator => 3,
        }
    }

    pub fn bedrock_game_type(self) -> i32 {
        match self {
            GameMode::Survival => 0,
            GameMode::Creative => 1,
            GameMode::Spectator => 6,
        }
    }
}

/// Validates CLI arguments after parsing.
/// For Java Edition: `--path` is required. If the directory doesn't exist, it will be created.
/// For Bedrock Edition (`--bedrock`): `--path` is optional (defaults to Desktop output).
/// Non-Earth bodies drive their own scale and force terrain-only. Shared by the
/// CLI and the GUI so the two cannot disagree about what Moon/Mars means.
pub fn apply_body_defaults(args: &mut Args) {
    if args.body.is_earth() {
        return;
    }
    args.scale = args.body.world_scale();
    args.mode = GenerationMode::TerrainOnly;
    args.overture = false;
    args.canopy_height = false;
    args.use_3d = false;
    args.interior = false;
    args.legacy_trees = false;
    args.aws_only_elevation = false;
    // Terrain only means no walls, so neither facade source has anything to
    // hang a photograph on. Cleared rather than left set so the run does not
    // pay for a Mapillary fetch or a resource pack it cannot use.
    args.building_facades = false;
    args.mapillary_facades = Some(false);
    // Relief already fits vanilla height, so the pack would add only empty sky.
    args.disable_height_limit = false;
    // Airless bodies look right at night. Only a default: comparing against the
    // flag's own default lets an explicit --world-time win.
    if args.world_time == DEFAULT_WORLD_TIME {
        args.world_time = MIDNIGHT_TICKS;
    }
}

/// Clap's `--world-time` default, so `apply_body_defaults` can tell left-alone
/// from explicitly-set.
pub const DEFAULT_WORLD_TIME: i64 = 6_000;
/// Minecraft tick for midnight (tick 0 is 06:00).
pub const MIDNIGHT_TICKS: i64 = 18_000;

pub fn validate_args(args: &Args) -> Result<(), String> {
    // Moon/Mars scale is ours, and sits below MIN_SCALE by design.
    if args.body.is_earth() {
        validate_scale(args.scale)?;
    }

    if args.bedrock && args.luanti {
        return Err("Cannot use --bedrock and --luanti together.".to_string());
    }

    // The legacy --terrain flag is redundant now that terrain is the default, but it must
    // not silently lose against a mode that asks for flat ground.
    if args.legacy_terrain && args.mode == GenerationMode::GeoOnly {
        return Err(
            "--terrain contradicts --mode geo-only (flat ground). Drop --terrain, or use --mode geo-terrain."
                .to_string(),
        );
    }

    if args.map_preview && args.luanti {
        return Err("--map-preview is not supported for Luanti worlds.".to_string());
    }

    // Never shipped working: X gets a cos(lat) factor and Z does not, so the world comes out
    // stretched north-south by 1/cos(lat) against both the elevation grid and its own
    // east-west scale. Still parsed so an old command line gets this instead of a parse error.
    if args.projection == crate::projection::ProjectionKind::WebMercator {
        return Err(
            "--projection web_mercator was experimental and never worked: it stretches the world north-south by 1/cos(latitude) (about 1.5x at 47 degrees), so objects come out elongated and misaligned with the terrain. Use --projection local."
                .to_string(),
        );
    }

    // Caught here rather than at the fetch, which only runs after the OSM
    // download: a missing token should not cost the user that wait first. Only
    // an explicit ask is an error, because the feature otherwise simply follows
    // the token and a run without one is a run without facades.
    if (args.mapillary_facades == Some(true) || args.mapillary_probe)
        && args.mapillary_api_token().is_none()
    {
        return Err(
            "--mapillary-facades and --mapillary-probe need --mapillary-token (or MAPILLARY_TOKEN). \
             Get one free at https://www.mapillary.com/developer."
                .to_string(),
        );
    }

    if args.mapillary_debug_dir.is_some() && !args.mapillary_probe {
        return Err("--mapillary-debug-dir only applies to --mapillary-probe.".to_string());
    }

    if args.mapillary_facade_debug_dir.is_none() && !args.mapillary_facade_debug_walls.is_empty() {
        return Err(
            "--mapillary-facade-debug-walls needs --mapillary-facade-debug-dir.".to_string(),
        );
    }

    if args.mapillary_facade_debug_dir.is_some() && !args.mapillary_pipeline_on() {
        return Err(
            "--mapillary-facade-debug-dir dumps what the pipeline built, so it needs a token \
             and no --mapillary-facades-dir."
                .to_string(),
        );
    }

    if let Some(dir) = &args.mapillary_facades_dir {
        if !dir.is_dir() {
            return Err(format!(
                "--mapillary-facades-dir: {} is not a directory.",
                dir.display()
            ));
        }
    }

    // The photo panels are Java entities carried by a resource pack, so no
    // other world format can show them. Said here rather than dropped silently
    // at the wall, and checked for the fetch too, not only for a facade folder.
    if args.mapillary_facades_wanted()
        && (args.bedrock || args.luanti)
        && args.mapillary_facade_mode.places_displays()
    {
        return Err(
            "--mapillary-facade-mode photos needs a Java world (item display entities, 1.21.4+). \
             Use `blocks`, which works on every world format."
                .to_string(),
        );
    }

    // The two facade sources hang on the same walls, so the presets take them
    // and the Mapillary facades stand down for the run. Said out loud, since a
    // token the user went and created would otherwise be ignored in silence.
    if args.building_facades
        && (args.mapillary_api_token().is_some() || args.mapillary_facades_dir.is_some())
    {
        println!(
            "Note: --building-facades takes the walls, so the Mapillary facades are off for this run."
        );
    }

    // The preset facades hang on the same item display entities, so they are
    // refused here for the same reason and in the same words. Said out loud
    // rather than dropped at the wall.
    if args.building_facades && (args.bedrock || args.luanti) {
        return Err(
            "--building-facades needs a Java world (item display entities, 1.21.4+). \
             Leave it off and the buildings are generated with their usual block walls."
                .to_string(),
        );
    }

    if let Some(dir) = &args.building_facades_dir {
        if !dir.is_dir() {
            return Err(format!(
                "--building-facades-dir: {} is not a directory.",
                dir.display()
            ));
        }
    }

    // A bounding box is required unless a local --file supplies one to derive it from.
    // Terrain-only mode ignores --file (it never loads OSM objects), so it always needs --bbox.
    if args.bbox.is_none() {
        if args.skip_objects() {
            return Err(
                "--mode terrain-only requires --bbox (a local --file is ignored in terrain-only mode)."
                    .to_string(),
            );
        }
        if args.file.is_none() {
            return Err(
                "Provide --bbox, or --file with a local .osm/.xml file to derive the bounding box from."
                    .to_string(),
            );
        }
    }

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
    } else if args.luanti {
        // Luanti: path optional, defaults to OS Luanti worlds dir
        if let Some(ref path) = args.path {
            if !path.exists() {
                return Err(format!("Path does not exist: {}", path.display()));
            }
            if !path.is_dir() {
                return Err(format!("Path is not a directory: {}", path.display()));
            }
        }
    } else if args.mapillary_probe {
        // The probe writes no world, so it needs no output directory.
    } else {
        // Java: path is required. If it exists, it must be a directory.
        // If it doesn't exist, create_new_world will create it.
        match &args.path {
            None => {
                return Err(
                    "The --output-dir argument is required for Java Edition. Provide the directory where the world should be created. Use --bedrock for Bedrock Edition output."
                        .to_string(),
                );
            }
            Some(ref path) => {
                if path.exists() && !path.is_dir() {
                    return Err(format!(
                        "Path exists but is not a directory: {}",
                        path.display()
                    ));
                }
                // If path doesn't exist, that's OK - create_new_world will create it
            }
        }
    }

    // Validate spawn point: both or neither must be provided
    match (args.spawn_lat, args.spawn_lng) {
        (Some(_), None) | (None, Some(_)) => {
            return Err("Both --spawn-lat and --spawn-lng must be provided together.".to_string());
        }
        (Some(lat), Some(lng)) => {
            // Validate coordinates are valid lat/lng (rejects NaN, inf, out-of-range)
            use crate::coordinate_system::geographic::LLPoint;
            let llpoint =
                LLPoint::new(lat, lng).map_err(|e| format!("Invalid spawn coordinates: {e}"))?;

            // Validate that spawn point is within the bounding box. Only enforceable when the
            // bbox is known up front; a file-derived bbox isn't available until the file is parsed.
            if let Some(bbox) = args.bbox {
                if !bbox.contains(&llpoint) {
                    return Err(
                        "Spawn point (--spawn-lat, --spawn-lng) must be within the bounding box."
                            .to_string(),
                    );
                }
            }
        }
        _ => {}
    }

    // Validate rotation angle range (also rejects NaN and infinity)
    if !args.rotation.is_finite() || args.rotation < -90.0 || args.rotation > 90.0 {
        return Err("Rotation angle must be between -90 and 90 degrees.".to_string());
    }

    let (floor, ceiling) = ground_level_bounds(args);
    if args.ground_level < floor || args.ground_level > ceiling {
        return Err(format!(
            "--ground-level must be between {floor} and {ceiling} for this world format (got {}).",
            args.ground_level
        ));
    }

    Ok(())
}

/// Legal `--ground-level` range for the chosen output format. The scaler clamps every column
/// into `[ground_level, ceiling]`, so a base above the ceiling makes that clamp's min exceed
/// its max and aborts the run after the whole elevation download.
fn ground_level_bounds(args: &Args) -> (i32, i32) {
    let floor = crate::ground::extended_min_y_for(args) + 2;
    let ceiling =
        crate::ground::world_top_y_for(args) - crate::elevation::postprocess::TERRAIN_HEIGHT_BUFFER;
    (floor, ceiling)
}

fn parse_duration(arg: &str) -> Result<std::time::Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(std::time::Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generation_mode() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let base = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];

        let parse = |extra: &[&str]| {
            let mut cmd: Vec<&str> = base.to_vec();
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };

        // geo-terrain: objects on real elevation (the default)
        let args = parse(&["--mode", "geo-terrain"]);
        assert_eq!(args.mode, GenerationMode::GeoTerrain);
        assert!(args.terrain());
        assert!(!args.skip_objects());
        assert!(validate_args(&args).is_ok());

        // geo-only: objects on flat ground
        let args = parse(&["--mode", "geo-only"]);
        assert!(!args.terrain());
        assert!(!args.skip_objects());
        assert!(validate_args(&args).is_ok());

        // terrain-only: elevation, no objects
        let args = parse(&["--mode", "terrain-only"]);
        assert!(args.terrain());
        assert!(args.skip_objects());
        assert!(validate_args(&args).is_ok());

        // The legacy --terrain flag contradicts flat ground, so it must not pass silently
        let args = parse(&["--mode", "geo-only", "--terrain"]);
        assert!(validate_args(&args).is_err());

        // Unknown modes are rejected by clap
        let mut cmd: Vec<&str> = base.to_vec();
        cmd.extend_from_slice(&["--mode", "objects"]);
        assert!(Args::try_parse_from(cmd.iter()).is_err());
    }

    #[test]
    fn web_mercator_projection_is_rejected() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };

        assert!(validate_args(&parse(&["--projection", "local"])).is_ok());
        let err = validate_args(&parse(&["--projection", "web_mercator"])).unwrap_err();
        assert!(err.contains("--projection local"), "unhelpful error: {err}");
        // Not just the terrain path: geo-only is distorted too.
        assert!(
            validate_args(&parse(&["--projection", "mercator", "--mode", "geo-only"])).is_err()
        );
    }

    #[test]
    fn ground_level_is_bounded_by_the_world_ceiling() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };

        let gl = |flags: &[&str], level: i32| {
            let mut args = parse(flags);
            args.ground_level = level;
            validate_args(&args)
        };

        assert!(validate_args(&parse(&[])).is_ok());
        // Vanilla ceiling is 319 - 15; above it the scaler's clamp has min > max and panics.
        assert!(gl(&[], 304).is_ok());
        assert!(gl(&[], 305).is_err());
        assert!(gl(&[], 400).is_err());
        // Below the bedrock plane there is nothing to stand on.
        assert!(gl(&[], -63).is_err());

        // The tall datapack raises the ceiling to 2031; Bedrock's pack stops far below it.
        let dhl: &[&str] = &["--disable-height-limit"];
        assert!(gl(dhl, 2016).is_ok());
        assert!(gl(dhl, 2017).is_err());
        assert!(gl(dhl, -2030).is_ok());
        assert!(gl(dhl, -2031).is_err());
        assert!(gl(&["--bedrock", "--disable-height-limit"], 500).is_err());
    }

    #[test]
    fn formats_without_an_extended_dimension_keep_the_vanilla_ground_level_range() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let gl = |flags: &[&str], level: i32| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(flags);
            let mut args = Args::parse_from(cmd.iter());
            args.ground_level = level;
            validate_args(&args)
        };

        for flags in [
            &["--luanti"][..],
            &["--luanti", "--disable-height-limit"][..],
            &["--bedrock"][..],
        ] {
            assert!(gl(flags, -62).is_ok(), "{flags:?} rejected the default");
            assert!(gl(flags, 304).is_ok(), "{flags:?} rejected 304");
            assert!(gl(flags, 305).is_err(), "{flags:?} accepted 305");
            assert!(gl(flags, -63).is_err(), "{flags:?} accepted -63");
        }

        // Bedrock's behavior pack raises the ceiling to 511 but leaves the floor vanilla.
        let bedrock_dhl: &[&str] = &["--bedrock", "--disable-height-limit"];
        assert!(gl(bedrock_dhl, 496).is_ok());
        assert!(gl(bedrock_dhl, 497).is_err());
        assert!(gl(bedrock_dhl, -63).is_err());
    }

    #[test]
    fn test_flags() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        // The legacy --terrain flag still parses and still yields terrain (now the default)
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
        assert!(args.legacy_terrain);
        assert!(args.terrain());
        assert!(validate_args(&args).is_ok());

        let cmd = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.debug);
        // Terrain is on by default, matching the GUI's "Objects + Terrain" mode
        assert_eq!(args.mode, GenerationMode::GeoTerrain);
        assert!(args.terrain());
        assert!(!args.skip_objects());
        assert!(!args.legacy_terrain);
        assert!(!args.bedrock);
        assert!(!args.disable_height_limit);
        assert!(!args.bake_lighting);
        assert!(!args.voxy_lod);
        assert!(!args.map_preview);
        assert_eq!(args.signage, SignageLevel::Basic);
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--signage",
            "none",
        ];
        assert!(!Args::parse_from(cmd.iter()).signage.enabled());
        // interior is opt-in (off by default); overture defaults to true
        assert!(!args.interior);
        assert!(args.overture);
    }

    #[test]
    fn test_bool_flags_can_be_disabled() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        // Test disabling interior/overture with =false
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--interior=false",
            "--overture=false",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.interior);
        assert!(!args.overture);

        // Test enabling with bare flag (no value)
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--interior",
            "--overture",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(args.interior);
        assert!(args.overture);
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
    fn test_disable_height_limit_flag() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        // Default is false
        let cmd = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.disable_height_limit);

        // Flag enables it
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--disable-height-limit",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(args.disable_height_limit);
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
    fn test_java_nonexistent_path_is_ok() {
        // Java: nonexistent paths are OK - create_new_world will create them
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("does_not_exist");
        let cmd = [
            "arnis",
            "--output-dir",
            nonexistent.to_str().unwrap(),
            "--bbox",
            "1,2,3,4",
        ];
        let args = Args::parse_from(cmd.iter());
        let result = validate_args(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_java_path_exists_but_is_file_fails() {
        // Java: if path exists but is a file, fail
        let tmpfile = tempfile::NamedTempFile::new().unwrap();
        let tmp_path = tmpfile.path().to_str().unwrap();

        let cmd = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::parse_from(cmd.iter());
        let result = validate_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a directory"));
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

        // `arnis` alone now parses (--bbox is optional at the CLI layer), but validation
        // rejects it: no output dir, and neither --bbox nor --file.
        let cmd = ["arnis"];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&args).is_err());

        let cmd = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&args).is_ok());

        // Verify --path still works as a deprecated alias
        let cmd = ["arnis", "--path", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&args).is_ok());

        // #881: --file without --bbox is accepted; the bbox is derived from the file later.
        let cmd = ["arnis", "--output-dir", tmp_path, "--file", "area.osm"];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&args).is_ok());

        // Neither --bbox nor --file (even with an output dir) is a validation error.
        let cmd = ["arnis", "--output-dir", tmp_path];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&args).is_err());

        // The --gui flag isn't used here, ugh. TODO clean up main.rs and its argparse usage.
        // let cmd = ["arnis", "--gui"];
        // assert!(Args::try_parse_from(cmd.iter()).is_ok());
    }

    #[test]
    fn test_terrain_only_requires_bbox_even_with_file() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        // terrain-only ignores --file, so a bbox is still required.
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--file",
            "area.osm",
            "--mode",
            "terrain-only",
        ];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&args).is_err());

        // With --bbox it validates.
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--mode",
            "terrain-only",
        ];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&args).is_ok());
    }

    /// `--help` must not print the Mapillary token.
    ///
    /// clap renders `[env: NAME=value]` for an env-backed argument unless
    /// `hide_env_values` is set, so without it `MAPILLARY_TOKEN=MLY|... arnis
    /// --help` puts the credential on stdout. The env var is read here rather
    /// than set, because setting one is process wide and every other test in
    /// this binary shares the process.
    #[test]
    fn the_help_names_the_token_variable_and_never_its_value() {
        use clap::CommandFactory;
        let cmd = Args::command();
        let token = cmd
            .get_arguments()
            .find(|a| a.get_id() == "mapillary_token")
            .expect("the token argument");
        assert!(
            token.is_hide_env_values_set(),
            "--help would print the value of MAPILLARY_TOKEN"
        );
        // And the variable's name is still shown, so the help still says where
        // a token may come from.
        assert!(!token.is_hide_env_set());
        let help = Args::command().render_long_help().to_string();
        assert!(
            help.contains("[env: MAPILLARY_TOKEN]"),
            "the help should name the variable without its value"
        );
        assert!(!help.contains("MAPILLARY_TOKEN="));
    }

    #[test]
    fn facade_modes_parse_and_need_a_java_world() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        // The facade directory has to exist for the mode to be validated at all.
        let facade_dir = tempfile::tempdir().unwrap();
        let facade_path = facade_dir.path().to_str().unwrap();

        let parse = |extra: &[&str]| {
            let mut cmd: Vec<&str> = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };

        // Photos unless asked otherwise, and the names the mode went by before
        // it was called that still select it, so an old script keeps working.
        assert_eq!(parse(&[]).mapillary_facade_mode, FacadeMode::Photos);
        for value in ["photos", "paintings-v2", "paintings2", "paintings"] {
            let args = parse(&["--mapillary-facade-mode", value]);
            assert_eq!(args.mapillary_facade_mode, FacadeMode::Photos, "{value}");
            assert!(args.mapillary_facade_mode.places_displays());
        }
        let args = parse(&["--mapillary-facade-mode", "blocks"]);
        assert_eq!(args.mapillary_facade_mode, FacadeMode::Blocks);
        assert!(!args.mapillary_facade_mode.places_displays());
        // The GUI hands the mode over as the same strings.
        assert_eq!(FacadeMode::from_str_lossy("blocks"), FacadeMode::Blocks);
        for value in [
            "photos",
            "Photos",
            "paintings",
            "paintings-v2",
            "paintings_v2",
            "paintings2",
            "PaintingsV2",
        ] {
            assert_eq!(
                FacadeMode::from_str_lossy(value),
                FacadeMode::Photos,
                "{value}"
            );
        }
        // Anything an older build left behind takes the default.
        assert_eq!(FacadeMode::from_str_lossy("v2"), FacadeMode::Photos);
        assert_eq!(FacadeMode::from_str_lossy(""), FacadeMode::Photos);

        // The photo panels are Java entities.
        let mut args = parse(&[
            "--mapillary-facades-dir",
            facade_path,
            "--mapillary-facade-mode",
            "photos",
        ]);
        assert!(validate_args(&args).is_ok());
        args.bedrock = true;
        let err = validate_args(&args).unwrap_err();
        assert!(err.contains("photos") && err.contains("Java"), "{err}");
        args.bedrock = false;
        args.luanti = true;
        assert!(validate_args(&args).is_err());
        // Blocks mode is fine on every format.
        args.mapillary_facade_mode = FacadeMode::Blocks;
        assert!(validate_args(&args).is_ok());

        // The same check has to reach the fetch path, which has no folder, and
        // it applies to the default too: a Bedrock run with a token has to say
        // `blocks` rather than get a world with a pack it cannot show.
        let mut args = parse(&["--mapillary-token", "MLY|test"]);
        assert!(validate_args(&args).is_ok());
        args.bedrock = true;
        let err = validate_args(&args).unwrap_err();
        assert!(err.contains("photos") && err.contains("Java"), "{err}");
        args.mapillary_facade_mode = FacadeMode::Blocks;
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn facade_px_takes_the_resolutions_the_atlas_budget_halves() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd: Vec<&str> = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::try_parse_from(cmd.iter())
        };

        assert_eq!(parse(&[]).unwrap().facade_px, 16);
        for value in ["4", "8", "16", "32"] {
            let px = parse(&["--facade-px", value]).unwrap().facade_px;
            assert_eq!(px.to_string(), value);
        }
        // Anything the ladder cannot step down from cleanly is refused with
        // the flag named, not rounded to something else in silence.
        for value in ["12", "0", "64", "sixteen"] {
            let err = parse(&["--facade-px", value]).unwrap_err().to_string();
            assert!(err.contains("--facade-px"), "{value}: {err}");
        }
    }

    #[test]
    fn preset_facades_are_off_by_default_and_need_a_java_world() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd: Vec<&str> = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };

        // Off unless asked for, and asking for it needs no token and no other
        // setting: that is the whole point of the second source.
        let args = parse(&[]);
        assert!(!args.building_facades);
        assert!(args.building_facades_dir.is_none());
        assert!(validate_args(&args).is_ok());

        let mut args = parse(&["--building-facades"]);
        assert!(args.building_facades);
        assert!(validate_args(&args).is_ok());

        // Panels are Java entities, on this source as on the other one.
        args.bedrock = true;
        let err = validate_args(&args).unwrap_err();
        assert!(
            err.contains("--building-facades") && err.contains("Java"),
            "{err}"
        );
        args.bedrock = false;
        args.luanti = true;
        assert!(validate_args(&args).is_err());
        // And turning it off leaves those formats generating as they always did.
        args.building_facades = false;
        assert!(validate_args(&args).is_ok());

        // The two facade sources are alternatives: asking for the presets turns
        // the Mapillary facades off, so no mode of theirs can clash with them
        // and none of these is refused.
        let mut args = parse(&["--building-facades", "--mapillary-token", "MLY|test"]);
        for mode in [FacadeMode::Photos, FacadeMode::Blocks] {
            args.mapillary_facade_mode = mode;
            assert!(validate_args(&args).is_ok(), "{mode:?} was refused");
            assert!(
                !args.mapillary_facades_on(),
                "{mode:?} left the Mapillary facades on beside the presets"
            );
        }
        // And with the presets off, a token alone still turns Mapillary on.
        args.building_facades = false;
        assert!(args.mapillary_facades_on());

        // A replacement set is pointed at by directory, and a directory that is
        // not there is a mistake worth saying out loud.
        let set = tempfile::tempdir().unwrap();
        let args = parse(&[
            "--building-facades",
            "--building-facades-dir",
            set.path().to_str().unwrap(),
        ]);
        assert!(validate_args(&args).is_ok());
        let args = parse(&["--building-facades-dir", "no/such/place"]);
        let err = validate_args(&args).unwrap_err();
        assert!(err.contains("--building-facades-dir"), "{err}");
    }

    #[test]
    fn a_token_alone_turns_facades_on() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd: Vec<&str> = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };

        // No token, no facades, and no complaint: the feature is simply absent.
        let mut args = parse(&[]);
        args.mapillary_token = None;
        assert!(!args.mapillary_facades_on());
        assert!(!args.mapillary_pipeline_on());
        assert!(validate_args(&args).is_ok());

        // A token is the whole opt-in.
        let mut args = parse(&["--mapillary-token", "MLY|test"]);
        assert!(args.mapillary_facades_on());
        assert!(args.mapillary_pipeline_on());

        // A blank token reads as no token, which is how an unset environment
        // variable arrives.
        args.mapillary_token = Some("   ".to_string());
        assert!(!args.mapillary_facades_on());

        // The flag can still refuse the work without losing the token.
        let args = parse(&[
            "--mapillary-token",
            "MLY|test",
            "--mapillary-facades",
            "false",
        ]);
        assert!(!args.mapillary_facades_on());
        assert_eq!(args.mapillary_api_token(), Some("MLY|test"));

        // Asking for it without a credential is the one case worth an error.
        let mut args = parse(&["--mapillary-facades"]);
        args.mapillary_token = None;
        assert!(args.mapillary_facades == Some(true));
        assert!(validate_args(&args).unwrap_err().contains("token"));

        // A folder overrides the fetch, so the pipeline stays off.
        let facade_dir = tempfile::tempdir().unwrap();
        let args = parse(&[
            "--mapillary-token",
            "MLY|test",
            "--mapillary-facades-dir",
            facade_dir.path().to_str().unwrap(),
        ]);
        assert!(args.mapillary_facades_on());
        assert!(!args.mapillary_pipeline_on());
        assert!(args.mapillary_facades_wanted());
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
