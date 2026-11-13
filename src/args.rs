use crate::coordinate_system::geographic::LLBBox;
use crate::element_processing::road_detail::RoadDetail;
use clap::builder::FalseyValueParser;
use clap::{ArgAction, Parser};
use std::path::PathBuf;
use std::time::Duration;

/// Command-line arguments parser
#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Args {
    /// Bounding box of the area (min_lat,min_lng,max_lat,max_lng).
    /// Required unless --file supplies a local OSM file to derive it from. Below scale 0.3
    /// and on the Moon or Mars no OSM data is loaded and --file is ignored, so there it is
    /// always required.
    #[arg(long, allow_hyphen_values = true, value_parser = LLBBox::from_str)]
    pub bbox: Option<LLBBox>,

    /// Generate only inside this outline instead of a rectangle: "lat,lng lat,lng lat,lng ...".
    /// Repeat it to merge several shapes into one world, where they keep their real
    /// positions. Data is still downloaded for the rectangle around all of them.
    #[arg(
        long = "polygon",
        value_name = "POINTS",
        allow_hyphen_values = true,
        conflicts_with = "bbox"
    )]
    pub polygon: Vec<String>,

    /// Like --polygon, from the Polygon and MultiPolygon features of a GeoJSON file (holes
    /// are kept out). Combines with --polygon.
    #[arg(long = "polygon-file", value_name = "FILE", conflicts_with = "bbox")]
    pub polygon_file: Option<PathBuf>,

    /// The shapes of --polygon and --polygon-file, read by `resolve_shapes`.
    #[arg(skip)]
    pub shapes: Option<crate::coordinate_system::geographic::LLShapes>,

    /// Local OSM data to generate from instead of downloading it: an .osm/.xml file (OSM XML)
    /// or Overpass JSON. Without --bbox, the area is taken from the file.
    #[arg(long, group = "location")]
    pub file: Option<String>,

    /// Save the OSM data this run uses to a file (Overpass JSON), to generate from it again
    /// later with --file
    #[arg(long, group = "location")]
    pub save_json_file: Option<String>,

    /// Folder the new world is created in, e.g. your Minecraft saves folder. Required for
    /// Java (and the saves folder with --one-world). Bedrock defaults to the Desktop and
    /// Luanti to its worlds folder. --path is a deprecated alias.
    #[arg(long = "output-dir", alias = "path")]
    pub path: Option<PathBuf>,

    /// Generate a Bedrock Edition world (.mcworld) instead of Java Edition
    #[arg(long)]
    pub bedrock: bool,

    /// Generate a Luanti/Minetest world (map.sqlite) instead of Java Edition
    #[arg(long)]
    pub luanti: bool,

    /// How Overpass is queried when the data comes from there: requests (built in), curl or
    /// wget
    #[arg(long, default_value = "requests")]
    pub downloader: String,

    /// World scale to use, in blocks per meter (1.0 = real size).
    /// Ignored for --body moon/mars, which use their own fixed scale.
    #[arg(long, default_value_t = 1.0, allow_hyphen_values = true, value_parser = parse_scale)]
    pub scale: f64,

    /// Multiplies terrain height: 2.0 makes hills twice as tall, 0.5 half as tall.
    /// Buildings and trees keep their size. Relief that no longer fits the build
    /// height is compressed to fit, as it is at 1.0.
    #[arg(long, default_value_t = 1.0, value_parser = parse_height_multiplier)]
    pub height_multiplier: f64,

    /// Celestial body to generate. moon and mars use NASA PDS elevation at a fixed
    /// low scale and have no OSM data, so every object option is ignored.
    #[arg(long, value_enum, default_value_t = crate::celestial::CelestialBody::Earth)]
    pub body: crate::celestial::CelestialBody,

    /// Projection mode for coordinate mapping.
    /// local: each generation starts at Minecraft (0,0) (default).
    /// web_mercator: Web Mercator centred on the bbox.
    #[arg(long, default_value = "local")]
    pub projection: crate::projection::ProjectionKind,

    /// Generate into one persistent Java world that every later run extends.
    /// --output-dir is then the saves folder. The first run fixes scale, ground
    /// level, terrain mode and build height; rotation must be 0.
    #[arg(long, default_value_t = false)]
    pub one_world: bool,

    /// Name of the world folder for --one-world (default "Arnis One World").
    #[arg(long)]
    pub world_name: Option<String>,

    /// Set by `one_world::prepare`.
    #[arg(skip)]
    pub one_world_run: Option<crate::one_world::RunContext>,

    /// The range an existing world's tall datapack declares, which a run into it keeps.
    #[arg(skip)]
    pub tall_range: Option<(i32, i32)>,

    /// Ground level to use in the Minecraft world
    #[arg(long, default_value_t = -62, allow_hyphen_values = true)]
    pub ground_level: i32,

    /// What to generate, mirroring the GUI's generation mode dropdown:
    /// geo-terrain: OSM objects on real elevation terrain (default)
    /// geo-only: OSM objects on flat ground
    /// terrain-only: real elevation terrain with OSM water and nature, but no buildings, roads
    /// or Overture objects (--overture has no effect); towns and fields turn back into nature
    /// blueprint: real elevation terrain with roads, rails, water and nature, and buildings
    /// drawn only as outlines on the ground
    #[arg(long, value_enum, default_value_t = GenerationMode::GeoTerrain)]
    pub mode: GenerationMode,

    /// Turn the world into a ruin: buildings partly collapsed and broken open, roads cracked
    /// and overgrown, lights out
    #[arg(long, default_value_t = false, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub post_apocalyptic: bool,

    /// Deep winter: snow on the ground, roofs and trees, frozen rivers and lakes, snowy biomes
    #[arg(long, default_value_t = false, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub winter: bool,

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

    /// Carve Minecraft-style caves into the filled ground: vanilla 1.21 noise caves and
    /// tunnel carvers, flooded pools and underground rivers, a lava sea at the bottom of the
    /// world, vanilla ores and eight themed cave biomes. Implies --fillground. Caves need rock
    /// to carve into: at the default ground level they only appear under terrain that rises
    /// above the lowest point of the area.
    #[arg(long, default_value_t = false)]
    pub caves: bool,

    /// Directory of a cave asset pack (cave_pack.json plus Sponge .schem formations such as
    /// ice spikes, dripstone columns and amethyst clusters) stamped into --caves. Defaults to
    /// a `cave-pack` directory next to the executable when one exists; without either, caves
    /// keep their procedural decoration only.
    #[arg(long = "cave-asset-pack", value_name = "DIR")]
    pub cave_asset_pack: Option<PathBuf>,

    /// Per-biome cave theme amounts as comma-separated `name=percent` pairs. Names: lush,
    /// dripstone, deepdark, mushroom, ice, amethyst, volcanic, coral. 100 is the default
    /// share, 0 turns the biome off, 200 roughly doubles its area. Depth and terrain gates
    /// (volcanic at the bottom, ice under mountains, coral in flooded pools) always apply.
    /// Example: --cave-biomes lush=150,deepdark=0,amethyst=60
    #[arg(long = "cave-biomes", value_name = "LIST")]
    pub cave_biomes: Option<String>,

    /// Render the cave biome layout for --bbox and exit without generating a world: writes
    /// `<PREFIX>-upper.png` (upper caves) and `<PREFIX>-deep.png` (deep caves), transparent
    /// where the cave is plain rock, and prints a `ZONEMAP {json}` line with each theme's
    /// share. Honours --scale and --cave-biomes, so it matches what --caves will carve.
    #[arg(long = "cave-zone-map", value_name = "PREFIX")]
    pub cave_zone_map: Option<PathBuf>,

    /// Blocks per output pixel for --cave-zone-map (1-512). Omitted picks a fine step that
    /// keeps the image within 1536 pixels.
    #[arg(long = "cave-zone-map-step", value_name = "BLOCKS")]
    pub cave_zone_map_step: Option<u32>,

    /// Use the legacy procedural trees instead of the bundled schematic tree pack.
    /// Schematic trees are on by default; this flag opts out.
    #[arg(long, default_value_t = false)]
    pub legacy_trees: bool,

    /// Farmland style: classic (one sheet of mixed crops), smallholding (narrow strips),
    /// patchwork (mixed fields of medium width) or prairie (wide, wheat-led fields).
    /// Anything but classic gives each field one crop, split inside its OSM outline.
    #[arg(long, value_enum, default_value_t)]
    pub fields: crate::element_processing::field_texture::FieldPreset,

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

    /// Base URL of the Arnis OSM tile archive, read instead of Overpass.
    #[arg(long = "osm-tiles-url", default_value = crate::osm_tiles::DEFAULT_OSM_TILES_URL)]
    pub osm_tiles_url: String,

    /// Query Overpass directly and skip the tile archive.
    #[arg(long, default_value_t = false)]
    pub no_tile_archive: bool,

    /// Spawn point latitude (optional, must be within bbox)
    #[arg(long, allow_hyphen_values = true)]
    pub spawn_lat: Option<f64>,

    /// Spawn point longitude (optional, must be within bbox)
    #[arg(long, allow_hyphen_values = true)]
    pub spawn_lng: Option<f64>,

    /// Clockwise rotation angle in degrees (optional, range: -90 to 90)
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub rotation: f64,

    /// Keep the vanilla build height (Y -64 to 319) and compress taller terrain to fit. By
    /// default an area that needs more room gets a bundled pack that extends the build
    /// height (Java 1.21.4+: Y -2032 to 2031, experimental; Bedrock 1.21.120+: up to Y 511).
    #[arg(long, default_value_t = false, conflicts_with = "disable_height_limit")]
    pub vanilla_height: bool,

    /// Add the extended build height pack even where the area fits the vanilla height.
    /// Before the terrain fit this says the height is fixed as extended (this flag or a
    /// One World); `ground::settle_build_height` then sets it for an area that needs it,
    /// so from there on it is the world's height.
    #[arg(long, default_value_t = false, hide = true)]
    pub disable_height_limit: bool,

    /// Use only the legacy AWS Terrain Tiles source (~30m) instead of
    /// Mapterhorn.
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

    /// Pre-generate the Distant Horizons mod's LOD database so the world renders
    /// to the horizon on first join, instead of DH building it chunk by chunk.
    /// Java only; implies --bake-lighting.
    #[arg(long, default_value_t = false)]
    pub dh_lod: bool,

    /// Render a top-down PNG map preview of the generated world (Java and Bedrock)
    #[arg(long, default_value_t = false)]
    pub map_preview: bool,

    /// Give the player a map: on Java a locked map item showing the whole world, on Bedrock
    /// the game's starting map
    #[arg(long = "map-item", default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub map_item: bool,

    /// Player game mode for the generated world
    #[arg(long, value_enum, default_value_t = GameMode::Creative)]
    pub gamemode: GameMode,

    /// Initial time of day in ticks (0 = dawn, 6000 = noon, 18000 = midnight)
    #[arg(long, default_value_t = DEFAULT_WORLD_TIME, value_parser = clap::value_parser!(i64).range(0..24000))]
    pub world_time: i64,

    /// Java only: what the game generates around the area, empty void or a flat grass plain
    #[arg(long = "world-type", value_enum, default_value_t = WorldType::Void)]
    pub world_type: WorldType,

    /// Natural mob spawning, Java and Bedrock. `animals` puts the world on Peaceful, so only
    /// animals, fish and bats turn up; `all` puts it on Normal for monsters too, with mob
    /// griefing off so creepers and endermen leave the city standing. Earth only; Zombie
    /// Outbreak and Disasters keep it off, as they bring their own mobs.
    #[arg(long = "mob-spawning", value_enum, default_value_t = MobSpawning::Off)]
    pub mob_spawning: MobSpawning,

    /// Readable image signs, Java only. `basic` covers public signage: street names,
    /// traffic signs, transit stops, information boards and billboards. `full` adds
    /// building signage: shop name plates, house numbers and crossing signs.
    #[arg(long, value_enum, default_value_t = SignageLevel::Basic)]
    pub signage: SignageLevel,

    /// Mapillary API token, from https://www.mapillary.com/developer. Required by
    /// --mapillary-facades and --mapillary-probe.
    // A credential: --help shows the variable's name but never its value.
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
    /// allows; walls that still do not fit at 4 are left without a panel.
    #[arg(long, default_value_t = 16, value_parser = parse_facade_px)]
    pub facade_px: u32,

    /// Directory holding the preset facade set: a manifest.json and the images
    /// it names. The set is compiled in, so this only has to be given to run a
    /// replacement one.
    #[arg(long)]
    pub building_facades_dir: Option<PathBuf>,

    /// Moon and Mars: lower gravity for players and mobs, with fall damage scaled to
    /// match. Runs from a data pack in the world. Java 1.21.4+.
    #[arg(long, default_value_t = false)]
    pub body_gravity: bool,

    /// Signal heads whose lamps cycle red, amber and green, with pedestrian heads at
    /// signalised crossings. Runs from a data pack in the world. Java 1.21.4+.
    #[arg(long, default_value_t = false)]
    pub traffic_lights: bool,

    /// An advancement tab for the area's landmarks, museums, parks, stations and more,
    /// each unlocked by walking there. Java 1.21.4+.
    #[arg(long, default_value_t = false)]
    pub discoveries: bool,

    /// An "Arnis" button in the pause menu with travel, discoveries and the data
    /// credits. Java 1.21.6+; older versions get the credits in chat instead.
    #[arg(long, default_value_t = false)]
    pub pause_menu: bool,

    /// Zombie Outbreak: five nights of growing waves through the streets, a boss on the
    /// last, and a supply drop every morning. Starts the world in survival. Java 1.21.6+.
    #[arg(long, default_value_t = false)]
    pub zombie_outbreak: bool,

    /// Disasters: a meteor shower, a tornado, a flood, an earthquake and more, one every few
    /// minutes in random order, picked from those the area can have. Floods rise from the
    /// mapped rivers and the sea. Starts the world in survival. Implies --fillground and a
    /// ground level of at least -40, as sinkholes and cracks open up the ground. Java 1.21.6+.
    #[arg(long, default_value_t = false)]
    pub disasters: bool,

    /// Cars that drive the streets at the speed limit, queue behind each other, turn at
    /// junctions and, with --traffic-lights, stop at red; and trains, S-Bahns, metros and
    /// trams that run the mapped tracks and stop at their stations. On Java, right-click a
    /// car or train to get in, or click one as a spectator to follow it. Java 1.21.4+ or Bedrock
    /// 1.21.120+; Bedrock has no traffic lights to stop at.
    #[arg(long, alias = "moving-cars", default_value_t = false)]
    pub moving_vehicles: bool,

    /// People who walk the mapped footways, sidewalks, squares, pedestrian streets and
    /// paths, busier by day and near shops and sights, waiting at kerbs and, with
    /// --traffic-lights, for the green man. Cars stop for them. Java 1.21.9+ shows them as mannequins in the
    /// game's own skins, older versions as villagers.
    #[arg(long, default_value_t = false)]
    pub pedestrians: bool,

    #[command(flatten)]
    pub live: LiveArgs,

    /// Also report progress as JSON lines on stdout for programs driving the CLI.
    #[arg(long, value_enum)]
    pub progress: Option<ProgressFormat>,

    /// Worker threads for generation. Defaults to 90% of the cores, or to RAYON_NUM_THREADS
    /// when that is set; this flag wins over both. In a --one-world job built in pieces it is
    /// the count for each piece, several of which run at once, and RAYON_NUM_THREADS is not
    /// read.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=4096))]
    pub threads: Option<u32>,

    /// Memory this run may assume it has, in MB, in place of the free-RAM reading when sizing
    /// its parallel work, so several processes can split one machine.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..=100_000_000))]
    pub ram_budget_mb: Option<u64>,

    /// Downloads this process keeps in flight at once (default 16). The pieces of a --one-world
    /// job share twice this, at least 2 each.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub max_downloads: Option<u32>,

    /// Skip the check for a newer release on GitHub.
    #[arg(long, env = "ARNIS_NO_UPDATE_CHECK", value_parser = FalseyValueParser::new())]
    pub no_update_check: bool,

    /// Skip the startup sweep that deletes cached elevation tiles older than 30 days.
    #[arg(long, env = "ARNIS_NO_CACHE_SWEEP", value_parser = FalseyValueParser::new())]
    pub no_cache_sweep: bool,

    /// Print the flags this build supports as one JSON line and exit, so a program driving the
    /// CLI can probe before using them.
    #[arg(long)]
    pub capabilities: bool,

    #[command(flatten)]
    pub units: UnitArgs,

    /// EXPERIMENTAL. Java region container; blinear writes Leaf 1.21.11+ and 26.x B_Linear regions (not with --one-world, --voxy-lod or --dh-lod).
    #[arg(long = "region-format", value_enum, default_value_t = crate::output_args::RegionFormat::Mca)]
    pub region_format: crate::output_args::RegionFormat,

    /// zstd level for --region-format blinear (Leaf's own default is 6).
    #[arg(long = "blinear-level", default_value_t = 6, value_parser = clap::value_parser!(i32).range(1..=22))]
    pub blinear_level: i32,

    /// LAT,LON for block (0, 0) of a --one-world world this run creates, instead of the centre of the first --bbox.
    #[arg(long, value_name = "LAT,LON", value_parser = crate::output_args::parse_origin, allow_hyphen_values = true)]
    pub origin: Option<(f64, f64)>,

    /// Java only: put the world border around the generated area (a One World: around all its areas) after the run.
    #[arg(long, default_value_t = false)]
    pub world_border: bool,

    /// Redraw the map item of the One World at --output-dir and --world-name from its area previews and exit.
    #[arg(long, default_value_t = false)]
    pub map_item_only: bool,

    /// Write the Distant Horizons LODs of the One World at --output-dir and --world-name from its region files and exit.
    #[arg(long, default_value_t = false)]
    pub dh_lod_only: bool,
    #[command(flatten)]
    pub water: WaterArgs,

    /// Experimental. Where the Köppen climate behind surfaces and biomes is read:
    /// origin (once, at the centre of the area, default) or per-position (across
    /// the area, with organic borders, so a large world crosses climate zones;
    /// biome latitude follows each block too).
    #[arg(long, value_enum, default_value_t = crate::climate_field::ClimateMode::Origin)]
    pub climate_mode: crate::climate_field::ClimateMode,

    /// Experimental. Render the per-position Köppen climate layout for --bbox to
    /// `<PREFIX>.png` and exit without generating a world; prints a
    /// `CLIMATEMAP {json}` line with each climate's share.
    #[arg(long = "climate-map", value_name = "PREFIX")]
    pub climate_map: Option<PathBuf>,

    /// Lowest Y of the extended-height world, a multiple of 16 from -2032 to -64. Turns on the
    /// extended build height; the default is the pack's -2032.
    #[arg(
        long,
        value_name = "Y",
        allow_hyphen_values = true,
        conflicts_with = "vanilla_height"
    )]
    pub min_y: Option<i32>,

    /// Highest Y of the extended-height world, one less than a multiple of 16 from 319 to
    /// 2031. Turns on the extended build height; the default is the pack's 2031.
    #[arg(
        long,
        value_name = "Y",
        allow_hyphen_values = true,
        conflicts_with = "vanilla_height"
    )]
    pub max_y: Option<i32>,

    /// EXPERIMENTAL. World seed mixed into most random choices (trees, rocks, flowers,
    /// building variety), so the same area can be rebuilt with a different but repeatable
    /// look; trains, facades and rooftop details keep theirs, and climate borders stay put.
    /// Omitted or 0 keeps the default look. A One World keeps the seed its first area was
    /// built with.
    #[arg(long, value_name = "SEED")]
    pub seed: Option<u64>,
    #[command(flatten)]
    pub scatter: ScatterArgs,

    /// Force a tree realm instead of picking one from the area's location: auto, afr, asn,
    /// aus, ena, eur, fl, ind, sam, wna or vanilla-plus. A forced realm skips the ecoregion
    /// mixes and keeps its palms.
    #[arg(long, value_name = "REALM", value_parser = clap::builder::PossibleValuesParser::new(crate::trees::tree_pack::REALMS))]
    pub tree_realm: Option<String>,

    /// Relative popularity of the schematic tree sizes, as name=percent pairs
    /// (small, medium, big, tall, giant; 0-200, omitted ones stay 100). 0 turns a
    /// size off. Measured canopy heights still pick their own size.
    /// Example: --tree-size-weights big=150,tall=50,giant=0
    #[arg(long, value_name = "LIST", value_parser = crate::trees::size_weights::SizeWeights::parse)]
    pub tree_size_weights: Option<crate::trees::size_weights::SizeWeights>,

    /// Experimental. A folder of your own tree schematics, laid out like the
    /// bundled packs: <realm>/<community>/<tree type>/[<size>/]<any name>.schem.
    /// The folders say what a file is; the size folder (small, medium, big,
    /// tall, giant) is optional and otherwise measured from the height. A file
    /// that does not load, is over 8 MB, wider than 64 blocks or taller than 256
    /// is skipped with a warning.
    #[arg(long, value_name = "FOLDER")]
    pub tree_pack_dir: Option<PathBuf>,

    /// With --tree-pack-dir: add the folder's trees to the built-in ones, or
    /// replace a realm's built-in trees wherever the folder has any for it.
    #[arg(long, value_enum, default_value_t = crate::trees::pack_dir::TreePackMode::Add)]
    pub tree_pack_mode: crate::trees::pack_dir::TreePackMode,

    /// Write every bundled tree schematic into the --tree-pack-dir layout in
    /// FOLDER, to edit or adapt, and exit. Files already there are kept.
    #[arg(long, value_name = "FOLDER")]
    pub export_tree_packs: Option<PathBuf>,

    /// How much of the underground the themed cave biomes take: vanilla (plain caves only),
    /// more-vanilla (about 30% themed), all-mix (every theme at its default amount, about 77%
    /// themed, the default) or more-mix (about 89% themed). --cave-biomes amounts override the
    /// style's per theme.
    #[arg(long = "cave-style", value_enum)]
    pub cave_style: Option<crate::caves::CaveStyle>,

    /// Ore veins in --caves: normal (vanilla's ore table, the default) or more (twice the
    /// veins of every metal and gem ore, at vanilla's heights).
    #[arg(long = "cave-ores", value_enum)]
    pub cave_ores: Option<crate::caves::CaveOres>,

    /// Seed for --caves and --cave-zone-map: each seed gives the same area a different cave
    /// layout. Omitted keeps the built-in seed, so an area always gets the same caves.
    #[arg(long = "cave-seed", value_name = "SEED")]
    pub cave_seed: Option<u64>,

    /// Y that stands for vanilla's world floor in the cave passes, a multiple of 16 from
    /// -2032 to 2016. One value for every run makes caves, the lava sea and the deepslate
    /// line continuous across them, and plans pools, rivers and geodes past the run's edge.
    #[arg(long = "cave-datum-y", value_name = "Y", allow_hyphen_values = true)]
    pub cave_datum_y: Option<i32>,
    /// How much road marking and how many minor ways to build: max, clean or compact.
    #[arg(long, value_enum, default_value_t = RoadDetail::Max)]
    pub road_detail: RoadDetail,

    /// Lay asphalt, concrete and paving mixes and built-up stone ground in patches a few
    /// blocks across instead of a per-block speckle, and give pitched clay roofs a full
    /// block in their stairs' colour, so no single block stands out as a dot.
    #[arg(long)]
    pub surface_patches: bool,

    /// Leave out buildings: OSM buildings, building parts, doors and entrances, the
    /// Overture footprints, landmarks and Mapillary facades. Roads, rail, water, land cover
    /// and terrain are built as usual.
    #[arg(long = "no-buildings", default_value_t = true, action = ArgAction::SetFalse)]
    pub buildings: bool,

    /// Which 3D props to place: all, none, or a list of boat, car, crane, excavator,
    /// fountain, helicopter, jetbridge, landmark, lighthouse, plane, playground, starship,
    /// tombstone, tractor and windturbine. Given, it decides the props whatever --no-3d
    /// says; left out, they follow --no-3d.
    #[arg(long, value_parser = crate::structures::PropSet::parse)]
    pub props: Option<crate::structures::PropSet>,

    /// Leave out every schematic prop below this world scale. Props keep their block size
    /// at any scale, so on a small-scale world they tower over the buildings around them.
    #[arg(long, value_parser = parse_props_min_scale)]
    pub props_min_scale: Option<f64>,

    /// Chest loot table (JSON) for --interior, in place of the built-in one. A file that
    /// cannot be read or fails validation stops the run with an error.
    #[arg(long, value_name = "FILE")]
    pub loot_table: Option<PathBuf>,

    /// Write the built-in chest loot table to FILE as JSON to start from, and exit.
    #[arg(long, value_name = "FILE")]
    pub dump_loot_table: Option<PathBuf>,
    /// Read OpenStreetMap from an .osm.pbf file, or from `geofabrik` for the smallest Geofabrik region holding the area, instead of the tile archive.
    #[arg(long, value_name = "PATH|geofabrik", conflicts_with = "file")]
    pub osm_pbf: Option<String>,

    /// The extract to download for `--osm-pbf geofabrik`, instead of picking one.
    #[arg(long, value_name = "URL", requires = "osm_pbf")]
    pub osm_pbf_url: Option<String>,

    /// Overpass server(s) to use in place of Arnis's own, tried in order; repeat the flag or separate with commas. The tile archive is still read first and these are its fallback; with --no-tile-archive they are the primary source.
    #[arg(
        long,
        env = "ARNIS_OVERPASS_URL",
        value_name = "URL",
        value_delimiter = ','
    )]
    pub overpass_url: Vec<String>,

    /// Never use the network: read everything from the caches and stop with a list of what they lack.
    #[arg(long, env = "ARNIS_OFFLINE", value_parser = FalseyValueParser::new())]
    pub offline: bool,

    /// Download and cache every input this command would read for --bbox, then exit without writing a world.
    #[arg(long, conflicts_with = "offline")]
    pub prewarm: bool,

    /// With --unit-regions, warm the caches for every piece, one at a time, before the pieces start building.
    #[arg(long, conflicts_with = "offline")]
    pub prewarm_first: bool,
}

/// Water options. Off unless asked for, so the GUI runs on `WaterArgs::default()`.
#[derive(clap::Args, Debug, Default)]
pub struct WaterArgs {
    /// River bed shape. `v1` digs mapped rivers, canals and streams a U-shaped bed that
    /// deepens with their width, in patches of earth, sand and gravel; lakes and the sea
    /// keep their bed.
    #[arg(long, value_enum, default_value_t = crate::river_bed::RiverBed::Off)]
    pub river_bed: crate::river_bed::RiverBed,

    /// Water detail. `scaled` rounds bed depths instead of flooring them and, on small
    /// maps, gives narrow water a bowl-shaped bed, draws streams narrower and lets water
    /// run over a road where it crosses a short stretch of water without a bridge.
    #[arg(long, value_enum, default_value_t = crate::water_detail::WaterDetail::Default)]
    pub water_detail: crate::water_detail::WaterDetail,
}

impl WaterArgs {
    /// How far these options can carve beyond the land-cover estimate.
    pub fn carve_depth(&self, scale: f64) -> crate::water_detail::CarveDepth {
        crate::water_detail::CarveDepth {
            floor: if self.river_bed == crate::river_bed::RiverBed::Off {
                0
            } else {
                crate::water_depth::MAX_WATER_DEPTH
            },
            detail: self.water_detail,
            scale,
        }
    }
}

/// Rocks and bushes on open land. On unless turned off.
#[derive(clap::Args, Debug)]
pub struct ScatterArgs {
    /// Scatter small andesite and tuff rocks over grassland and cropland outside towns
    /// (--rocks false turns it off). Needs land cover.
    #[arg(long, default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub rocks: bool,

    /// Share of 16x16 chunks that get a rock, from 0.0 to 1.0.
    #[arg(long, default_value_t = 0.02, value_parser = parse_density)]
    pub rock_density: f64,

    /// Scatter small bushes of ten species over grassland and cropland outside towns
    /// (--bushes false turns it off). Needs land cover.
    #[arg(long, default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub bushes: bool,

    /// Share of 16x16 chunks that get a bush, from 0.0 to 1.0.
    /// Where a chunk rolls both, the rock wins.
    #[arg(long, default_value_t = 0.05, value_parser = parse_density)]
    pub bush_density: f64,
}

impl Default for ScatterArgs {
    fn default() -> Self {
        Self {
            rocks: true,
            rock_density: 0.02,
            bushes: true,
            bush_density: 0.05,
        }
    }
}

fn parse_density(s: &str) -> Result<f64, String> {
    match s.trim().parse::<f64>() {
        Ok(v) if (0.0..=1.0).contains(&v) => Ok(v),
        _ => Err(format!("{s}: expected a share from 0.0 to 1.0")),
    }
}

/// Large One World jobs cut into region-group pieces.
#[derive(clap::Args, Debug, Default)]
pub struct UnitArgs {
    /// Print how --one-world would cut --bbox into pieces of at most N x N regions (one JSON
    /// line on stdout) and exit without generating.
    #[arg(long, value_name = "auto|N", value_parser = parse_unit_size)]
    pub plan_units: Option<UnitSize>,

    /// Piece size of a --one-world job, at most N x N regions. auto takes 8 x 8 pieces, 12 x 12
    /// or 16 x 16 with memory to spare, 4 x 4 or 2 x 2 short of it, split smaller along a
    /// --plan-mask; with --one-world-workers auto a small selection is built in one run.
    #[arg(long, value_name = "auto|N", value_parser = parse_unit_size)]
    pub unit_regions: Option<UnitSize>,

    /// One piece of a job, run by the job's coordinator.
    #[arg(long, hide = true)]
    pub one_world_unit: Option<PathBuf>,

    /// Build a --one-world selection in pieces, this many at once, each in its own process
    /// with all of --threads and a share of the job's memory and downloads. auto picks 1 to 6
    /// from the free RAM and cores: 1 with six cores or fewer, or too little memory for a piece.
    /// Pieces are 8 x 8 regions unless --unit-regions says otherwise; run the same command
    /// again to resume an interrupted job.
    #[arg(long, value_name = "auto|N", value_parser = parse_workers)]
    pub one_world_workers: Option<Workers>,

    /// Build only these pieces of the job (numbered from 1, as the job prints them), say one
    /// that failed; the others wait for the next run of the same job.
    #[arg(long, value_name = "N,N...", value_delimiter = ',',
          value_parser = clap::value_parser!(u32).range(1..))]
    pub only_pieces: Option<Vec<u32>>,

    /// Snap a --one-world job to whole pieces on the world's region lattice: cover grows it,
    /// fit shrinks it; merge keeps its edges but folds an edge strip thinner than a quarter
    /// region into its neighbour.
    #[arg(long, value_enum, value_name = "cover|fit|merge")]
    pub snap_regions: Option<SnapRegions>,

    /// Regions added on every side of a --one-world job, after any --snap-regions.
    #[arg(long, value_name = "K", default_value_t = 0,
          value_parser = clap::value_parser!(i32).range(0..=64))]
    pub margin_regions: i32,

    /// Skip the pieces of a --one-world job whose every chunk is already in the world, say
    /// from an earlier job over part of this one.
    #[arg(long)]
    pub skip_built: bool,

    /// Order a --one-world job builds its pieces in: from the centre out (the default), from
    /// one side, or the listed piece numbers first and the rest from the centre out.
    #[arg(long, value_name = "centre|west|east|north|south|N,N...", value_parser = parse_piece_order)]
    pub piece_order: Option<PieceOrder>,

    /// Keep only the pieces of a --one-world job that touch this polygon, a GeoJSON
    /// (Multi)Polygon, Feature or FeatureCollection, or an Osmosis .poly file.
    #[arg(long, value_name = "FILE")]
    pub plan_mask: Option<PathBuf>,

    /// Once every piece of a --one-world job is built, redraw the world's map item over all
    /// of them; without it the map shows the first piece only.
    #[arg(long)]
    pub stitch_map: bool,

    /// Leave these cells of a --one-world job out (plan indexes from 0, as --plan-units lists
    /// them); kept with the job, and an empty list builds every cell again.
    #[arg(long, hide = true, value_name = "N,N...", value_parser = parse_cells)]
    pub skip_cells: Option<std::collections::BTreeSet<u32>>,
}

/// `--skip-cells`: plan indexes, comma separated; empty for none.
fn parse_cells(s: &str) -> Result<std::collections::BTreeSet<u32>, String> {
    s.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.parse().map_err(|_| format!("{t}: not a cell number")))
        .collect()
}

/// `--piece-order`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PieceOrder {
    Centre,
    West,
    East,
    North,
    South,
    /// Piece numbers, from 1.
    List(Vec<u32>),
}

fn parse_piece_order(s: &str) -> Result<PieceOrder, String> {
    Ok(match s {
        "centre" | "center" => PieceOrder::Centre,
        "west" => PieceOrder::West,
        "east" => PieceOrder::East,
        "north" => PieceOrder::North,
        "south" => PieceOrder::South,
        _ => PieceOrder::List(
            s.split(',')
                .map(|n| match n.trim().parse::<u32>() {
                    Ok(n @ 1..) => Ok(n),
                    _ => Err(format!(
                        "{s}: expected centre, west, east, north, south or piece numbers like 3,1,2"
                    )),
                })
                .collect::<Result<_, _>>()?,
        ),
    })
}

/// How `--snap-regions` fits a job to the region lattice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SnapRegions {
    /// Grow to whole pieces.
    Cover,
    /// Shrink to whole pieces; at least the one under the centre.
    Fit,
    /// Keep the edges, merge thin edge strips into their neighbours.
    Merge,
}

impl UnitArgs {
    /// Whether this run coordinates a job of pieces rather than building one area itself.
    pub fn coordinates(&self) -> bool {
        (self.unit_regions.is_some() || self.one_world_workers.is_some())
            && self.one_world_unit.is_none()
    }
}

/// Piece size of a job: N x N regions, or mixed sizes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitSize {
    Auto,
    N(i32),
}

impl std::fmt::Display for UnitSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnitSize::Auto => f.write_str("auto"),
            UnitSize::N(n) => write!(f, "{n}"),
        }
    }
}

fn parse_unit_size(s: &str) -> Result<UnitSize, String> {
    match s {
        "auto" => Ok(UnitSize::Auto),
        _ => match s.parse::<i32>() {
            Ok(n @ 1..=64) => Ok(UnitSize::N(n)),
            _ => Err(format!("{s}: expected auto or a size from 1 to 64")),
        },
    }
}

/// How many pieces of a job run at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workers {
    Auto,
    N(u32),
}

fn parse_workers(s: &str) -> Result<Workers, String> {
    match s {
        "auto" => Ok(Workers::Auto),
        _ => match s.parse::<u32>() {
            Ok(n @ 1..=64) => Ok(Workers::N(n)),
            _ => Err(format!("{s}: expected auto or a count from 1 to 64")),
        },
    }
}

impl Args {
    /// The thread count asked for with --threads, if any.
    pub fn thread_count(&self) -> Option<usize> {
        self.threads.map(|n| n as usize)
    }
}

/// Live tiles build with the stock settings only, so `--live` refuses every flag that would
/// be dropped on its tiles. `--seed`, `--cave-seed` and `--cave-datum-y` travel in the
/// world's manifest; `--no-update-check` and `--no-cache-sweep` act on this process only.
pub fn refuse_live_extras(args: &Args) -> Result<(), String> {
    if !args.live.live {
        return Ok(());
    }
    let (u, w, sc) = (&args.units, &args.water, &args.scatter);
    let used: Vec<&str> = [
        (args.progress.is_some(), "--progress"),
        (args.threads.is_some(), "--threads"),
        (args.ram_budget_mb.is_some(), "--ram-budget-mb"),
        (args.max_downloads.is_some(), "--max-downloads"),
        (u.plan_units.is_some(), "--plan-units"),
        (u.unit_regions.is_some(), "--unit-regions"),
        (u.one_world_unit.is_some(), "--one-world-unit"),
        (
            !matches!(u.one_world_workers, None | Some(Workers::N(1))),
            "--one-world-workers",
        ),
        (u.only_pieces.is_some(), "--only-pieces"),
        (u.snap_regions.is_some(), "--snap-regions"),
        (u.margin_regions != 0, "--margin-regions"),
        (u.skip_built, "--skip-built"),
        (u.piece_order.is_some(), "--piece-order"),
        (u.plan_mask.is_some(), "--plan-mask"),
        (u.stitch_map, "--stitch-map"),
        (u.skip_cells.is_some(), "--skip-cells"),
        (
            args.region_format != crate::output_args::RegionFormat::Mca,
            "--region-format",
        ),
        (args.blinear_level != 6, "--blinear-level"),
        (args.origin.is_some(), "--origin"),
        (args.world_border, "--world-border"),
        (args.map_item_only, "--map-item-only"),
        (args.dh_lod_only, "--dh-lod-only"),
        (w.river_bed != Default::default(), "--river-bed"),
        (w.water_detail != Default::default(), "--water-detail"),
        (args.climate_mode != Default::default(), "--climate-mode"),
        (args.climate_map.is_some(), "--climate-map"),
        (args.min_y.is_some(), "--min-y"),
        (args.max_y.is_some(), "--max-y"),
        (
            sc.rock_density != ScatterArgs::default().rock_density,
            "--rock-density",
        ),
        (
            sc.bush_density != ScatterArgs::default().bush_density,
            "--bush-density",
        ),
        (args.tree_realm.is_some(), "--tree-realm"),
        (args.tree_size_weights.is_some(), "--tree-size-weights"),
        (args.tree_pack_dir.is_some(), "--tree-pack-dir"),
        (
            args.tree_pack_mode != Default::default(),
            "--tree-pack-mode",
        ),
        (args.export_tree_packs.is_some(), "--export-tree-packs"),
        (args.cave_style.is_some(), "--cave-style"),
        (args.cave_ores.is_some(), "--cave-ores"),
        (args.road_detail != RoadDetail::Max, "--road-detail"),
        (args.surface_patches, "--surface-patches"),
        (!args.buildings, "--no-buildings"),
        (args.props.is_some(), "--props"),
        (args.props_min_scale.is_some(), "--props-min-scale"),
        (args.loot_table.is_some(), "--loot-table"),
        (args.dump_loot_table.is_some(), "--dump-loot-table"),
        (args.osm_pbf.is_some(), "--osm-pbf"),
        (args.osm_pbf_url.is_some(), "--osm-pbf-url"),
        (!args.overpass_url.is_empty(), "--overpass-url"),
        (args.offline, "--offline"),
        (args.prewarm, "--prewarm"),
        (args.prewarm_first, "--prewarm-first"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    if used.is_empty() {
        return Ok(());
    }
    let mut msg = format!(
        "--live builds its tiles with the standard settings only, so it cannot take {}.",
        used.join(", ")
    );
    if args.offline {
        msg.push_str(
            " --offline is not supported in Live mode: the tiles and the server download need the network.",
        );
    }
    Err(msg)
}

/// The line `--capabilities` prints: a JSON array of every long flag.
pub fn capabilities_json() -> String {
    use clap::CommandFactory;
    let flags: Vec<String> = Args::command()
        .get_arguments()
        .filter_map(|a| a.get_long().map(str::to_string))
        .collect();
    serde_json::to_string(&flags).unwrap_or_default()
}

/// Machine-readable progress formats for `--progress`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum ProgressFormat {
    /// One JSON object per line.
    Json,
}

/// Live mode: the One World on a local Minecraft server, generated around the players.
#[derive(clap::Args, Debug, Clone)]
pub struct LiveArgs {
    /// Play the One World on a local Minecraft Java server that keeps generating the land
    /// around you while you play. Needs --one-world; --bbox, if given, is where you start.
    /// Runs until Ctrl+C.
    #[arg(long, requires = "one_world")]
    pub live: bool,

    /// Agree to the Minecraft EULA (https://aka.ms/MinecraftEULA), which a server needs.
    #[arg(long)]
    pub accept_eula: bool,

    /// Minecraft version of the live server, which must match the game. Default: the
    /// version the launcher played last.
    #[arg(long = "live-version", id = "live_version", value_name = "VERSION")]
    pub version: Option<String>,

    /// How many chunks around each player the live server loads and sends.
    #[arg(long = "live-view-distance", default_value_t = crate::live::DEFAULT_VIEW_DISTANCE,
        value_parser = clap::value_parser!(u32).range(4..=32))]
    pub view_distance: u32,

    /// Port the live server listens on (default 25565, or a free one).
    #[arg(long = "live-port")]
    pub port: Option<u16>,

    /// Internal: one tile of a live session.
    #[arg(long = "live-tile", hide = true)]
    pub tile: bool,
}

impl Default for LiveArgs {
    fn default() -> Self {
        Self {
            live: false,
            accept_eula: false,
            version: None,
            view_distance: crate::live::DEFAULT_VIEW_DISTANCE,
            port: None,
            tile: false,
        }
    }
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
    /// Real elevation terrain with OSM water and nature, but no buildings or roads
    TerrainOnly,
    /// Roads, water and nature on real elevation, with buildings as outlines only
    Blueprint,
}

impl GenerationMode {
    /// Whether real elevation is fetched and applied instead of flat ground.
    pub fn terrain(self) -> bool {
        !matches!(self, GenerationMode::GeoOnly)
    }

    /// Whether every man-made object is left out: buildings, roads, Overture, landmarks.
    pub fn skip_objects(self) -> bool {
        matches!(self, GenerationMode::TerrainOnly)
    }

    /// Mode named by the GUI, as `--mode` spells it. Unknown names fall back to the default.
    pub fn from_name(name: &str) -> Self {
        <Self as clap::ValueEnum>::from_str(name, true).unwrap_or_default()
    }

    /// Whether built-up land and fields in the land cover turn back into natural cover. A
    /// blueprint draws its plan on that clean ground, not on paving where buildings would be.
    pub fn natural_land_cover(self) -> bool {
        matches!(
            self,
            GenerationMode::TerrainOnly | GenerationMode::Blueprint
        )
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

/// Range of `--height-multiplier`. Past 10 even a gentle hill outgrows vanilla build height.
pub const MIN_HEIGHT_MULTIPLIER: f64 = 0.1;
pub const MAX_HEIGHT_MULTIPLIER: f64 = 10.0;

pub fn validate_height_multiplier(multiplier: f64) -> Result<(), String> {
    if !(MIN_HEIGHT_MULTIPLIER..=MAX_HEIGHT_MULTIPLIER).contains(&multiplier) {
        return Err(format!(
            "Terrain height multiplier must be between {MIN_HEIGHT_MULTIPLIER} and {MAX_HEIGHT_MULTIPLIER} (got {multiplier})."
        ));
    }
    Ok(())
}

fn parse_height_multiplier(arg: &str) -> Result<f64, String> {
    let multiplier: f64 = arg
        .parse()
        .map_err(|_| format!("`{arg}` is not a number"))?;
    validate_height_multiplier(multiplier)?;
    Ok(multiplier)
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

    /// The prop families this run places: `--props`, else all or none by `--no-3d`;
    /// none below `--props-min-scale`.
    pub fn props(&self) -> crate::structures::PropSet {
        use crate::structures::PropSet;
        if self.props_min_scale.is_some_and(|min| self.scale < min) {
            return PropSet::NONE;
        }
        self.props.unwrap_or(if self.use_3d {
            PropSet::ALL
        } else {
            PropSet::NONE
        })
    }

    /// Whether objects are being skipped only because the scale is below `OBJECT_SKIP_SCALE`.
    pub fn skip_objects_due_to_scale(&self) -> bool {
        !self.mode.skip_objects() && self.scale < OBJECT_SKIP_SCALE
    }

    /// Whether no OpenStreetMap data is loaded at all: too small a scale, or another body.
    /// Terrain-only still loads it, for the water and nature it keeps.
    pub fn skip_osm(&self) -> bool {
        self.scale < OBJECT_SKIP_SCALE || !self.body.is_earth()
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

/// What a Java world generates past the area Arnis wrote.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum WorldType {
    /// Nothing: the area is an island in the void.
    Void,
    /// A superflat grass plain at the area's ground level.
    Flat,
}

impl WorldType {
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "flat" => WorldType::Flat,
            _ => WorldType::Void,
        }
    }
}

/// What spawns in the world on its own. Monsters only spawn above Peaceful on every version
/// Arnis supports, so the difficulty is what separates the two kinds.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum MobSpawning {
    /// Nothing, as the game rules Arnis has always written.
    #[default]
    Off,
    /// Animals, fish and bats, on Peaceful.
    Animals,
    /// Animals and monsters, on Normal, with mob griefing off.
    All,
}

impl MobSpawning {
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "animals" => MobSpawning::Animals,
            "all" => MobSpawning::All,
            _ => MobSpawning::Off,
        }
    }

    /// The `doMobSpawning` game rule.
    pub fn spawns(self) -> bool {
        self != MobSpawning::Off
    }

    /// Whether monsters come too, which takes a difficulty above Peaceful and mob griefing
    /// off, so creepers and endermen cannot take the generated city apart.
    pub fn monsters(self) -> bool {
        self == MobSpawning::All
    }

    /// Normal for monsters, else Peaceful, as the template has always been.
    pub fn java_difficulty(self) -> i8 {
        if self.monsters() {
            2
        } else {
            0
        }
    }

    /// Normal, as Arnis has always written, except Peaceful where only animals are to spawn.
    pub fn bedrock_difficulty(self) -> i32 {
        match self {
            MobSpawning::Animals => 0,
            MobSpawning::Off | MobSpawning::All => 2,
        }
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

/// Lowest ground a world with disasters is built on. The default puts the lowest ground two
/// blocks above bedrock, which leaves sinkholes and cracks nothing to open into.
pub const DISASTER_GROUND_LEVEL: i32 = -40;

/// What some features need from the rest of the world: caves carve into the filled ground,
/// and disasters dig holes in it, with rock under even the lowest ground. The Zombie
/// Outbreak sends its own waves, and Disasters play on Normal, which would let monsters in
/// beside Animals, and need mob griefing for their meteors to crater, so both keep natural
/// spawning off.
pub fn apply_feature_needs(args: &mut Args) {
    // Overture only adds buildings and Mapillary only dresses them, so --no-buildings has
    // nothing to fetch.
    args.overture &= args.buildings;
    if !args.buildings {
        args.mapillary_facades = Some(false);
    }
    if args.caves || args.disasters {
        args.fillground = true;
    }
    if args.disasters {
        args.ground_level = args.ground_level.max(DISASTER_GROUND_LEVEL);
    }
    if args.zombie_outbreak || args.disasters {
        args.mob_spawning = MobSpawning::Off;
    }
}

fn parse_props_min_scale(s: &str) -> Result<f64, String> {
    match s.trim().parse::<f64>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Ok(v),
        _ => Err(format!("{s}: expected a scale of 0 or more")),
    }
}

/// Validates CLI arguments after parsing.
/// For Java Edition: `--path` is required. If the directory doesn't exist, it will be created.
/// For Bedrock Edition (`--bedrock`): `--path` is optional (defaults to Desktop output).
/// Non-Earth bodies drive their own scale and force terrain-only. Shared by the
/// CLI and the GUI so the two cannot disagree about what Moon/Mars means, or
/// what a blueprint leaves out.
pub fn apply_body_defaults(args: &mut Args) {
    apply_mode_defaults(args);
    if args.body.is_earth() {
        return;
    }
    // Snow and ruins dress Earth weather and Earth towns.
    args.winter = false;
    args.post_apocalyptic = false;
    // The superflat template is a grass plain with villages.
    args.world_type = WorldType::Void;
    args.scale = args.body.world_scale();
    args.mode = GenerationMode::TerrainOnly;
    args.overture = false;
    args.canopy_height = false;
    args.use_3d = false;
    args.interior = false;
    args.legacy_trees = false;
    args.aws_only_elevation = false;
    // The cave passes dress an Earth underground: lush moss, rivers, coral reefs.
    args.caves = false;
    // Terrain only means no walls, so neither facade source has anything to
    // hang a photograph on. Cleared rather than left set so the run does not
    // pay for a Mapillary fetch or a resource pack it cannot use.
    args.building_facades = false;
    args.mapillary_facades = Some(false);
    // Relief already fits vanilla height, so the pack would add only empty sky.
    args.disable_height_limit = false;
    args.vanilla_height = true;
    // No roads or places off Earth.
    args.traffic_lights = false;
    args.discoveries = false;
    args.moving_vehicles = false;
    args.pedestrians = false;
    // Nothing lives there, and the one barren biome would still spawn zombies.
    args.mob_spawning = MobSpawning::Off;
    // Airless bodies look right at night. Only a default: comparing against the
    // flag's own default lets an explicit --world-time win.
    if args.world_time == DEFAULT_WORLD_TIME {
        args.world_time = MIDNIGHT_TICKS;
    }
}

/// A blueprint shows where buildings stand, not what they look like, so whatever
/// dresses a building or stands in for one is off, and so is street furniture.
fn apply_mode_defaults(args: &mut Args) {
    if args.mode != GenerationMode::Blueprint {
        return;
    }
    args.use_3d = false;
    args.interior = false;
    args.building_facades = false;
    args.mapillary_facades = Some(false);
    args.signage = SignageLevel::None;
    // Signal nodes are left out, so cars would stop at lights that were never built.
    args.traffic_lights = false;
}

/// Clap's `--world-time` default, so `apply_body_defaults` can tell left-alone
/// from explicitly-set.
pub const DEFAULT_WORLD_TIME: i64 = 6_000;
/// Minecraft tick for midnight (tick 0 is 06:00).
pub const MIDNIGHT_TICKS: i64 = 18_000;

/// Reads --polygon/--polygon-file into `args.shapes` and sets --bbox to their rectangle.
pub fn resolve_shapes(args: &mut Args) -> Result<(), String> {
    use crate::coordinate_system::geographic::LLShapes;
    let mut polygons = Vec::new();
    for (i, points) in args.polygon.iter().enumerate() {
        let ring =
            LLShapes::parse_points(points).map_err(|e| format!("--polygon {}: {e}", i + 1))?;
        polygons.push(vec![ring]);
    }
    if let Some(path) = &args.polygon_file {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("--polygon-file {}: {e}", path.display()))?;
        polygons.extend(
            LLShapes::from_geojson(&text)
                .map_err(|e| format!("--polygon-file {}: {e}", path.display()))?,
        );
    }
    if polygons.is_empty() {
        return Ok(());
    }
    let shapes = LLShapes::new(polygons).map_err(|e| format!("--polygon: {e}"))?;
    args.bbox = Some(shapes.bbox());
    args.shapes = Some(shapes);
    Ok(())
}

pub fn validate_args(args: &Args) -> Result<(), String> {
    let cave_preview = args.cave_zone_map.is_some();
    if (args.cave_style.is_some() || args.cave_seed.is_some()) && !args.caves && !cave_preview {
        return Err(
            "--cave-style and --cave-seed only apply to --caves or --cave-zone-map.".to_string(),
        );
    }
    if args.cave_ores.is_some() && !args.caves {
        return Err("--cave-ores only applies to --caves.".to_string());
    }
    if let Some(y) = args.cave_datum_y {
        if !args.caves {
            return Err("--cave-datum-y only applies to --caves.".to_string());
        }
        if y.rem_euclid(16) != 0 || !(-2032..=2016).contains(&y) {
            return Err("--cave-datum-y must be a multiple of 16 from -2032 to 2016.".to_string());
        }
        // A One World always has the extended height; a run that picks its own may extend.
        let extended =
            args.disable_height_limit || args.one_world || crate::ground::auto_height(args);
        let (bottom, top) = (
            crate::ground::world_min_y(args, extended),
            crate::ground::world_top_y(args, extended),
        );
        if !(bottom..=top).contains(&y) {
            return Err(format!(
                "--cave-datum-y must lie inside this world's height, {bottom} to {top} (got {y})."
            ));
        }
    }
    for warning in ignored_flags(args) {
        eprintln!("Warning: {warning}");
    }
    if !args.one_world
        && (args.units.plan_units.is_some()
            || args.units.unit_regions.is_some()
            || args.units.one_world_unit.is_some()
            || args.units.one_world_workers.is_some()
            || args.units.only_pieces.is_some())
    {
        return Err(
            "--plan-units, --unit-regions, --one-world-workers and --only-pieces only apply to --one-world."
                .to_string(),
        );
    }
    if (args.units.skip_built || args.units.piece_order.is_some() || args.units.stitch_map)
        && !args.units.coordinates()
    {
        return Err(
            "--skip-built, --piece-order and --stitch-map only apply to a job built in pieces (--unit-regions)."
                .to_string(),
        );
    }
    if args.units.stitch_map && !args.map_item {
        return Err("--stitch-map redraws the map item, so it needs --map-item.".to_string());
    }
    let planned = args.units.coordinates() || args.units.plan_units.is_some();
    if (args.units.snap_regions.is_some()
        || args.units.margin_regions != 0
        || args.units.plan_mask.is_some())
        && !planned
    {
        return Err(
            "--snap-regions, --margin-regions and --plan-mask only apply to a job built in pieces (--unit-regions or --plan-units)."
                .to_string(),
        );
    }
    if args.units.only_pieces.is_some() && !args.units.coordinates() {
        return Err(
            "--only-pieces only applies to a job built in pieces (--unit-regions).".to_string(),
        );
    }
    if args.units.skip_cells.is_some() && !args.units.coordinates() {
        return Err(
            "--skip-cells only applies to a job built in pieces (--unit-regions).".to_string(),
        );
    }
    refuse_live_extras(args)?;
    crate::output_args::validate(args)?;
    if args.climate_map.is_some() && args.bbox.is_none() {
        return Err("--climate-map needs --bbox.".to_string());
    }
    // Rotation resamples the ground grids, which the per-position climate does not follow.
    if args.climate_mode == crate::climate_field::ClimateMode::PerPosition && args.rotation != 0.0 {
        return Err("--climate-mode per-position does not support --rotation.".to_string());
    }
    crate::y_bounds::check(args)?;
    if args.prewarm_first && !args.units.coordinates() {
        return Err(
            "--prewarm-first only applies to a job built in pieces (--unit-regions).".to_string(),
        );
    }
    if args.offline && args.mapillary_probe {
        return Err("--mapillary-probe asks Mapillary, so it cannot run --offline.".to_string());
    }
    // One World and live mode track their areas as rectangles of whole chunks.
    if args.shapes.is_some() && (args.one_world || args.live.live) {
        return Err(
            "--polygon makes a world of its own; --one-world and --live generate rectangles."
                .to_string(),
        );
    }

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

    let extras = [
        (args.pause_menu, "--pause-menu"),
        (args.discoveries, "--discoveries"),
        (args.traffic_lights, "--traffic-lights"),
        (args.zombie_outbreak, "--zombie-outbreak"),
        (args.disasters, "--disasters"),
        (args.body_gravity, "--body-gravity"),
        (args.moving_vehicles, "--moving-vehicles"),
        (args.pedestrians, "--pedestrians"),
    ];
    if args.bedrock || args.luanti {
        // Bedrock runs the vehicles from an add-on of its own.
        let java_only = |flag: &str| args.luanti || flag != "--moving-vehicles";
        if let Some((_, flag)) = extras.iter().find(|(on, flag)| *on && java_only(flag)) {
            return Err(format!(
                "{flag} runs from a Java Edition data pack and is not available for Bedrock or Luanti worlds."
            ));
        }
    }
    // Each area would write its pack over the last one's.
    if args.one_world {
        if let Some((_, flag)) = extras.iter().find(|(on, _)| *on) {
            return Err(format!("{flag} is not available with --one-world yet."));
        }
    }
    if args.body_gravity && args.body.is_earth() {
        return Err("--body-gravity only applies to --body moon or --body mars.".to_string());
    }

    if args.map_preview && args.luanti {
        return Err("--map-preview is not supported for Luanti worlds.".to_string());
    }

    // Mineclonia spawns by its own settings, which live outside the world.
    if args.mob_spawning.spawns() && args.luanti {
        return Err("--mob-spawning is not available for Luanti worlds.".to_string());
    }

    if args.one_world {
        if args.bedrock || args.luanti {
            return Err("--one-world is available for Java Edition worlds only.".to_string());
        }
        if !args.body.is_earth() {
            return Err("--one-world is available for Earth only.".to_string());
        }
        if args.rotation.abs() > f64::EPSILON {
            return Err(
                "--one-world keeps the world aligned to real-world coordinates; --rotation must be 0."
                    .to_string(),
            );
        }
        if args.mapillary_probe {
            return Err("--one-world and --mapillary-probe do not combine.".to_string());
        }
    } else if args.world_name.is_some() {
        return Err("--world-name only applies to --one-world.".to_string());
    }
    if args.projection == crate::projection::ProjectionKind::WebMercator && !args.one_world {
        println!(
            "Note: --projection web_mercator centres the world on the bbox; scale is exact at the centre latitude only."
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
    if let Some(dir) = &args.tree_pack_dir {
        if !dir.is_dir() {
            return Err(format!(
                "--tree-pack-dir: {} is not a directory.",
                dir.display()
            ));
        }
    }

    let cave_preview = args.cave_zone_map.is_some();
    if let Some(spec) = &args.cave_biomes {
        crate::caves::decoration::BiomeAmounts::parse(spec)
            .map_err(|e| format!("--cave-biomes: {e}"))?;
        if !args.caves && !cave_preview {
            return Err("--cave-biomes only applies to --caves or --cave-zone-map.".to_string());
        }
    }
    if let Some(dir) = &args.cave_asset_pack {
        if !args.caves {
            return Err("--cave-asset-pack only applies to --caves.".to_string());
        }
        if !dir.join("cave_pack.json").is_file() {
            return Err(format!(
                "--cave-asset-pack: {} has no cave_pack.json.",
                dir.display()
            ));
        }
    }
    if args.cave_zone_map_step.is_some() && !cave_preview {
        return Err("--cave-zone-map-step only applies to --cave-zone-map.".to_string());
    }
    if cave_preview {
        if args.bbox.is_none() {
            return Err("--cave-zone-map needs --bbox.".to_string());
        }
        // The images are laid out on the unrotated bbox, so a rotated world would not match.
        if args.rotation != 0.0 {
            return Err("--cave-zone-map does not support --rotation.".to_string());
        }
    }

    // A bounding box is required unless a local --file supplies one to derive it from.
    // A run that loads no OSM data ignores --file, so it always needs --bbox.
    // Live mode can continue a One World where its players left off.
    if args.bbox.is_none() && !args.live.live {
        if args.skip_osm() {
            return Err(
                "--bbox is required: below scale 0.3, or off Earth, no OpenStreetMap data is loaded and --file is ignored."
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
    } else if args.mapillary_probe || cave_preview || args.climate_map.is_some() || args.prewarm {
        // The probe, the cave preview, the climate map and a prewarm write no world, so they need
        // no output directory.
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

            // Validate that spawn point is within the bounding box. A file-derived bbox is
            // checked in run_cli once the file is parsed.
            if let Some(bbox) = args.bbox {
                if !bbox.contains(&llpoint) {
                    return Err(
                        "Spawn point (--spawn-lat, --spawn-lng) must be within the bounding box."
                            .to_string(),
                    );
                }
            }
            if args.shapes.as_ref().is_some_and(|s| !s.contains(&llpoint)) {
                return Err(
                    "Spawn point (--spawn-lat, --spawn-lng) must be inside the --polygon shapes."
                        .to_string(),
                );
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

/// Flags that were given but do nothing with the rest of the command line.
fn ignored_flags(args: &Args) -> Vec<&'static str> {
    let scatter = ScatterArgs::default();
    let pack_flags = args.tree_realm.is_some()
        || args.tree_size_weights.is_some()
        || args.tree_pack_dir.is_some();
    [
        (
            args.loot_table.is_some() && !args.interior,
            "--loot-table only applies to --interior; it is ignored.",
        ),
        (
            !args.scatter.rocks && args.scatter.rock_density != scatter.rock_density,
            "--rock-density does nothing with --rocks false.",
        ),
        (
            !args.scatter.bushes && args.scatter.bush_density != scatter.bush_density,
            "--bush-density does nothing with --bushes false.",
        ),
        (
            args.blinear_level != 6
                && args.region_format != crate::output_args::RegionFormat::Blinear,
            "--blinear-level only applies to --region-format blinear; it is ignored.",
        ),
        (
            pack_flags && args.legacy_trees,
            "--tree-realm, --tree-size-weights and --tree-pack-dir do nothing with --legacy-trees.",
        ),
    ]
    .into_iter()
    .filter_map(|(ignored, warning)| ignored.then_some(warning))
    .collect()
}

/// Legal `--ground-level` range for the chosen output format. The scaler clamps every column
/// into `[ground_level, ceiling]`, so a base above the ceiling makes that clamp's min exceed
/// its max and aborts the run after the whole elevation download. A run that picks its build
/// height from the area may use the extended range: a base outside vanilla's makes it extend.
fn ground_level_bounds(args: &Args) -> (i32, i32) {
    let extended = args.disable_height_limit || crate::ground::auto_height(args);
    let range = crate::ground::ground_level_range(args, extended);
    (*range.start(), *range.end())
}

fn parse_duration(arg: &str) -> Result<std::time::Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(std::time::Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_that_do_nothing_are_named_and_kept() {
        let p = |x: &[&str]| {
            Args::try_parse_from(
                ["arnis", "--output-dir", ".", "--bbox", "1,2,3,4"]
                    .iter()
                    .chain(x),
            )
            .unwrap()
        };
        assert!(ignored_flags(&p(&[])).is_empty());
        for (flags, named) in [
            (&["--loot-table", "x.json"][..], "--loot-table"),
            (
                &["--rocks", "false", "--rock-density", "0.5"],
                "--rock-density",
            ),
            (
                &["--bushes", "false", "--bush-density", "0.5"],
                "--bush-density",
            ),
            (&["--blinear-level", "9"], "--blinear-level"),
            (&["--legacy-trees", "--tree-realm", "eur"], "--legacy-trees"),
        ] {
            let args = p(flags);
            let warned = ignored_flags(&args);
            assert!(warned.len() == 1 && warned[0].contains(named), "{flags:?}");
            assert!(validate_args(&args).is_ok(), "{flags:?}");
        }
        // On by default: no land cover is no reason to speak up.
        assert!(ignored_flags(&p(&["--rock-density", "0.5", "--mode", "geo-only"])).is_empty());
        assert!(ignored_flags(&p(&["--interior", "--loot-table", "x.json"])).is_empty());
    }

    #[test]
    fn cave_datum_stays_inside_the_world() {
        let p = |x: &[&str]| {
            Args::try_parse_from(
                ["arnis", "--output-dir", ".", "--bbox", "1,2,3,4", "--caves"]
                    .iter()
                    .chain(x),
            )
            .unwrap()
        };
        let vanilla = |y: &str| validate_args(&p(&["--vanilla-height", "--cave-datum-y", y]));
        assert!(vanilla("-64").is_ok() && vanilla("304").is_ok());
        assert!(vanilla("-128").unwrap_err().contains("-64 to 319"));
        assert!(vanilla("2016").is_err());
        assert!(validate_args(&p(&["--disable-height-limit", "--cave-datum-y", "-1024"])).is_ok());
    }

    #[test]
    fn capabilities_lists_every_long_flag() {
        use clap::CommandFactory;
        let line = capabilities_json();
        assert!(line.starts_with('[') && !line.contains('\n'));
        let v: Vec<String> = serde_json::from_str(&line).unwrap();
        let flags: Vec<&str> = v.iter().map(String::as_str).collect();
        for a in Args::command().get_arguments() {
            if let Some(l) = a.get_long() {
                assert!(flags.contains(&l), "missing --{l}");
            }
        }
        assert!(flags.contains(&"threads") && flags.contains(&"capabilities"));
    }

    #[test]
    fn piece_fitting_flags_need_a_job_and_no_live() {
        let p = |x: &[&str]| Args::try_parse_from(["arnis"].iter().chain(x));
        let refused = |x: &[&str], flag: &str| {
            let err = validate_args(&p(x).unwrap()).err().unwrap_or_default();
            err.contains(flag)
        };
        let job = ["--one-world", "--bbox", "44.44,26.09,44.45,26.10"];
        let with = |extra: &[&'static str]| [&job[..], extra].concat();
        assert!(refused(
            &with(&["--snap-regions", "cover"]),
            "--snap-regions"
        ));
        assert!(refused(
            &with(&["--margin-regions", "2"]),
            "--margin-regions"
        ));
        for ok in [
            with(&[
                "--unit-regions",
                "2",
                "--snap-regions",
                "fit",
                "--margin-regions",
                "1",
            ]),
            with(&["--plan-units", "2", "--snap-regions", "merge"]),
        ] {
            assert!(!refused(&ok, "--snap-regions"), "{ok:?}");
        }
        for flags in [["--snap-regions", "cover"], ["--margin-regions", "1"]] {
            let live = p(&[&["--live", "--one-world"], &flags[..]].concat()).unwrap();
            let err = refuse_live_extras(&live).unwrap_err();
            assert!(err.contains(flags[0]), "{err}");
        }
        assert!(p(&["--snap-regions", "square"]).is_err());
        assert!(p(&["--margin-regions", "-1"]).is_err());
        assert_eq!(p(&[]).unwrap().units.margin_regions, 0);
        assert!(refused(&with(&["--skip-built"]), "--skip-built"));
        assert!(!refused(
            &with(&["--unit-regions", "2", "--skip-built"]),
            "--skip-built"
        ));
        let order = |v: &str| p(&["--piece-order", v]).map(|a| a.units.piece_order);
        assert_eq!(order("west").unwrap(), Some(PieceOrder::West));
        assert_eq!(order("center").unwrap(), Some(PieceOrder::Centre));
        assert_eq!(order("3,1").unwrap(), Some(PieceOrder::List(vec![3, 1])));
        assert!(order("0,1").is_err() && order("up").is_err());
        assert!(refused(&with(&["--piece-order", "west"]), "--piece-order"));
        assert!(refused(&with(&["--plan-mask", "m.geojson"]), "--plan-mask"));
        assert!(refused(&with(&["--stitch-map"]), "--stitch-map"));
        assert!(!refused(
            &with(&["--unit-regions", "1", "--stitch-map"]),
            "--stitch-map"
        ));
        let live = p(&["--live", "--one-world", "--stitch-map"]).unwrap();
        assert!(refuse_live_extras(&live)
            .unwrap_err()
            .contains("--stitch-map"));
        let live = p(&["--live", "--one-world", "--plan-mask", "m.geojson"]).unwrap();
        assert!(refuse_live_extras(&live)
            .unwrap_err()
            .contains("--plan-mask"));
        let live = p(&["--live", "--one-world", "--skip-built"]).unwrap();
        assert!(refuse_live_extras(&live)
            .unwrap_err()
            .contains("--skip-built"));
        let size = |v: &str| p(&["--unit-regions", v]).map(|a| a.units.unit_regions);
        assert_eq!(size("auto").unwrap(), Some(UnitSize::Auto));
        assert_eq!(size("4").unwrap(), Some(UnitSize::N(4)));
        assert!(size("0").is_err() && size("65").is_err() && size("big").is_err());
        let plan = p(&["--plan-units", "auto"]).unwrap().units.plan_units;
        assert_eq!(plan, Some(UnitSize::Auto));
    }

    #[test]
    fn threads_must_be_positive() {
        let p = |x: &[&str]| Args::try_parse_from(["arnis"].iter().chain(x));
        assert!(p(&["--threads", "0"]).is_err());
        assert_eq!(p(&["--threads", "4"]).unwrap().thread_count(), Some(4));
        assert_eq!(p(&[]).unwrap().thread_count(), None);
    }

    #[test]
    fn gameplay_extras_are_java_only_but_vehicles_and_gravity_needs_another_body() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            let mut args = Args::parse_from(cmd.iter());
            apply_body_defaults(&mut args);
            args
        };
        assert!(validate_args(&parse(&["--traffic-lights", "--zombie-outbreak"])).is_ok());
        let bedrock = validate_args(&parse(&["--bedrock", "--discoveries"])).unwrap_err();
        assert!(bedrock.contains("--discoveries"), "{bedrock}");
        // Vehicles come with an add-on of their own on Bedrock, but not on Luanti.
        assert!(validate_args(&parse(&["--bedrock", "--moving-vehicles"])).is_ok());
        let luanti = validate_args(&parse(&["--luanti", "--moving-vehicles"])).unwrap_err();
        assert!(luanti.contains("--moving-vehicles"), "{luanti}");
        // Walkers are Java only.
        let walkers = validate_args(&parse(&["--bedrock", "--pedestrians"])).unwrap_err();
        assert!(walkers.contains("--pedestrians"), "{walkers}");
        assert!(validate_args(&parse(&["--pedestrians", "--moving-vehicles"])).is_ok());
        let lights = validate_args(&parse(&[
            "--bedrock",
            "--moving-vehicles",
            "--traffic-lights",
        ]))
        .unwrap_err();
        assert!(lights.contains("--traffic-lights"), "{lights}");
        assert!(validate_args(&parse(&["--body-gravity"])).is_err());
        assert!(validate_args(&parse(&["--body", "moon", "--body-gravity"])).is_ok());
        let one_world = validate_args(&parse(&["--one-world", "--pause-menu"])).unwrap_err();
        assert!(one_world.contains("--pause-menu"), "{one_world}");
    }

    #[test]
    fn missing_tree_pack_dir_is_refused() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let check = |dir: &str| {
            let mut args = Args::parse_from([
                "arnis",
                "--output-dir",
                tmp_path,
                "--bbox",
                "1,2,3,4",
                "--tree-pack-dir",
                dir,
            ]);
            apply_body_defaults(&mut args);
            validate_args(&args)
        };
        assert!(check(tmp_path).is_ok());
        let missing = tmpdir.path().join("nope");
        let err = check(missing.to_str().unwrap()).unwrap_err();
        assert!(err.contains("--tree-pack-dir"), "{err}");
    }

    #[test]
    fn disasters_bring_filled_ground_with_rock_under_the_lowest_point() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            let mut args = Args::parse_from(cmd.iter());
            apply_body_defaults(&mut args);
            apply_feature_needs(&mut args);
            args
        };
        let plain = parse(&[]);
        assert!(!plain.fillground);
        assert_eq!(plain.ground_level, -62);
        let disasters = parse(&["--disasters"]);
        assert!(disasters.fillground);
        assert_eq!(disasters.ground_level, DISASTER_GROUND_LEVEL);
        assert!(validate_args(&disasters).is_ok());
        let higher = parse(&["--disasters", "--ground-level", "10"]);
        assert_eq!(higher.ground_level, 10);
        assert!(parse(&["--caves"]).fillground);
    }

    #[test]
    fn mob_spawning_stays_off_where_the_world_brings_its_own_mobs() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            let mut args = Args::parse_from(cmd.iter());
            apply_body_defaults(&mut args);
            apply_feature_needs(&mut args);
            args
        };
        assert_eq!(parse(&[]).mob_spawning, MobSpawning::Off);
        let animals = parse(&["--mob-spawning", "animals"]);
        assert_eq!(animals.mob_spawning, MobSpawning::Animals);
        assert!(validate_args(&animals).is_ok());
        let bedrock = parse(&["--bedrock", "--mob-spawning", "all"]);
        assert_eq!(bedrock.mob_spawning, MobSpawning::All);
        assert!(validate_args(&bedrock).is_ok());

        for extra in [
            &["--zombie-outbreak"][..],
            &["--disasters"],
            &["--body", "moon"],
        ] {
            let mut cmd = extra.to_vec();
            cmd.extend(["--mob-spawning", "all"]);
            assert_eq!(parse(&cmd).mob_spawning, MobSpawning::Off, "{extra:?}");
        }

        let luanti = validate_args(&parse(&["--luanti", "--mob-spawning", "animals"])).unwrap_err();
        assert!(luanti.contains("--mob-spawning"), "{luanti}");
        assert!(validate_args(&parse(&["--luanti"])).is_ok());

        assert_eq!(MobSpawning::from_str_lossy("animals"), MobSpawning::Animals);
        assert_eq!(MobSpawning::from_str_lossy("all"), MobSpawning::All);
        assert_eq!(MobSpawning::from_str_lossy("bogus"), MobSpawning::Off);
        assert!(!MobSpawning::Off.spawns() && !MobSpawning::Off.monsters());
        assert!(MobSpawning::Animals.spawns() && !MobSpawning::Animals.monsters());
        assert!(MobSpawning::All.spawns() && MobSpawning::All.monsters());
        let difficulties = [MobSpawning::Off, MobSpawning::Animals, MobSpawning::All]
            .map(|m| (m.java_difficulty(), m.bedrock_difficulty()));
        assert_eq!(difficulties, [(0, 2), (0, 0), (2, 2)]);
    }

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
    fn web_mercator_projection_is_accepted() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };

        assert!(validate_args(&parse(&["--projection", "local"])).is_ok());
        assert!(validate_args(&parse(&["--projection", "web_mercator"])).is_ok());
        assert!(validate_args(&parse(&["--projection", "mercator", "--mode", "geo-only"])).is_ok());
    }

    #[test]
    fn height_multiplier_defaults_to_real_height_and_is_bounded() {
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", ".", "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::try_parse_from(cmd.iter())
        };

        assert_eq!(parse(&[]).unwrap().height_multiplier, 1.0);
        assert_eq!(
            parse(&["--height-multiplier", "2.5"])
                .unwrap()
                .height_multiplier,
            2.5
        );
        for bad in ["0", "0.05", "11", "NaN", "inf", "tall"] {
            assert!(
                parse(&["--height-multiplier", bad]).is_err(),
                "accepted {bad}"
            );
        }
    }

    #[test]
    fn one_world_is_java_earth_and_unrotated() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };

        assert!(validate_args(&parse(&["--one-world"])).is_ok());
        assert!(validate_args(&parse(&["--one-world", "--world-name", "Home"])).is_ok());
        assert!(validate_args(&parse(&["--one-world", "--bedrock"])).is_err());
        assert!(validate_args(&parse(&["--one-world", "--luanti"])).is_err());
        assert!(validate_args(&parse(&["--one-world", "--rotation", "15"])).is_err());
        assert!(validate_args(&parse(&["--one-world", "--body", "moon"])).is_err());
        assert!(validate_args(&parse(&["--world-name", "Home"])).is_err());
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
        let vanilla: &[&str] = &["--vanilla-height"];
        assert!(gl(vanilla, 304).is_ok());
        assert!(gl(vanilla, 305).is_err());
        assert!(gl(vanilla, 400).is_err());
        // Below the bedrock plane there is nothing to stand on.
        assert!(gl(vanilla, -63).is_err());

        // The tall datapack raises the ceiling to 2031; Bedrock's pack stops far below it.
        // By default a base outside vanilla's range is what makes the run extend.
        for extended in [&[][..], &["--disable-height-limit"][..]] {
            assert!(gl(extended, 2016).is_ok(), "{extended:?}");
            assert!(gl(extended, 2017).is_err(), "{extended:?}");
            assert!(gl(extended, -2030).is_ok(), "{extended:?}");
            assert!(gl(extended, -2031).is_err(), "{extended:?}");
        }
        assert!(gl(&["--bedrock", "--disable-height-limit"], 500).is_err());
        assert!(gl(&["--bedrock"], 500).is_err());
    }

    #[test]
    fn vanilla_height_and_the_forced_pack_exclude_each_other() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let cmd = |extra: &[&'static str]| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            cmd
        };
        let args = Args::try_parse_from(cmd(&["--vanilla-height"])).unwrap();
        assert!(args.vanilla_height && !args.disable_height_limit);
        assert!(
            Args::try_parse_from(cmd(&["--vanilla-height", "--disable-height-limit"])).is_err()
        );
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
            &["--bedrock", "--vanilla-height"][..],
            &["--body", "moon"][..],
        ] {
            assert!(gl(flags, -62).is_ok(), "{flags:?} rejected the default");
            assert!(gl(flags, 304).is_ok(), "{flags:?} rejected 304");
            assert!(gl(flags, 305).is_err(), "{flags:?} accepted 305");
            assert!(gl(flags, -63).is_err(), "{flags:?} accepted -63");
        }

        // Bedrock's behavior pack raises the ceiling to 511 but leaves the floor vanilla.
        for bedrock in [
            &["--bedrock"][..],
            &["--bedrock", "--disable-height-limit"][..],
        ] {
            assert!(gl(bedrock, 496).is_ok(), "{bedrock:?}");
            assert!(gl(bedrock, 497).is_err(), "{bedrock:?}");
            assert!(gl(bedrock, -63).is_err(), "{bedrock:?}");
        }
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
        assert!(!args.dh_lod);
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
    fn live_flags_parse_beside_the_version_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd = [
            "arnis",
            "--output-dir",
            tmp.path().to_str().unwrap(),
            "--one-world",
            "--live",
            "--live-version",
            "26.3",
            "--live-view-distance",
            "16",
        ];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(args.live.live);
        assert_eq!(args.live.version.as_deref(), Some("26.3"));
        assert_eq!(args.live.view_distance, 16);
        assert!(validate_args(&args).is_ok());
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
    fn test_terrain_only_reads_a_file_but_small_scale_needs_a_bbox() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        // Terrain-only loads OSM for its water, so a file can supply the bbox.
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
        assert!(!args.skip_osm());
        assert!(validate_args(&args).is_ok());

        // Below the object scale nothing is loaded, so --file is ignored.
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--file",
            "area.osm",
            "--scale",
            "0.1",
        ];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&args).is_err());
    }

    #[test]
    fn blueprint_turns_off_whatever_dresses_a_building() {
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", "out", "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };
        let mut args = parse(&["--mode", "blueprint", "--interior", "--building-facades"]);
        assert!(args.terrain());
        assert!(!args.skip_objects());
        apply_body_defaults(&mut args);
        assert!(!args.use_3d && !args.interior && !args.building_facades);
        assert_eq!(args.mapillary_facades, Some(false));
    }

    #[test]
    fn a_blueprint_has_no_street_furniture() {
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", "out", "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            let mut args = Args::parse_from(cmd.iter());
            apply_body_defaults(&mut args);
            args
        };
        let args = parse(&["--traffic-lights", "--signage", "full"]);
        assert!(args.traffic_lights && args.signage.full());
        let args = parse(&[
            "--mode",
            "blueprint",
            "--traffic-lights",
            "--signage",
            "full",
        ]);
        assert!(!args.traffic_lights && !args.signage.enabled());
    }

    #[test]
    fn gui_mode_names_match_the_cli() {
        assert_eq!(
            GenerationMode::from_name("blueprint"),
            GenerationMode::Blueprint
        );
        assert_eq!(
            GenerationMode::from_name("terrain-only"),
            GenerationMode::TerrainOnly
        );
        assert_eq!(
            GenerationMode::from_name("geo-only"),
            GenerationMode::GeoOnly
        );
        assert_eq!(
            GenerationMode::from_name("nonsense"),
            GenerationMode::GeoTerrain
        );
    }

    #[test]
    fn winter_and_ruins_stay_on_earth() {
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", "out", "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };
        let mut args = parse(&["--winter", "--post-apocalyptic"]);
        apply_body_defaults(&mut args);
        assert!(args.winter && args.post_apocalyptic);
        let mut args = parse(&["--winter", "--post-apocalyptic", "--body", "moon"]);
        apply_body_defaults(&mut args);
        assert!(!args.winter && !args.post_apocalyptic);
    }

    #[test]
    fn the_grass_plain_stays_on_earth() {
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", "out", "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            let mut args = Args::parse_from(cmd.iter());
            apply_body_defaults(&mut args);
            args
        };
        assert_eq!(parse(&["--world-type", "flat"]).world_type, WorldType::Flat);
        for body in ["moon", "mars"] {
            let args = parse(&["--world-type", "flat", "--body", body]);
            assert_eq!(args.world_type, WorldType::Void);
        }
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
        assert!(!help.contains("hide_env_values"));
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

    #[test]
    fn gameplay_extras_are_refused_in_pieces() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        for flag in ["--traffic-lights", "--pedestrians"] {
            let cmd = [
                "arnis",
                "--output-dir",
                tmp_path,
                "--one-world",
                "--unit-regions=1",
            ];
            let args = Args::parse_from(cmd.iter().chain(&["--bbox=1,2,3,4", flag]));
            assert!(validate_args(&args).unwrap_err().contains(flag));
        }
    }

    #[test]
    fn live_with_advanced_flags_is_refused() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path, "--live", "--one-world"];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };
        // Harmless or manifest-borne flags stay allowed.
        let plain = parse(&["--seed", "7", "--cave-seed", "3", "--no-update-check"]);
        assert!(refuse_live_extras(&plain).is_ok());
        let err = refuse_live_extras(&parse(&[
            "--river-bed",
            "v1",
            "--no-buildings",
            "--surface-patches",
            "--threads",
            "4",
        ]))
        .unwrap_err();
        for name in [
            "--river-bed",
            "--no-buildings",
            "--surface-patches",
            "--threads",
        ] {
            assert!(err.contains(name), "{err}");
        }
        assert!(!err.contains("--offline"), "{err}");
        let err = validate_args(&parse(&["--offline"])).unwrap_err();
        assert!(
            err.contains("--offline is not supported in Live mode"),
            "{err}"
        );
        // Without --live nothing changes.
        let off = Args::parse_from(["arnis", "--river-bed", "v1"]);
        assert!(refuse_live_extras(&off).is_ok());
    }

    #[test]
    fn unit_flags_need_one_world_and_a_sane_range() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };
        assert!(validate_args(&parse(&["--one-world", "--plan-units", "4"])).is_ok());
        assert!(validate_args(&parse(&["--plan-units", "4"])).is_err());
        assert!(validate_args(&parse(&["--unit-regions", "4"])).is_err());
        assert!(validate_args(&parse(&["--one-world", "--unit-regions", "64"])).is_ok());
        for bad in ["0", "65"] {
            assert!(Args::try_parse_from(["arnis", "--plan-units", bad]).is_err());
            assert!(Args::try_parse_from(["arnis", "--unit-regions", bad]).is_err());
        }
        // Pieces built side by side, or only some of them, need a One World job.
        let w = parse(&["--one-world", "--one-world-workers", "auto"]);
        assert!(validate_args(&w).is_ok() && w.units.coordinates());
        assert_eq!(
            parse(&["--one-world", "--one-world-workers", "3"])
                .units
                .one_world_workers,
            Some(Workers::N(3))
        );
        assert!(validate_args(&parse(&["--one-world-workers", "auto"])).is_err());
        assert!(validate_args(&parse(&["--one-world", "--only-pieces", "2"])).is_err());
        let retry = parse(&["--one-world", "--unit-regions", "2", "--only-pieces", "2,5"]);
        assert!(validate_args(&retry).is_ok());
        assert_eq!(retry.units.only_pieces, Some(vec![2, 5]));
        assert!(Args::try_parse_from(["arnis", "--one-world-workers", "0"]).is_err());
        assert!(Args::try_parse_from(["arnis", "--only-pieces", "0"]).is_err());
    }

    #[test]
    fn blinear_with_lod_is_refused() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };
        let bl = ["--region-format", "blinear"];
        assert!(validate_args(&parse(&bl)).is_ok());
        assert_eq!(parse(&bl).blinear_level(), Some(6));
        assert_eq!(parse(&[]).blinear_level(), None);
        for refused in ["--dh-lod", "--voxy-lod", "--one-world", "--bedrock"] {
            let args = parse(&[bl[0], bl[1], refused]);
            assert!(validate_args(&args).is_err(), "{refused}");
        }
        assert!(Args::try_parse_from(["arnis", "--blinear-level", "23"]).is_err());
    }

    #[test]
    fn origin_border_and_map_item_only_validate() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let parse = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };
        assert_eq!(
            parse(&["--origin", "-45.5, 170"]).origin,
            Some((-45.5, 170.0))
        );
        assert!(Args::try_parse_from(["arnis", "--origin", "90,0"]).is_err());
        assert!(validate_args(&parse(&["--origin", "1,2"])).is_err());
        assert!(validate_args(&parse(&["--one-world", "--origin", "1,2"])).is_ok());
        assert!(validate_args(&parse(&["--world-border"])).is_ok());
        assert!(validate_args(&parse(&["--world-border", "--bedrock"])).is_err());
        assert!(validate_args(&parse(&["--map-item-only"])).is_err());
        assert!(validate_args(&parse(&["--dh-lod-only"])).is_err());
        let world_only = Args::parse_from(["arnis", "--one-world", "--map-item-only"]);
        assert!(
            crate::output_args::validate(&world_only).is_ok(),
            "no bbox needed"
        );
        let dh_only = Args::parse_from(["arnis", "--one-world", "--dh-lod-only"]);
        assert!(crate::output_args::validate(&dh_only).is_ok());
    }

    #[test]
    fn no_buildings_skips_overture_fetch() {
        let fetched = |extra: &[&str]| {
            let mut args = Args::parse_from(
                ["arnis", "--bbox", "1,2,3,4", "--mapillary-token", "t"]
                    .iter()
                    .chain(extra),
            );
            apply_feature_needs(&mut args);
            (args.overture, args.mapillary_pipeline_on())
        };
        assert_eq!(fetched(&[]), (true, true));
        assert_eq!(fetched(&["--no-buildings"]), (false, false));
    }

    #[test]
    fn props_override_no_3d() {
        use crate::structures::{Prop, PropSet};
        let parse =
            |extra: &[&str]| Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter().chain(extra));
        assert_eq!(parse(&[]).props(), PropSet::ALL);
        assert_eq!(parse(&["--no-3d"]).props(), PropSet::NONE);
        let car = parse(&["--no-3d", "--props", "car"]).props();
        assert!(car.has(Prop::Car) && !car.has(Prop::Boat));
        assert_eq!(parse(&["--props", "none"]).props(), PropSet::NONE);
    }

    #[test]
    fn below_min_scale_no_props() {
        use crate::structures::PropSet;
        let at = |scale: &str| {
            Args::parse_from([
                "arnis",
                "--bbox",
                "1,2,3,4",
                "--scale",
                scale,
                "--props-min-scale",
                "0.5",
            ])
            .props()
        };
        assert_eq!(at("0.4"), PropSet::NONE);
        assert_eq!(at("0.5"), PropSet::ALL);
        assert!(Args::try_parse_from(["arnis", "--props-min-scale", "-1"]).is_err());
    }

    #[test]
    fn polygons_set_the_area_and_its_rectangle() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let resolved = |extra: &[&str]| {
            let mut cmd = vec!["arnis", "--output-dir", tmp_path];
            cmd.extend_from_slice(extra);
            let mut args = Args::try_parse_from(cmd.iter()).map_err(|e| e.to_string())?;
            resolve_shapes(&mut args)?;
            validate_args(&args)?;
            Ok::<Args, String>(args)
        };

        let tri = "52.50,13.37 52.53,13.37 52.50,13.42";
        let args = resolved(&[
            "--polygon",
            tri,
            "--polygon",
            "52.6,13.5 52.61,13.5 52.61,13.52",
        ])
        .unwrap();
        let shapes = args.shapes.as_ref().unwrap();
        assert_eq!(shapes.polygons().len(), 2);
        let bbox = args.bbox.unwrap();
        assert_eq!((bbox.min().lat(), bbox.min().lng()), (52.50, 13.37));
        assert_eq!((bbox.max().lat(), bbox.max().lng()), (52.61, 13.52));

        // The rectangle and the shape are two ways of saying where; one is enough.
        assert!(resolved(&["--bbox", "1,2,3,4", "--polygon", tri]).is_err());
        assert!(resolved(&["--polygon", "52.5,13.3 52.6,13.3"]).is_err());
        assert!(resolved(&["--polygon", tri, "--one-world"]).is_err());

        let inside = ["--spawn-lat", "52.505", "--spawn-lng", "13.38"];
        assert!(resolved(&[&["--polygon", tri][..], &inside[..]].concat()).is_ok());
        // Inside the rectangle, across the diagonal from the triangle.
        let outside = ["--spawn-lat", "52.528", "--spawn-lng", "13.415"];
        assert!(resolved(&[&["--polygon", tri][..], &outside[..]].concat()).is_err());

        let geojson = tmpdir.path().join("area.geojson");
        std::fs::write(
            &geojson,
            r#"{"type":"Polygon","coordinates":[[[13.37,52.5],[13.42,52.5],[13.42,52.53],[13.37,52.5]]]}"#,
        )
        .unwrap();
        let file = geojson.to_str().unwrap();
        let args = resolved(&["--polygon-file", file, "--polygon", tri]).unwrap();
        assert_eq!(args.shapes.unwrap().polygons().len(), 2);
        assert!(resolved(&["--polygon-file", "/no/such/file.geojson"]).is_err());
    }
}
