//! Hand-built models for landmarks whose OpenStreetMap geometry is too coarse.
//! Matched by Wikidata QID so a model only replaces that one object, and placed
//! from the anchor lat/lon because ways get clipped to the selected area.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use colored::Colorize;
use fastnbt::Value;

use crate::args::Args;
use crate::block_definitions::{
    Block, BlockWithProperties, AIR, LIGHT_GRAY_CONCRETE, POLISHED_BASALT, STONE,
};
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::osm_parser::ProcessedElement;
use crate::structures::schematic::{load_palettized, rotate_props, PalettizedSchematic};
use crate::world_editor::WorldEditor;

/// Where a landmark's blocks come from.
enum Source {
    /// A gzipped Sponge `.schem`.
    Schem(&'static [u8]),
    /// Built in code, laid out as a loaded `.schem`.
    Built(fn() -> PalettizedSchematic),
    /// Built in code to sit in sloping ground: columns at most a layer above `ground_y` follow
    /// the terrain one by one, and each taller part rests on the highest ground it meets, its
    /// footings carried down to the ground under them.
    OnTerrain(fn() -> PalettizedSchematic),
    /// A gzipped Sponge `.schem` placed as `OnTerrain` is.
    SchemOnTerrain(&'static [u8]),
    /// A `.schem` of terrain itself, placed as `Schem` only where the ground keeps its real height.
    Terrain(&'static [u8]),
}

/// One bundled landmark model.
struct Landmark {
    /// Shown in the placement log line.
    name: &'static str,
    /// Wikidata QID of the object this model replaces.
    qid: &'static str,
    /// Fallback identity if the element lost its `wikidata` tag, and mapped parts the model replaces.
    osm_ids: &'static [(&'static str, u64)],
    /// North-up at one block per metre.
    model: Source,
    /// Point the anchor is pinned to.
    lat: f64,
    lon: f64,
    /// Schematic XZ that lands on (`lat`, `lon`).
    anchor_x: f64,
    anchor_z: f64,
    /// Schematic Y sitting at ground level; layers below it get dug in.
    ground_y: i32,
    /// Shift against the sampled ground. Negative digs the model in.
    ground_offset: i32,
    /// Blocks marking the interior, which is dug out. Empty digs out everything.
    interior_marker: &'static [Block],
    /// How far the ground reaches in past the interior, in blocks.
    ground_overlap: i32,
    /// Half-extents in metres of the area whose OSM features this replaces.
    suppress_half_x: f64,
    suppress_half_z: f64,
    /// Farthest the model reaches from the anchor, in metres.
    reach_m: i32,
    /// Blocks the model stands above its ground, so the build height can count it without
    /// decoding it. A test holds it to the model.
    height: i32,
}

/// Anchors are measured by matching each model against its real OSM footprint.
const LANDMARKS: &[Landmark] = &[
    Landmark {
        name: "Olympiastadion München",
        qid: "Q131610",
        // The stadium and the eight concrete backstay anchors the model builds.
        osm_ids: &[
            ("way", 419_656_920),
            ("way", 117_479_842),
            ("way", 117_479_843),
            ("way", 117_479_845),
            ("way", 117_479_850),
            ("way", 117_479_852),
            ("way", 117_479_855),
            ("way", 117_479_856),
            ("way", 117_479_858),
        ],
        model: Source::Schem(include_bytes!(
            "../assets/structures/landmarks/olympiastadion_munich.schem"
        )),
        // Origin of the model's laser survey frame, 4 m WSW of the field centre; true north.
        lat: 48.173_102_3,
        lon: 11.546_496_1,
        anchor_x: 189.5,
        anchor_z: 201.5,
        // The pitch, 8.3 m under the median ground round the bowl.
        ground_y: 3,
        ground_offset: -9,
        // The ring's fill, moat, booth and tunnels; filling the enclosed ring gives the bowl.
        interior_marker: &[LIGHT_GRAY_CONCRETE],
        // Keeps the outer walkway's rim, so its edge meets the terrain.
        ground_overlap: 1,
        // Stops short of the retail units 155 m east and the roofs 210 m north.
        suppress_half_x: 135.0,
        suppress_half_z: 145.0,
        reach_m: 217,
        height: 84,
    },
    Landmark {
        name: "Olympiahalle",
        qid: "Q48849",
        // The hall, its roof and connectors, and the mapped cable anchors it builds.
        osm_ids: &[
            ("way", 303_099_272),
            ("way", 15_805_226),
            ("way", 302_726_370),
            ("way", 117_477_621),
            ("way", 117_470_701),
            ("way", 117_468_801),
            ("way", 117_468_806),
            ("way", 117_470_696),
            ("way", 117_470_697),
            ("way", 117_470_698),
            ("way", 117_470_700),
            ("way", 117_470_703),
            ("way", 117_470_705),
            ("way", 117_470_707),
            ("way", 117_470_708),
            ("way", 117_470_709),
            ("way", 117_470_711),
            ("way", 117_470_712),
            ("way", 117_470_713),
            ("way", 117_470_714),
            ("way", 117_477_617),
            ("way", 117_479_853),
            ("way", 117_523_417),
            ("way", 117_523_419),
            ("way", 117_524_350),
            ("way", 118_239_000),
            ("way", 118_241_446),
            ("way", 118_241_447),
            ("way", 118_241_448),
            ("way", 118_241_449),
            ("way", 118_241_450),
            ("way", 118_241_451),
            ("way", 360_117_190),
            ("way", 360_117_191),
            ("way", 360_117_192),
            ("way", 360_117_193),
            ("way", 360_117_194),
            ("way", 360_341_546),
            ("way", 360_341_549),
            ("way", 360_341_552),
            ("way", 360_341_554),
            ("way", 360_341_560),
            ("way", 360_341_561),
            ("way", 360_341_564),
        ],
        model: Source::Schem(include_bytes!(
            "../assets/structures/landmarks/olympiahalle_munich.schem"
        )),
        // Centroid of the mapped outline; roof, masts and anchors at their laser heights.
        lat: 48.174_924_8,
        lon: 11.550_061_4,
        anchor_x: 222.2,
        anchor_z: 176.3,
        // The layer of the laser's median ground over the sampled grid.
        ground_y: 13,
        ground_offset: 0,
        // A sill under every glass-wall column rings exactly the mapped outline.
        interior_marker: &[POLISHED_BASALT],
        ground_overlap: 0,
        // Short of the admin office and the retail unit, which are not modelled.
        suppress_half_x: 60.0,
        suppress_half_z: 50.0,
        reach_m: 238,
        height: 61,
    },
    Landmark {
        name: "Olympia-Schwimmhalle",
        qid: "Q3882013",
        // The hall, its roof and connector to the Olympiahalle, the lake mast, cables and anchors.
        osm_ids: &[
            ("way", 227_012_665),
            ("way", 15_805_217),
            ("way", 117_468_809),
            ("way", 117_470_710),
            ("way", 117_479_859),
            ("way", 118_153_987),
            ("way", 118_153_988),
            ("way", 118_153_989),
            ("way", 118_153_990),
            ("way", 118_153_991),
            ("way", 118_153_992),
            ("way", 118_153_993),
            ("way", 118_161_792),
            ("way", 118_161_796),
            ("way", 269_819_552),
            ("way", 117_468_804),
            ("way", 117_468_800),
            ("way", 117_468_805),
            ("way", 117_468_808),
            ("way", 117_468_810),
            ("way", 360_117_188),
            ("way", 360_117_189),
            ("way", 360_341_557),
        ],
        model: Source::Schem(include_bytes!(
            "../assets/structures/landmarks/olympia_schwimmhalle_munich.schem"
        )),
        // Centroid of the mapped outline; roof, mast and anchors at their laser heights.
        lat: 48.173_552_8,
        lon: 11.551_536_7,
        anchor_x: 97.9,
        anchor_z: 114.9,
        ground_y: 12,
        ground_offset: 0,
        // A sill under every glass-wall column rings exactly the mapped outline.
        interior_marker: &[POLISHED_BASALT],
        ground_overlap: 0,
        suppress_half_x: 40.0,
        suppress_half_z: 55.0,
        reach_m: 135,
        height: 69,
    },
    Landmark {
        name: "Olympiaturm",
        qid: "Q599148",
        osm_ids: &[("way", 164_084_344)],
        model: Source::Schem(include_bytes!(
            "../assets/structures/landmarks/olympiaturm_munich.schem"
        )),
        // Shaft axis fitted to the laser points.
        lat: 48.174_417_1,
        lon: 11.553_755_7,
        anchor_x: 15.5,
        anchor_z: 15.5,
        ground_y: 0,
        ground_offset: 0,
        interior_marker: &[],
        ground_overlap: 0,
        // Not the visitor entrance building 27 m east.
        suppress_half_x: 22.0,
        suppress_half_z: 22.0,
        reach_m: 22,
        height: 291,
    },
    Landmark {
        name: "Elizabeth Tower",
        qid: "Q41225",
        osm_ids: &[("way", 123_557_148)],
        model: Source::Built(crate::structures::elizabeth_tower::build),
        // Centre of the mapped outline; standing square to its 6.4 degree turn moves corners ~1 m.
        lat: 51.500_701_2,
        lon: -0.124_573_0,
        anchor_x: 8.5,
        anchor_z: 8.5,
        ground_y: 0,
        ground_offset: 0,
        interior_marker: &[],
        ground_overlap: 0,
        // The tower's own parts, not the Palace wings against it.
        suppress_half_x: 8.0,
        suppress_half_z: 8.0,
        reach_m: 13,
        height: 96,
    },
    Landmark {
        name: "Sydney Opera House",
        qid: "Q45178",
        osm_ids: &[("relation", 9_596_872)],
        model: Source::Built(crate::structures::sydney_opera_house::build),
        // Built on the mapped outline, shell parts and harbour edge; anchored mid-podium.
        lat: -33.857_024_6,
        lon: 151.215_191_8,
        anchor_x: 90.0,
        anchor_z: 105.0,
        // The broadwalk sits on the ground, the seawall below it.
        ground_y: crate::structures::sydney_opera_house::GROUND,
        ground_offset: -1,
        interior_marker: &[],
        ground_overlap: 0,
        // The podium and its shells, not the forecourt buildings.
        suppress_half_x: 72.0,
        suppress_half_z: 102.0,
        reach_m: 155,
        height: 64,
    },
    Landmark {
        name: "Brandenburger Tor",
        qid: "Q82425",
        osm_ids: &[("way", 518_071_791), ("relation", 7_288_267)],
        model: Source::Built(crate::structures::brandenburger_tor::build),
        // Middle of the twelve mapped columns. The model is sheared onto the gate's turn.
        lat: 52.516_271_9,
        lon: 13.377_702_2,
        anchor_x: crate::structures::brandenburger_tor::GATE_X as f64 + 0.5,
        anchor_z: crate::structures::brandenburger_tor::GATE_Z as f64 + 0.5,
        ground_y: 0,
        ground_offset: 0,
        interior_marker: &[],
        ground_overlap: 0,
        // Gate, colonnades and gatehouses, not Haus Sommer or Haus Liebermann beside them.
        suppress_half_x: 30.0,
        suppress_half_z: 38.0,
        reach_m: 46,
        height: 26,
    },
    Landmark {
        name: "Reichstagsgebäude",
        qid: "Q151897",
        osm_ids: &[("relation", 2_201_742)],
        model: Source::Built(crate::structures::reichstag::build),
        // Middle of the dome, from the mapped outline and Berlin's surface model.
        lat: 52.518_595_7,
        lon: 13.376_098_4,
        anchor_x: crate::structures::reichstag::ANCHOR.0,
        anchor_z: crate::structures::reichstag::ANCHOR.1,
        ground_y: 0,
        ground_offset: 0,
        interior_marker: &[],
        ground_overlap: 0,
        // The building and its steps, not the visitor pavilions on Platz der Republik.
        suppress_half_x: 64.0,
        suppress_half_z: 74.0,
        reach_m: 92,
        height: 53,
    },
    Landmark {
        name: "Elbphilharmonie",
        qid: "Q673223",
        osm_ids: &[("way", 24_981_342)],
        model: Source::Built(crate::structures::elbphilharmonie::build),
        // Middle of the mapped outline's bounding box, which the model is built on.
        lat: 53.541_250_6,
        lon: 9.984_103_2,
        anchor_x: 60.0,
        anchor_z: 44.5,
        ground_y: crate::structures::elbphilharmonie::GROUND,
        ground_offset: 0,
        interior_marker: &[],
        ground_overlap: 0,
        // The warehouse and its parts, short of the ticket pavilion east of it.
        suppress_half_x: 57.5,
        suppress_half_z: 41.5,
        reach_m: 72,
        height: 100,
    },
    Landmark {
        name: "Hollywood Sign",
        qid: "Q180376",
        osm_ids: crate::structures::hollywood_sign::LETTER_WAYS,
        model: Source::OnTerrain(crate::structures::hollywood_sign::build),
        // The mapped sign node; the letters stand on their mapped baselines around it.
        lat: 34.134_099_1,
        lon: -118.321_652_0,
        anchor_x: crate::structures::hollywood_sign::ANCHOR_X as f64,
        anchor_z: crate::structures::hollywood_sign::ANCHOR_Z as f64,
        ground_y: crate::structures::hollywood_sign::GROUND,
        ground_offset: 0,
        interior_marker: &[],
        ground_overlap: 0,
        suppress_half_x: 56.0,
        suppress_half_z: 10.0,
        reach_m: 58,
        height: 14,
    },
    Landmark {
        name: "Stonehenge",
        qid: "Q39671",
        osm_ids: crate::structures::stonehenge::MAPPED,
        model: Source::OnTerrain(crate::structures::stonehenge::build),
        // Centre of the sarsen circle, the stone plan fitted to the mapped stones.
        lat: 51.178_860_4,
        lon: -1.826_203_6,
        anchor_x: crate::structures::stonehenge::CENTRE_X as f64,
        anchor_z: crate::structures::stonehenge::CENTRE_Z as f64,
        ground_y: crate::structures::stonehenge::GROUND,
        ground_offset: 0,
        interior_marker: &[],
        ground_overlap: 0,
        // The henge; nothing is built inside it.
        suppress_half_x: 62.0,
        suppress_half_z: 62.0,
        reach_m: 162,
        height: 8,
    },
    Landmark {
        name: "Colosseum",
        qid: "Q10285",
        osm_ids: &[("relation", 1_834_818)],
        model: Source::Built(crate::structures::colosseum::build),
        // Centre of the 188 by 156 m ellipse fitted to the mapped rings, arena and hypogeum.
        lat: 41.890_236_8,
        lon: 12.492_329_1,
        anchor_x: crate::structures::colosseum::CX,
        anchor_z: crate::structures::colosseum::CZ,
        // Arena level; the hypogeum goes down below it.
        ground_y: crate::structures::colosseum::GROUND,
        ground_offset: -1,
        // A tuff course under the cavea; the outer rings and the paving follow the terrain.
        interior_marker: &[crate::block_definitions::TUFF],
        ground_overlap: 0,
        // The ellipse and nothing past it.
        suppress_half_x: 92.0,
        suppress_half_z: 80.0,
        reach_m: 100,
        height: 49,
    },
    Landmark {
        name: "Cristo Redentor",
        qid: "Q79961",
        // The statue and terrace parts; the stairs down and station escalators stay with OSM.
        osm_ids: &[
            ("node", 4_919_986_733),
            ("relation", 16_147_269),
            ("way", 105_667_349),
            ("way", 1_447_517_217),
            ("way", 1_447_517_218),
            ("way", 1_447_517_219),
            ("way", 1_447_517_220),
            ("way", 1_021_319_646),
            ("way", 1_021_319_647),
            ("way", 1_021_319_651),
            ("way", 1_021_319_652),
            ("way", 1_021_319_653),
            ("way", 274_557_377),
            ("way", 274_562_791),
        ],
        model: Source::SchemOnTerrain(include_bytes!(
            "../assets/structures/landmarks/christ_the_redeemer.schem"
        )),
        // The mapped monument node; statue and pedestal face 80.9 degrees, as the mapped octagon.
        lat: -22.951_917_3,
        lon: -43.210_458_5,
        anchor_x: 21.5,
        anchor_z: 24.5,
        // Layer 0 is the footing, so every column stands on layer 1 as one part.
        ground_y: 1,
        // Puts the platform floor (layer 38) on the summit, which peaks.rs lifts to its mapped height.
        ground_offset: -38,
        interior_marker: &[],
        ground_overlap: 0,
        suppress_half_x: 20.0,
        suppress_half_z: 18.0,
        reach_m: 32,
        height: 76,
    },
    Landmark {
        name: "Taj Mahal",
        qid: "Q9141",
        // The complex way and relation; its 54 mapped parts fall inside the suppress box.
        osm_ids: &[("way", 375_257_537), ("relation", 11_089_173)],
        model: Source::Schem(include_bytes!(
            "../assets/structures/landmarks/taj_mahal.schem"
        )),
        // Mid-complex on its axis, 163 m south of the dome, so the box stays off the river.
        lat: 27.173_538_0,
        lon: 78.042_100_0,
        anchor_x: 154.5,
        anchor_z: 223.5,
        // Garden turf; the terrace is a metre above it, the river wall 7 layers below.
        ground_y: 7,
        ground_offset: -1,
        interior_marker: &[],
        ground_overlap: 0,
        // River wall to great gate, wall to wall; not the forecourt south of the gate.
        suppress_half_x: 155.0,
        suppress_half_z: 224.0,
        reach_m: 275,
        height: 76,
    },
    Landmark {
        name: "Parthenon",
        qid: "Q10288",
        // Temple, Acropolis relation and corner parts; the box spares the site sheds 8 m south.
        osm_ids: &[
            ("way", 910_010_406),
            ("relation", 4_423_249),
            ("way", 386_428_494),
            ("way", 386_428_497),
            ("way", 386_428_499),
            ("way", 386_428_502),
            ("way", 386_428_503),
            ("way", 386_428_506),
            ("way", 386_428_509),
            ("way", 386_428_510),
            ("way", 386_428_512),
            ("way", 386_428_519),
            ("way", 386_428_521),
            ("way", 386_428_524),
            ("way", 386_428_527),
            ("way", 386_428_530),
            ("way", 386_428_540),
            ("way", 386_428_542),
            ("way", 386_428_543),
            ("way", 386_428_547),
            ("way", 386_428_549),
            ("way", 386_428_552),
            ("way", 386_428_562),
            ("way", 386_428_564),
            ("way", 386_428_579),
            ("way", 386_428_580),
            ("way", 386_428_590),
            ("way", 386_428_591),
            ("way", 386_428_598),
            ("way", 386_428_600),
            ("way", 386_428_602),
            ("way", 386_428_632),
            ("way", 386_428_644),
            ("way", 386_428_651),
            ("way", 386_428_654),
            ("way", 386_428_655),
            ("way", 386_428_686),
            ("way", 386_428_693),
            ("way", 386_428_708),
            ("way", 386_428_711),
            ("way", 386_428_713),
            ("way", 386_428_720),
            ("way", 386_428_737),
            ("way", 386_428_741),
            ("way", 386_428_742),
        ],
        model: Source::Schem(include_bytes!(
            "../assets/structures/landmarks/parthenon.schem"
        )),
        // Middle of the colonnade; the 46 mapped columns centre within a centimetre of it.
        lat: 37.971_503_4,
        lon: 23.726_617_7,
        anchor_x: 41.0,
        anchor_z: 27.0,
        // Buried footing and euthynteria below; the stylobate stays level, so not on terrain.
        ground_y: 3,
        ground_offset: 0,
        interior_marker: &[],
        ground_overlap: 0,
        suppress_half_x: 37.5,
        suppress_half_z: 20.5,
        reach_m: 50,
        height: 18,
    },
    Landmark {
        name: "Arc de Triomphe",
        qid: "Q64436",
        osm_ids: &[("way", 226_413_508)],
        model: Source::Schem(include_bytes!(
            "../assets/structures/landmarks/arc_de_triomphe.schem"
        )),
        // Middle of the body from IGN LiDAR HD; the model is built at its 25.6 degree turn.
        lat: 48.873_778_8,
        lon: 2.295_038_7,
        anchor_x: 23.784,
        anchor_z: 29.451,
        // The paving under the vault is dug in.
        ground_y: 2,
        ground_offset: 0,
        interior_marker: &[],
        ground_overlap: 0,
        suppress_half_x: 26.0,
        suppress_half_z: 28.0,
        reach_m: 40,
        height: 50,
    },
    Landmark {
        name: "Mount Rushmore",
        qid: "Q83497",
        // Carving, summit, talus (else grass) and the four head nodes (else statues on the faces).
        osm_ids: &[
            ("way", 550_692_883),
            ("node", 357_100_300),
            ("way", 823_076_723),
            ("node", 11_872_642_649),
            ("node", 11_872_642_650),
            ("node", 11_872_642_651),
            ("node", 11_872_642_652),
        ],
        model: Source::Terrain(include_bytes!(
            "../assets/structures/landmarks/mount_rushmore.schem"
        )),
        // Middle of the footprint's bounding box, on a block corner; shaped from USGS 3DEP lidar.
        lat: 43.878_649_3,
        lon: -103.458_775_4,
        anchor_x: 68.0,
        anchor_z: 93.0,
        // The median ground over the box, 1650 m.
        ground_y: 41,
        ground_offset: 0,
        // Every column is dug out from its lowest block, so carved hollows replace the terrain.
        interior_marker: &[],
        ground_overlap: 0,
        suppress_half_x: 64.0,
        suppress_half_z: 89.0,
        reach_m: 116,
        height: 85,
    },
    Landmark {
        name: "Eiffel Tower",
        qid: "Q243",
        // The tower way, which also carries the 3DMR model, and its mapped pillars, floors and parts.
        osm_ids: &[
            ("way", 5_013_364),
            ("way", 69_034_127),
            ("way", 69_034_135),
            ("way", 69_034_179),
            ("way", 69_034_180),
            ("relation", 4_114_839),
            ("relation", 4_114_842),
            ("way", 308_021_389),
            ("way", 308_145_239),
            ("way", 308_145_258),
            ("way", 308_145_259),
            ("way", 308_687_744),
            ("way", 308_687_745),
            ("way", 308_687_748),
            ("way", 308_687_752),
            ("way", 308_687_753),
            ("way", 308_689_164),
            ("way", 308_689_165),
            ("way", 308_689_166),
            ("way", 308_689_167),
            ("way", 1_462_597_762),
            ("way", 1_462_597_763),
            ("way", 1_462_597_764),
            ("way", 1_462_597_765),
        ],
        model: Source::Schem(include_bytes!(
            "../assets/structures/landmarks/eiffel_tower.schem"
        )),
        // Tower axis from IGN LiDAR HD; the model is built at the real 44.3 degree turn.
        lat: 48.858_260_0,
        lon: 2.294_499_8,
        anchor_x: 92.0,
        anchor_z: 92.0,
        // Two layers of buried pier stones.
        ground_y: 2,
        ground_offset: 0,
        interior_marker: &[],
        ground_overlap: 0,
        // The pillars and the kiosks under them, not the buildings round the esplanade.
        suppress_half_x: 66.0,
        suppress_half_z: 66.0,
        reach_m: 90,
        height: 330,
    },
    Landmark {
        name: "Gateway Arch",
        qid: "Q2027162",
        // The node carries the QID; the building relation is tagged with the park's QID instead.
        osm_ids: &[("node", 4_004_558_403), ("relation", 12_813_920)],
        model: Source::Schem(include_bytes!(
            "../assets/structures/landmarks/gateway_arch.schem"
        )),
        // Between the legs at plaza level, from the design solid fitted to USGS 3DEP lidar.
        lat: 38.624_608_0,
        lon: -90.184_975_7,
        anchor_x: 37.5,
        anchor_z: 95.5,
        // Three layers of leg below the plaza.
        ground_y: 3,
        ground_offset: 0,
        interior_marker: &[],
        ground_overlap: 0,
        suppress_half_x: 37.0,
        suppress_half_z: 95.0,
        reach_m: 98,
        height: 192,
    },
    Landmark {
        name: "Space Needle",
        qid: "Q5317",
        osm_ids: &[("way", 12_903_132), ("relation", 19_745_997)],
        model: Source::Schem(include_bytes!(
            "../assets/structures/landmarks/space_needle.schem"
        )),
        // Tower axis from USGS 3DEP lidar; the mapped way sits a metre west of it.
        lat: 47.620_503_9,
        lon: -122.349_283_9,
        anchor_x: 25.5,
        anchor_z: 25.5,
        ground_y: 2,
        ground_offset: 0,
        interior_marker: &[],
        ground_overlap: 0,
        // The pavilion and halo, not the entry canopy 30 m south.
        suppress_half_x: 22.0,
        suppress_half_z: 22.0,
        reach_m: 23,
        height: 186,
    },
];

/// A landmark resolved to a spot in this world.
pub struct LandmarkPlacement {
    landmark: &'static Landmark,
    /// World XZ the model anchor is pinned to.
    world_x: i32,
    world_z: i32,
}

pub struct LandmarkPrescan {
    placements: Vec<LandmarkPlacement>,
    suppressed: HashSet<(&'static str, u64)>,
}

impl LandmarkPrescan {
    /// OSM elements a landmark replaces, also fed to the 3D-model prescan.
    pub fn suppressed(&self) -> &HashSet<(&'static str, u64)> {
        &self.suppressed
    }

    /// Regions each landmark may write to, so stream-to-disk keeps them resident.
    pub fn placement_regions(&self, scale: f64) -> Vec<Vec<(i32, i32)>> {
        self.placements
            .iter()
            .map(|p| {
                let r = (p.landmark.reach_m as f64 * scale).ceil() as i32;
                crate::models_3d::region_keys_around(p.world_x, p.world_z, r)
            })
            .collect()
    }
}

/// The landmarks this world may hold, each a tall model as high as it stands, so the build
/// height counts them even where their OSM element carries no height: those whose element is
/// in `data`, and those within reach of `bbox`, which `prescan` places whether or not it is.
pub fn tall_models_in(
    data: &crate::osm_parser::OsmData,
    bbox: Option<&LLBBox>,
) -> Vec<crate::elevation::TallModel> {
    let in_data = |landmark: &Landmark| {
        data.elements().iter().any(|e| {
            landmark.osm_ids.contains(&(e.r#type.as_str(), e.id))
                || e.tags
                    .as_ref()
                    .and_then(|t| t.get("wikidata"))
                    .is_some_and(|q| q.trim() == landmark.qid)
        })
    };
    let in_reach = |landmark: &Landmark| {
        bbox.is_some_and(|b| {
            let lat_m = 111_320.0;
            let lon_m = lat_m * landmark.lat.to_radians().cos().max(0.01);
            let reach = f64::from(landmark.reach_m);
            landmark.lat + reach / lat_m >= b.min().lat()
                && landmark.lat - reach / lat_m <= b.max().lat()
                && landmark.lon + reach / lon_m >= b.min().lng()
                && landmark.lon - reach / lon_m <= b.max().lng()
        })
    };
    LANDMARKS
        .iter()
        .filter(|l| in_data(l) || in_reach(l))
        .map(|l| crate::elevation::TallModel {
            lat: l.lat,
            lon: l.lon,
            height_m: f64::from(l.height),
        })
        .collect()
}

/// Whether a bundled landmark model stands for this Wikidata QID.
pub fn has_landmark(qid: &str) -> bool {
    LANDMARKS.iter().any(|l| l.qid == qid)
}

/// Whether this run places landmarks: `--props` (else `--no-3d`) allows them, and it builds
/// objects and buildings.
pub fn placed(args: &Args) -> bool {
    args.props().has(crate::structures::Prop::Landmark) && !args.skip_objects() && args.buildings
}

/// Resolve the landmarks inside this world and collect what they replace.
/// `relief` is the terrain's blocks per metre, `None` without elevation.
pub fn prescan(
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    llbbox: LLBBox,
    args: &Args,
    relief: Option<f64>,
) -> LandmarkPrescan {
    let mut placements: Vec<LandmarkPlacement> = Vec::new();
    let mut suppressed: HashSet<(&'static str, u64)> = HashSet::new();

    if !placed(args) {
        return LandmarkPrescan {
            placements,
            suppressed,
        };
    }

    for landmark in LANDMARKS {
        let Some((world_x, world_z)) = world_anchor(landmark.lat, landmark.lon, llbbox, args)
        else {
            continue;
        };
        // The model only matters if its reach overlaps the world.
        let reach = (landmark.reach_m as f64 * args.scale).ceil() as i32;
        if world_x + reach < xzbbox.min_x()
            || world_x - reach > xzbbox.max_x()
            || world_z + reach < xzbbox.min_z()
            || world_z - reach > xzbbox.max_z()
        {
            continue;
        }
        let true_relief = relief.is_some_and(|bpm| (bpm / args.scale - 1.0).abs() <= 0.02);
        if matches!(landmark.model, Source::Terrain(_)) && !true_relief {
            println!(
                "  Left out {}: it needs the terrain at its real height",
                landmark.name
            );
            continue;
        }

        let placement = LandmarkPlacement {
            landmark,
            world_x,
            world_z,
        };
        suppressed.extend(suppressed_by(&placement, elements, args));
        placements.push(placement);
    }

    LandmarkPrescan {
        placements,
        suppressed,
    }
}

/// Project an anchor into world XZ the way the parser projects node coords.
fn world_anchor(lat: f64, lon: f64, llbbox: LLBBox, args: &Args) -> Option<(i32, i32)> {
    let llpoint = LLPoint::new(lat, lon).ok()?;
    let (transformer, pre_rotation_bbox) = crate::projection::ProjectionSpec::from_args(args)
        .transformer(&llbbox)
        .ok()?;

    let xzpoint = transformer.transform_point(llpoint);
    Some(crate::map_transformation::rotate::rotate_xz_point(
        xzpoint.x,
        xzpoint.z,
        args.rotation,
        &pre_rotation_bbox,
    ))
}

/// The landmark's own elements plus what stands where the model goes.
fn suppressed_by(
    placement: &LandmarkPlacement,
    elements: &[ProcessedElement],
    args: &Args,
) -> HashSet<(&'static str, u64)> {
    let landmark = placement.landmark;
    let mut out: HashSet<(&'static str, u64)> = HashSet::new();

    for element in elements {
        let key = (element.kind(), element.id());
        let tags = element.tags();
        let is_landmark = tags.get("wikidata").map(|q| q.trim()) == Some(landmark.qid)
            || landmark.osm_ids.contains(&key);
        if is_landmark {
            out.insert(key);
            continue;
        }
        if !is_replaceable(element) {
            continue;
        }
        let Some((x, z)) = centroid_xz(element) else {
            continue;
        };
        let (mx, mz) = to_model_xz(placement, args, x as f64, z as f64);
        if (mx - landmark.anchor_x).abs() <= landmark.suppress_half_x
            && (mz - landmark.anchor_z).abs() <= landmark.suppress_half_z
        {
            out.insert(key);
        }
    }

    out
}

/// Features the model renders itself. Roads and greenery are left alone.
fn is_replaceable(element: &ProcessedElement) -> bool {
    let tags = element.tags();
    if tags.contains_key("building") || tags.contains_key("building:part") {
        return true;
    }
    matches!(
        tags.get("leisure").map(String::as_str),
        Some("stadium") | Some("pitch") | Some("track") | Some("sports_centre")
    )
}

/// `ProcessedElement::nodes` is empty for relations, so walk their members.
fn centroid_xz(element: &ProcessedElement) -> Option<(i32, i32)> {
    let (mut sx, mut sz, mut n) = (0i64, 0i64, 0i64);
    let mut add = |x: i32, z: i32| {
        sx += x as i64;
        sz += z as i64;
        n += 1;
    };
    match element {
        ProcessedElement::Node(node) => add(node.x, node.z),
        ProcessedElement::Way(way) => way.nodes.iter().for_each(|p| add(p.x, p.z)),
        ProcessedElement::Relation(rel) => rel
            .members
            .iter()
            .flat_map(|m| m.way.nodes.iter())
            .for_each(|p| add(p.x, p.z)),
    }
    (n > 0).then(|| ((sx / n) as i32, (sz / n) as i32))
}

/// Sine/cosine of the world rotation, matching `map_transformation::rotate`.
#[inline]
fn rotation_sin_cos(args: &Args) -> (f64, f64) {
    args.rotation.to_radians().sin_cos()
}

/// World XZ to model XZ: undo the rotation and the scale.
#[inline]
fn to_model_xz(placement: &LandmarkPlacement, args: &Args, wx: f64, wz: f64) -> (f64, f64) {
    let (sin_t, cos_t) = rotation_sin_cos(args);
    let dx = (wx - placement.world_x as f64) / args.scale;
    let dz = (wz - placement.world_z as f64) / args.scale;
    (
        placement.landmark.anchor_x + dx * cos_t + dz * sin_t,
        placement.landmark.anchor_z - dx * sin_t + dz * cos_t,
    )
}

/// Model XZ to world XZ, the inverse of `to_model_xz`.
#[inline]
fn to_world_xz(placement: &LandmarkPlacement, args: &Args, mx: f64, mz: f64) -> (f64, f64) {
    let (sin_t, cos_t) = rotation_sin_cos(args);
    let dx = (mx - placement.landmark.anchor_x) * args.scale;
    let dz = (mz - placement.landmark.anchor_z) * args.scale;
    (
        placement.world_x as f64 + dx * cos_t - dz * sin_t,
        placement.world_z as f64 + dx * sin_t + dz * cos_t,
    )
}

/// A landmark schematic laid out for column-wise stamping. Palette indices and a
/// dense column table cost about 3 MB at 350k voxels, against 11 MB expanded.
struct Model {
    palette: Vec<BlockWithProperties>,
    /// Voxels sorted by (x, z, y), so every column is one contiguous slice.
    voxels: Vec<(i16, i16, i16, u8)>,
    /// (offset, length) into `voxels`, indexed by `slot`.
    columns: Vec<(u32, u32)>,
    /// Columns dug out down to the model. Elsewhere the ground stays.
    excavated: Vec<bool>,
    /// For a model on terrain, each column's standing part, or `DRAPED`, indexed by `slot`,
    /// and how many parts there are.
    parts: Option<(Vec<u32>, usize)>,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    /// Highest occupied layer, so how far terrain must be cleared.
    max_y: i32,
}

impl Model {
    fn load(landmark: &Landmark) -> Result<Self, String> {
        let mut parsed = match landmark.model {
            Source::Schem(bytes) | Source::SchemOnTerrain(bytes) | Source::Terrain(bytes) => {
                load_palettized(bytes)?
            }
            Source::Built(build) | Source::OnTerrain(build) => build(),
        };
        join_connectables(&mut parsed);
        let mut voxels = parsed.voxels;
        if voxels.is_empty() {
            return Err("no mapped blocks".to_string());
        }
        voxels.sort_unstable_by_key(|v| (v.0, v.2, v.1));

        let min_x = i32::from(voxels.iter().map(|v| v.0).min().unwrap_or(0));
        let max_x = i32::from(voxels.iter().map(|v| v.0).max().unwrap_or(0));
        let min_z = i32::from(voxels.iter().map(|v| v.2).min().unwrap_or(0));
        let max_z = i32::from(voxels.iter().map(|v| v.2).max().unwrap_or(0));
        let max_y = i32::from(voxels.iter().map(|v| v.1).max().unwrap_or(0));

        let span_x = (max_x - min_x + 1) as usize;
        let span_z = (max_z - min_z + 1) as usize;
        let mut columns = vec![(0u32, 0u32); span_x * span_z];
        let mut interior = vec![false; span_x * span_z];
        let is_marker: Vec<bool> = parsed
            .palette
            .iter()
            .map(|b| landmark.interior_marker.contains(&b.block))
            .collect();

        let mut start = 0usize;
        while start < voxels.len() {
            let (x, _, z, _) = voxels[start];
            let mut end = start + 1;
            while end < voxels.len() && voxels[end].0 == x && voxels[end].2 == z {
                end += 1;
            }
            let slot = (i32::from(x) - min_x) as usize + (i32::from(z) - min_z) as usize * span_x;
            columns[slot] = (start as u32, (end - start) as u32);
            interior[slot] = voxels[start..end].iter().any(|v| is_marker[v.3 as usize]);
            start = end;
        }

        let excavated = if landmark.interior_marker.is_empty() {
            vec![true; span_x * span_z]
        } else {
            let filled = fill_enclosed(&interior, span_x, span_z);
            erode(
                &filled,
                span_x,
                span_z,
                landmark.ground_overlap.max(0) as usize,
            )
        };

        let parts = matches!(
            landmark.model,
            Source::OnTerrain(_) | Source::SchemOnTerrain(_)
        )
        .then(|| {
            let standing: Vec<bool> = columns
                .iter()
                .map(|&(offset, len)| {
                    len > 0
                        && i32::from(voxels[(offset + len - 1) as usize].1) > landmark.ground_y + 1
                })
                .collect();
            standing_parts(&standing, span_x, span_z)
        });

        Ok(Model {
            palette: parsed.palette,
            voxels,
            columns,
            excavated,
            parts,
            min_x,
            max_x,
            min_z,
            max_z,
            max_y,
        })
    }

    #[inline]
    fn slot(&self, x: i32, z: i32) -> Option<usize> {
        if x < self.min_x || x > self.max_x || z < self.min_z || z > self.max_z {
            return None;
        }
        let span_x = (self.max_x - self.min_x + 1) as usize;
        Some((x - self.min_x) as usize + (z - self.min_z) as usize * span_x)
    }

    #[inline]
    fn column(&self, x: i32, z: i32) -> Option<&[(i16, i16, i16, u8)]> {
        let (offset, len) = self.columns[self.slot(x, z)?];
        (len > 0).then(|| &self.voxels[offset as usize..(offset + len) as usize])
    }

    /// True where the ground is cut away rather than closing over the model.
    #[inline]
    fn excavated(&self, x: i32, z: i32) -> bool {
        self.slot(x, z).is_some_and(|s| self.excavated[s])
    }
}

/// `mask` plus what it encloses, by flooding the outside in from the border.
/// A tribune ring becomes the whole bowl, so the pitch counts as interior.
pub(crate) fn fill_enclosed(mask: &[bool], span_x: usize, span_z: usize) -> Vec<bool> {
    let mut outside = vec![false; mask.len()];
    let mut stack: Vec<usize> = Vec::new();
    let push = |i: usize, outside: &mut Vec<bool>, stack: &mut Vec<usize>| {
        if !mask[i] && !outside[i] {
            outside[i] = true;
            stack.push(i);
        }
    };
    for x in 0..span_x {
        push(x, &mut outside, &mut stack);
        push((span_z - 1) * span_x + x, &mut outside, &mut stack);
    }
    for z in 0..span_z {
        push(z * span_x, &mut outside, &mut stack);
        push(z * span_x + span_x - 1, &mut outside, &mut stack);
    }
    while let Some(i) = stack.pop() {
        let (x, z) = (i % span_x, i / span_x);
        if x > 0 {
            push(i - 1, &mut outside, &mut stack);
        }
        if x + 1 < span_x {
            push(i + 1, &mut outside, &mut stack);
        }
        if z > 0 {
            push(i - span_x, &mut outside, &mut stack);
        }
        if z + 1 < span_z {
            push(i + span_x, &mut outside, &mut stack);
        }
    }
    outside.iter().map(|&o| !o).collect()
}

/// Part of a column that follows the terrain on its own.
const DRAPED: u32 = u32::MAX;

/// Number the 8-connected groups of standing columns, the rest draping, and count the groups.
fn standing_parts(standing: &[bool], span_x: usize, span_z: usize) -> (Vec<u32>, usize) {
    let mut parts = vec![DRAPED; standing.len()];
    let mut next = 0u32;
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..standing.len() {
        if !standing[start] || parts[start] != DRAPED {
            continue;
        }
        parts[start] = next;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, z) = ((i % span_x) as i64, (i / span_x) as i64);
            for (dx, dz) in (-1..=1).flat_map(|dz| (-1..=1).map(move |dx| (dx, dz))) {
                let (nx, nz) = (x + dx, z + dz);
                if nx < 0 || nz < 0 || nx >= span_x as i64 || nz >= span_z as i64 {
                    continue;
                }
                let n = nx as usize + nz as usize * span_x;
                if standing[n] && parts[n] == DRAPED {
                    parts[n] = next;
                    stack.push(n);
                }
            }
        }
        next += 1;
    }
    (parts, next as usize)
}

/// Shrink `mask` by `r` blocks, as two linear passes rather than a square.
fn erode(mask: &[bool], span_x: usize, span_z: usize, r: usize) -> Vec<bool> {
    if r == 0 {
        return mask.to_vec();
    }
    let run = |src: &[bool], along_x: bool| -> Vec<bool> {
        let (outer, inner, step) = if along_x {
            (span_z, span_x, 1usize)
        } else {
            (span_x, span_z, span_x)
        };
        let mut out = vec![false; src.len()];
        for o in 0..outer {
            let base = if along_x { o * span_x } else { o };
            for i in 0..inner {
                if i < r || i + r >= inner {
                    continue;
                }
                out[base + i * step] = (i - r..=i + r).all(|k| src[base + k * step]);
            }
        }
        out
    };
    run(&run(mask, true), false)
}

impl LandmarkPrescan {
    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }

    /// Stamp `indices` (ascending), after ground generation so terrain Y is final.
    pub fn place(&self, editor: &mut WorldEditor, args: &Args, indices: &[usize]) {
        if indices.is_empty() || !editor.place_prop(crate::structures::Prop::Landmark) {
            return;
        }
        for &i in indices {
            place_one(editor, args, &self.placements[i]);
        }
    }
}

fn place_one(editor: &mut WorldEditor, args: &Args, placement: &LandmarkPlacement) {
    let landmark = placement.landmark;
    let model = match Model::load(landmark) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "{} {} model failed to load: {e}",
                "Warning:".yellow().bold(),
                landmark.name
            );
            return;
        }
    };

    let base_y = base_ground_y(editor, args, placement);
    // Block states rotate in quarter turns only. Doing it once per palette
    // entry keeps it off the per-voxel path.
    let quarter = (args.rotation / 90.0).round().rem_euclid(4.0) as u8;
    let palette: Vec<BlockWithProperties> = if quarter == 0 {
        model.palette.clone()
    } else {
        model
            .palette
            .iter()
            .map(|b| {
                let props = b
                    .properties
                    .as_ref()
                    .map(|p| crate::block_definitions::intern_props(rotate_props(p, quarter)));
                BlockWithProperties::from_arc(b.block, props)
            })
            .collect()
    };
    // A model layer covers `scale` world layers, so nothing gaps at scale > 1.
    let layer = |dy: i32| base_y + (dy as f64 * args.scale).round() as i32;
    // Columns reaching the ground layer carry load; roof spans do not.
    let bury_below = landmark.ground_y + 2;

    // World AABB of the model footprint: transform its four corners.
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for (cx, cz) in [
        (model.min_x, model.min_z),
        (model.max_x + 1, model.min_z),
        (model.min_x, model.max_z + 1),
        (model.max_x + 1, model.max_z + 1),
    ] {
        let (wx, wz) = to_world_xz(placement, args, cx as f64, cz as f64);
        min_x = min_x.min(wx.floor() as i32);
        max_x = max_x.max(wx.ceil() as i32);
        min_z = min_z.min(wz.floor() as i32);
        max_z = max_z.max(wz.ceil() as i32);
    }

    let (world_min_x, world_min_z) = editor.get_min_coords();
    let (world_max_x, world_max_z) = editor.get_max_coords();
    min_x = min_x.max(world_min_x);
    max_x = max_x.min(world_max_x);
    min_z = min_z.max(world_min_z);
    max_z = max_z.min(world_max_z);

    if let Some((parts, part_count)) = &model.parts {
        let bounds = (min_x, max_x, min_z, max_z);
        let parts = (parts.as_slice(), *part_count);
        stand_on_terrain(editor, args, placement, &model, parts, &palette, bounds);
        return;
    }

    let clear_top = layer(model.max_y - landmark.ground_y + 1);
    // Writes past the world floor get clamped onto it rather than dropped, so a
    // dug-in model would smear its buried courses across bedrock. Skip them.
    let world_floor = crate::world_editor::min_y() + 1;
    let mut placed = 0usize;

    // Sampling the destination grid keeps it hole-free at any rotation or scale.
    for wz in min_z..=max_z {
        for wx in min_x..=max_x {
            let (mx, mz) = to_model_xz(placement, args, wx as f64 + 0.5, wz as f64 + 0.5);
            let (model_x, model_z) = (mx.floor() as i32, mz.floor() as i32);
            let Some(column) = model.column(model_x, model_z) else {
                continue;
            };

            let terrain = editor.get_ground_level(wx, wz);
            let column_low = i32::from(column[0].1);
            let column_bottom = layer(column_low - landmark.ground_y);
            let column_top = layer(i32::from(column[column.len() - 1].1) + 1 - landmark.ground_y);

            // Inside the interior this is a hole, so cut down to the model.
            // Outside, cut only what stands where the model breaks through.
            // A span overhead keeps what stands on the ground under it.
            let span = column_low > bury_below;
            let clear_from = if model.excavated(model_x, model_z) {
                if span {
                    column_bottom
                } else {
                    column_bottom.min(terrain + 1)
                }
            } else if column_top > terrain {
                if span {
                    column_bottom.max(terrain + 1)
                } else {
                    terrain + 1
                }
            } else {
                clear_top
            };
            for y in clear_from.max(world_floor)..clear_top {
                if editor.get_block_absolute(wx, y, wz).is_some() {
                    editor.set_block_absolute(AIR, wx, y, wz, None, Some(&[]));
                }
            }
            // Bridge any gap under a load-bearing column so nothing floats.
            // Starting under its lowest block keeps a below-grade section hollow.
            if column_low <= bury_below {
                let floor = (column_bottom - MAX_FOUNDATION_DEPTH).max(world_floor);
                let mut y = column_bottom - 1;
                while y >= floor && editor.get_block_absolute(wx, y, wz).is_none() {
                    editor.set_block_absolute(STONE, wx, y, wz, None, Some(&[]));
                    y -= 1;
                }
            }

            for &(_, my, _, slot) in column {
                let y0 = layer(i32::from(my) - landmark.ground_y);
                let y1 = layer(i32::from(my) + 1 - landmark.ground_y).max(y0 + 1);
                for y in y0.max(world_floor)..y1 {
                    editor.set_block_with_properties_absolute(
                        palette[slot as usize].clone(),
                        wx,
                        y,
                        wz,
                        None,
                        Some(&[]),
                    );
                    placed += 1;
                }
            }
        }
    }

    if placed > 0 {
        println!(
            "  Placed {} ({} blocks)",
            landmark.name.bright_white().bold(),
            placed
        );
    }
}

/// The ground offset in blocks, scaled like the model's layers.
fn scaled_offset(landmark: &Landmark, args: &Args) -> i32 {
    (f64::from(landmark.ground_offset) * args.scale).round() as i32
}

/// Joins walls, fences, bars and panes inside the model as a block update would in game.
fn join_connectables(model: &mut PalettizedSchematic) {
    use crate::element_processing::connected_blocks::{is_connectable, joined, joins};
    const SIDES: [(i16, i16); 4] = [(0, -1), (0, 1), (1, 0), (-1, 0)];
    let cells: HashMap<(i16, i16, i16), u8> = model
        .voxels
        .iter()
        .map(|v| ((v.0, v.1, v.2), v.3))
        .collect();
    let mut slots: HashMap<(u8, [bool; 4]), u8> = HashMap::new();
    let mut palette = model.palette.clone();
    for v in &mut model.voxels {
        let authored = &model.palette[v.3 as usize];
        let block = authored.block;
        if !is_connectable(block) {
            continue;
        }
        let sides = SIDES.map(|(dx, dz)| {
            cells.get(&(v.0 + dx, v.1, v.2 + dz)).is_some_and(|&s| {
                let n = model.palette[s as usize].block;
                joins(block, n) || sturdy(n)
            })
        });
        let slot = *slots.entry((v.3, sides)).or_insert_with(|| {
            if palette.len() >= 256 {
                return v.3;
            }
            let mut fresh = joined(block, sides);
            keep_authored(&mut fresh, authored);
            palette.push(fresh);
            (palette.len() - 1) as u8
        });
        v.3 = slot;
    }
    model.palette = palette;
}

/// Carries over what joining can't tell: tall wall sides, a raised post, water.
fn keep_authored(fresh: &mut BlockWithProperties, authored: &BlockWithProperties) {
    let is = |props: &HashMap<String, Value>, key: &str, want: &str| matches!(props.get(key), Some(Value::String(v)) if v == want);
    let (Some(Value::Compound(old)), Some(new)) =
        (authored.properties.as_deref(), fresh.properties.as_mut())
    else {
        return;
    };
    let Value::Compound(new) = Arc::make_mut(new) else {
        return;
    };
    for side in ["north", "south", "east", "west"] {
        if is(old, side, "tall") && is(new, side, "low") {
            new.insert(side.to_string(), Value::String("tall".to_string()));
        }
    }
    for key in ["up", "waterlogged"] {
        if is(old, key, "true") && new.contains_key(key) {
            new.insert(key.to_string(), Value::String("true".to_string()));
        }
    }
}

/// A block with full sides that walls, fences and panes join, read from its name.
fn sturdy(block: Block) -> bool {
    const THIN: &[&str] = &[
        "air",
        "light",
        "water",
        "lava",
        "fire",
        "soul_fire",
        "ladder",
        "rail",
        "snow",
        "scaffolding",
        "anvil",
        "hopper",
        "grindstone",
        "lectern",
        "chain",
        "iron_chain",
        "cobweb",
        "vine",
        "cauldron",
        "chest",
        "bell",
        "short_grass",
        "tall_grass",
        "fern",
        "large_fern",
        "dead_bush",
        "lantern",
        "soul_lantern",
        "torch",
        "end_rod",
        "lightning_rod",
        "flower_pot",
        "cactus",
        "bamboo",
        "sugar_cane",
        "lily_pad",
        "kelp",
        "seagrass",
        "poppy",
        "dandelion",
        "allium",
        "cornflower",
        "sunflower",
        "lilac",
        "peony",
        "lily_of_the_valley",
        "barrier",
        "carved_pumpkin",
        "jack_o_lantern",
        "melon",
        "pumpkin",
    ];
    const THIN_KINDS: &[&str] = &[
        "slab", "stairs", "wall", "fence", "gate", "pane", "bars", "trapdoor", "door", "carpet",
        "button", "plate", "torch", "banner", "sign", "leaves", "head", "skull", "bed", "candle",
        "pickle", "sapling", "tulip", "orchid", "bluet", "daisy", "bush", "flower", "box",
    ];
    let name = block.name();
    let kind = name.rsplit('_').next().unwrap_or(name);
    !(THIN.contains(&name) || THIN_KINDS.contains(&kind) || name.starts_with("potted_"))
}

/// Places a model on terrain: draped columns on their own ground, each part on the highest
/// ground under it with its footings carried down. Nothing is dug out, and the footprint is
/// cleared to the model's full height as for other landmarks.
fn stand_on_terrain(
    editor: &mut WorldEditor,
    args: &Args,
    placement: &LandmarkPlacement,
    model: &Model,
    (parts, part_count): (&[u32], usize),
    palette: &[BlockWithProperties],
    (min_x, max_x, min_z, max_z): (i32, i32, i32, i32),
) {
    let landmark = placement.landmark;
    let columns_at = |wx: i32, wz: i32| covering_columns(model, parts, placement, args, wx, wz);

    // A part rests on its columns that meet the ground layer; one without any, on all of them.
    let mut part_ground = vec![(i32::MIN, i32::MIN); part_count];
    for wz in min_z..=max_z {
        for wx in min_x..=max_x {
            if let Some((columns, part)) = columns_at(wx, wz) {
                let column = columns[columns.len() - 1];
                if part != DRAPED {
                    let terrain = editor.get_ground_level(wx, wz);
                    let (bearing, any) = &mut part_ground[part as usize];
                    *any = (*any).max(terrain);
                    if column.iter().any(|v| i32::from(v.1) == landmark.ground_y) {
                        *bearing = (*bearing).max(terrain);
                    }
                }
            }
        }
    }

    let height = model.max_y - landmark.ground_y + 1;
    let world_floor = crate::world_editor::min_y() + 1;
    let mut placed = 0usize;
    for wz in min_z..=max_z {
        for wx in min_x..=max_x {
            let Some((columns, part)) = columns_at(wx, wz) else {
                continue;
            };
            let column = columns[columns.len() - 1];
            let terrain = editor.get_ground_level(wx, wz);
            let ground = match part {
                DRAPED => terrain,
                _ => match part_ground[part as usize] {
                    (i32::MIN, any) => any,
                    (bearing, _) => bearing,
                },
            };
            let base = ground + 1 + scaled_offset(landmark, args);
            let (low, layers) = column_layers(&columns, landmark.ground_y, args.scale, palette);
            let bottom = base + low;
            let top = bottom + layers.len() as i32;
            let clear_top = top.max(base + (f64::from(height) * args.scale).round() as i32);
            for y in (terrain + 1).max(world_floor)..=clear_top {
                if editor.get_block_absolute(wx, y, wz).is_some() {
                    editor.set_block_absolute(AIR, wx, y, wz, None, Some(&[]));
                }
            }
            // A footing below the ground layer goes on down to the terrain under it.
            let footing = (part != DRAPED && i32::from(column[0].1) < landmark.ground_y)
                .then_some(column[0].3);
            let below = footing.map_or(0, |_| (bottom - terrain - 1).max(0));
            let layers = std::iter::repeat_n(footing, below as usize).chain(layers);
            for (y, slot) in (bottom - below..).zip(layers) {
                let Some(slot) = slot.filter(|_| y >= world_floor) else {
                    continue;
                };
                editor.set_block_with_properties_absolute(
                    palette[slot as usize].clone(),
                    wx,
                    y,
                    wz,
                    None,
                    Some(&[]),
                );
                placed += 1;
            }
        }
    }

    if placed > 0 {
        println!(
            "  Placed {} ({} blocks)",
            landmark.name.bright_white().bold(),
            placed
        );
    }
}

/// A model column: its voxels from the lowest up.
type Column<'m> = &'m [(i16, i16, i16, u8)];

/// The model columns a world column shows, the one setting its part last, and that part. Below
/// full scale a world column spans all those whose middle falls in it, and the tallest, then
/// fullest, sets the part.
fn covering_columns<'m>(
    model: &'m Model,
    parts: &[u32],
    placement: &LandmarkPlacement,
    args: &Args,
    wx: i32,
    wz: i32,
) -> Option<(Vec<Column<'m>>, u32)> {
    let at = |x: i32, z: i32| Some((model.column(x, z)?, parts[model.slot(x, z)?]));
    let (mx, mz) = to_model_xz(placement, args, wx as f64 + 0.5, wz as f64 + 0.5);
    let centre = || at(mx.floor() as i32, mz.floor() as i32).map(|(c, p)| (vec![c], p));
    if args.scale >= 1.0 {
        return centre();
    }
    let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
        .map(|(dx, dz)| to_model_xz(placement, args, wx as f64 + dx, wz as f64 + dz));
    let span = |pick: fn(&(f64, f64)) -> f64| {
        let values = corners.iter().map(pick);
        let low = values.clone().fold(f64::INFINITY, f64::min).floor() as i32;
        (low, values.fold(f64::NEG_INFINITY, f64::max).ceil() as i32)
    };
    let ((x0, x1), (z0, z1)) = (span(|c| c.0), span(|c| c.1));
    let mut found = false;
    let mut columns: Vec<(Column<'m>, u32)> = Vec::new();
    for z in z0..=z1 {
        for x in x0..=x1 {
            let (cx, cz) = to_world_xz(placement, args, x as f64 + 0.5, z as f64 + 0.5);
            if (cx.floor() as i32, cz.floor() as i32) != (wx, wz) {
                continue;
            }
            found = true;
            columns.extend(at(x, z));
        }
    }
    if !found {
        return centre();
    }
    let rank = |c: Column| (c[c.len() - 1].1, c.len());
    columns.sort_by_key(|&(c, _)| rank(c));
    let part = columns.last()?.1;
    Some((columns.into_iter().map(|(c, _)| c).collect(), part))
}

/// Columns laid out in world layers from their lowest, offset from the ground layer, as for
/// other landmarks: each model layer covers at least one world layer. Where several share one,
/// a plain block beats one with states, that a plant, that air; then the higher layer, then the
/// later column.
fn column_layers(
    columns: &[Column],
    ground_y: i32,
    scale: f64,
    palette: &[BlockWithProperties],
) -> (i32, Vec<Option<u8>>) {
    let offset = |dy: i32| (f64::from(dy) * scale).round() as i32;
    let low = columns
        .iter()
        .map(|c| offset(i32::from(c[0].1) - ground_y))
        .min()
        .unwrap_or(0);
    let mut layers: Vec<Option<(u8, u8)>> = Vec::new();
    for &(_, my, _, slot) in columns.iter().flat_map(|c| c.iter()) {
        let dy = i32::from(my) - ground_y;
        let start = offset(dy);
        let end = offset(dy + 1).max(start + 1);
        let block = &palette[slot as usize];
        let class = if block.block == AIR {
            0
        } else if crate::ground_decoration::LOOSE_PLANTS.contains(&block.block) {
            1
        } else if block.properties.is_some() {
            2
        } else {
            3
        };
        for y in start..end {
            let i = (y - low) as usize;
            if layers.len() <= i {
                layers.resize(i + 1, None);
            }
            if layers[i].is_none_or(|(_, kept)| class >= kept) {
                layers[i] = Some((slot, class));
            }
        }
    }
    let layers = layers.into_iter().map(|l| l.map(|(slot, _)| slot));
    (low, layers.collect())
}

/// Deepest foundation built when terrain falls away below a landmark.
const MAX_FOUNDATION_DEPTH: i32 = 64;

/// Median terrain over the replaced area plus the offset, so a slope cannot drag
/// the build. Only the ground layer is held above bedrock; buried courses drop.
fn base_ground_y(editor: &WorldEditor, args: &Args, placement: &LandmarkPlacement) -> i32 {
    const SAMPLES: i32 = 16;
    let landmark = placement.landmark;
    let mut levels: Vec<i32> = Vec::with_capacity(((SAMPLES + 1) * (SAMPLES + 1)) as usize);
    for i in 0..=SAMPLES {
        for j in 0..=SAMPLES {
            let mx = landmark.anchor_x
                + landmark.suppress_half_x * (2.0 * i as f64 / SAMPLES as f64 - 1.0);
            let mz = landmark.anchor_z
                + landmark.suppress_half_z * (2.0 * j as f64 / SAMPLES as f64 - 1.0);
            let (wx, wz) = to_world_xz(placement, args, mx, mz);
            levels.push(editor.get_ground_level(wx.round() as i32, wz.round() as i32));
        }
    }
    levels.sort_unstable();
    let base = levels[levels.len() / 2] + 1 + scaled_offset(landmark, args);
    // One course above the lowest writable layer so the ground layer exists.
    base.max(crate::world_editor::min_y() + 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Looked up by identity so adding a landmark cannot repoint a test.
    fn landmark(qid: &str) -> &'static Landmark {
        LANDMARKS
            .iter()
            .find(|l| l.qid == qid)
            .unwrap_or_else(|| panic!("no landmark {qid}"))
    }

    const STADIUM: &str = "Q131610";
    const TOWER: &str = "Q599148";
    const SWIM_HALL: &str = "Q3882013";
    const EVENT_HALL: &str = "Q48849";

    #[test]
    fn a_landmark_in_the_data_counts_as_tall_as_its_model() {
        let data: crate::osm_parser::OsmData = serde_json::from_str(
            r#"{"elements": [
                {"type": "node", "id": 1, "lat": 48.17, "lon": 11.55},
                {"type": "way", "id": 164084344, "nodes": [1], "tags": {"building": "tower"}},
                {"type": "node", "id": 2, "lat": 51.5, "lon": -0.12, "tags": {"wikidata": "Q41225"}}
            ]}"#,
        )
        .unwrap();
        let found = tall_models_in(&data, None);
        assert_eq!(found.len(), 2, "{found:?}");
        let tower = landmark(TOWER);
        let olympiaturm = found.iter().find(|m| m.lat == tower.lat).unwrap();
        // The 291 m tower, less the empty layers at the top of its schematic.
        assert!(olympiaturm.height_m > 280.0, "{olympiaturm:?}");
        let empty: crate::osm_parser::OsmData =
            serde_json::from_str(r#"{"elements": []}"#).unwrap();
        assert!(tall_models_in(&empty, None).is_empty());
        // Placed from its anchor even when the bbox stops short of its OSM outline.
        let near = LLBBox::new(48.1740, 11.5530, 48.1743, 11.5536).unwrap();
        let reached = tall_models_in(&empty, Some(&near));
        assert!(reached.iter().any(|m| m.lat == tower.lat), "{reached:?}");
        let far = LLBBox::new(48.0, 11.0, 48.01, 11.01).unwrap();
        assert!(tall_models_in(&empty, Some(&far)).is_empty());
    }

    // Below full scale a model layer still shows: a lintel five up keeps a world layer over its
    // upright, grass in a ditch gives way to the turf under it, and a stone beside a tuft wins.
    #[test]
    fn half_scale_columns_keep_their_layers() {
        use crate::block_definitions::{ANDESITE, GRASS, GRASS_BLOCK};
        let palette = [STONE, ANDESITE, GRASS_BLOCK, GRASS].map(BlockWithProperties::simple);
        let trilithon: Vec<(i16, i16, i16, u8)> = (8..=14)
            .map(|y| (0, y, 0, 0))
            .chain([(0, 15, 0, 1)])
            .collect();
        let (low, layers) = column_layers(&[&trilithon], 10, 0.5, &palette);
        assert_eq!(low, -1);
        assert_eq!(layers.last(), Some(&Some(1)));
        assert!(layers.iter().all(Option::is_some));
        let ditch = [(0, 8, 0, 2), (0, 9, 0, 3)];
        assert_eq!(
            column_layers(&[&ditch], 10, 0.5, &palette),
            (-1, vec![Some(2)])
        );
        let turf = [(1, 9, 0, 2), (1, 10, 0, 3)];
        let stone = [(0, 9, 0, 0), (0, 10, 0, 0)];
        let merged = column_layers(&[&turf, &stone], 10, 0.5, &palette);
        assert_eq!(merged, (-1, vec![Some(0), Some(0)]));
        let (low, layers) = column_layers(&[&trilithon], 10, 1.0, &palette);
        assert_eq!((low, layers.len()), (-2, trilithon.len()));
    }

    #[test]
    fn every_landmark_height_is_its_models() {
        for l in LANDMARKS {
            let model = match l.model {
                Source::Schem(bytes) | Source::SchemOnTerrain(bytes) | Source::Terrain(bytes) => {
                    load_palettized(bytes).unwrap()
                }
                Source::Built(build) | Source::OnTerrain(build) => build(),
            };
            let top = model.voxels.iter().map(|v| i32::from(v.1)).max().unwrap() + 1;
            assert_eq!(
                l.height,
                top - l.ground_y + l.ground_offset.max(0),
                "{}",
                l.name
            );
        }
    }

    #[test]
    fn every_landmark_asset_parses() {
        for landmark in LANDMARKS {
            let model = Model::load(landmark)
                .unwrap_or_else(|e| panic!("{} failed to load: {e}", landmark.name));
            assert!(!model.voxels.is_empty(), "{} has no blocks", landmark.name);
            // The anchor and ground layer must be inside the model, or the
            // placement maths would pin it to empty space.
            assert!(
                (model.min_x as f64) <= landmark.anchor_x
                    && landmark.anchor_x <= (model.max_x + 1) as f64,
                "{} anchor_x outside the model",
                landmark.name
            );
            assert!(
                (model.min_z as f64) <= landmark.anchor_z
                    && landmark.anchor_z <= (model.max_z + 1) as f64,
                "{} anchor_z outside the model",
                landmark.name
            );
            assert!(
                landmark.ground_y >= 0 && landmark.ground_y <= model.max_y,
                "{} ground_y outside the model",
                landmark.name
            );
        }
    }

    #[test]
    fn landmark_identities_are_unique() {
        let mut qids: Vec<&str> = LANDMARKS.iter().map(|l| l.qid).collect();
        qids.sort_unstable();
        let count = qids.len();
        qids.dedup();
        assert_eq!(qids.len(), count, "duplicate landmark QID");
    }

    #[test]
    fn model_columns_cover_every_voxel() {
        let model = Model::load(landmark(TOWER)).expect("tower parses");
        let mut seen = 0usize;
        for &(x, _, z, _) in &model.voxels {
            seen += model
                .column(i32::from(x), i32::from(z))
                .map(<[_]>::len)
                .unwrap_or(0);
        }
        // Every voxel is reachable through its own column exactly len times.
        assert!(seen >= model.voxels.len());
        for &(x, _, z, _) in &model.voxels {
            let column = model
                .column(i32::from(x), i32::from(z))
                .expect("column exists");
            assert!(column.windows(2).all(|w| w[0].1 <= w[1].1), "column sorted");
            assert!(
                column.iter().all(|v| (v.3 as usize) < model.palette.len()),
                "palette index in range"
            );
        }
    }

    // Pin the storage form: palette indices and a dense column table.
    #[test]
    fn landmark_models_stay_compact() {
        assert_eq!(std::mem::size_of::<(i16, i16, i16, u8)>(), 8);
        for landmark in LANDMARKS {
            let model = Model::load(landmark).expect("parses");
            let bytes = model.voxels.len() * std::mem::size_of::<(i16, i16, i16, u8)>()
                + model.columns.len() * std::mem::size_of::<(u32, u32)>();
            assert!(
                bytes < 8 * 1024 * 1024,
                "{} model needs {bytes} bytes resident",
                landmark.name
            );
        }
    }

    // The marker ring must resolve to the bowl: the pitch it encloses is dug
    // out, the concourse and pylon feet outside it stay under the park.
    #[test]
    fn stadium_excavates_its_bowl_and_nothing_else() {
        let landmark = landmark(STADIUM);
        let model = Model::load(landmark).expect("stadium parses");
        let to_world = |ax: f64, az: f64| (ax.round() as i32, az.round() as i32);

        let (px, pz) = to_world(landmark.anchor_x, landmark.anchor_z);
        assert!(model.excavated(px, pz), "pitch centre must be dug out");

        // The model's outermost filled columns are concourse and roof feet.
        for (x, z) in [
            (model.min_x, (model.min_z + model.max_z) / 2),
            (model.max_x, (model.min_z + model.max_z) / 2),
            ((model.min_x + model.max_x) / 2, model.min_z),
            ((model.min_x + model.max_x) / 2, model.max_z),
        ] {
            assert!(!model.excavated(x, z), "edge ({x}, {z}) must stay buried");
        }

        // The overlap must actually pull the boundary inside the marker ring.
        let markers: HashSet<(i16, i16)> = model
            .voxels
            .iter()
            .filter(|v| {
                landmark
                    .interior_marker
                    .contains(&model.palette[v.3 as usize].block)
            })
            .map(|v| (v.0, v.2))
            .collect();
        assert!(
            markers
                .iter()
                .any(|&(x, z)| !model.excavated(i32::from(x), i32::from(z))),
            "ground_overlap must reach in under the tribunes"
        );
    }

    #[test]
    fn joins_read_whole_block_names_and_keep_tall_sides() {
        use crate::block_definitions::{LANTERN, LIGHT, SEA_LANTERN, STONE_BRICK_WALL, WATER};
        assert!(sturdy(LIGHT_GRAY_CONCRETE) && sturdy(SEA_LANTERN) && sturdy(STONE));
        assert!(!sturdy(LIGHT) && !sturdy(LANTERN) && !sturdy(WATER) && !sturdy(AIR));

        let tall = BlockWithProperties::new(
            STONE_BRICK_WALL,
            Some(Value::Compound(HashMap::from([
                ("east".to_string(), Value::String("tall".to_string())),
                ("up".to_string(), Value::String("true".to_string())),
            ]))),
        );
        let mut model = PalettizedSchematic {
            palette: vec![tall, BlockWithProperties::simple(STONE)],
            voxels: vec![(0, 0, 0, 0), (1, 0, 0, 1)],
            width: 2,
            length: 1,
        };
        join_connectables(&mut model);
        let props = model.palette[model.voxels[0].3 as usize].properties.clone();
        let Some(Value::Compound(props)) = props.as_deref().cloned() else {
            panic!("no properties");
        };
        let get = |k: &str| props.get(k).cloned();
        assert_eq!(get("east"), Some(Value::String("tall".to_string())));
        assert_eq!(get("west"), Some(Value::String("none".to_string())));
        assert_eq!(get("up"), Some(Value::String("true".to_string())));
    }

    // The swim hall's glazed façade is a closed ring, so hole-filling it must
    // yield the building interior while the roof cables outside stay buried.
    #[test]
    fn swim_hall_excavates_only_inside_its_facade() {
        let landmark = landmark(SWIM_HALL);
        let model = Model::load(landmark).expect("swim hall parses");

        let (cx, cz) = (landmark.anchor_x as i32, landmark.anchor_z as i32);
        assert!(model.excavated(cx, cz), "hall interior must be dug out");

        // The roof cables and their anchors reach far past the hall; the park
        // under them has to survive.
        assert!(!model.excavated(model.min_x, cz), "west cable anchor");
        assert!(!model.excavated(model.max_x, cz), "east cable anchor");
        assert!(!model.excavated(cx, model.max_z), "south cable anchor");

        // The dug-out area must be the façade's interior, not its bounding box.
        let facade: Vec<(i32, i32)> = model
            .voxels
            .iter()
            .filter(|v| model.palette[v.3 as usize].block == POLISHED_BASALT)
            .map(|v| (i32::from(v.0), i32::from(v.2)))
            .collect();
        let (fx0, fx1) = (
            facade.iter().map(|p| p.0).min().unwrap(),
            facade.iter().map(|p| p.0).max().unwrap(),
        );
        let (fz0, fz1) = (
            facade.iter().map(|p| p.1).min().unwrap(),
            facade.iter().map(|p| p.1).max().unwrap(),
        );
        // Corners of the façade bbox lie outside the rounded ring.
        assert!(
            !model.excavated(fx0, fz0) && !model.excavated(fx1, fz1),
            "façade bbox corners are outside the ring, so not interior"
        );
    }

    // The event hall's cladding wraps it, so the same hole-fill must isolate the
    // building from the roof cables reaching out across the park.
    #[test]
    fn event_hall_excavates_only_its_building() {
        let landmark = landmark(EVENT_HALL);
        let model = Model::load(landmark).expect("event hall parses");
        let (cx, cz) = (landmark.anchor_x as i32, landmark.anchor_z as i32);
        assert!(model.excavated(cx, cz), "hall interior must be dug out");
        assert!(!model.excavated(model.min_x, model.min_z), "cable corner");
        assert!(!model.excavated(model.max_x, model.max_z), "cable corner");

        // The dug-out area should be the building, not the model's full reach.
        let dug = model.excavated.iter().filter(|&&e| e).count();
        let footprint =
            ((model.max_x - model.min_x + 1) * (model.max_z - model.min_z + 1)) as usize;
        assert!(
            dug * 2 < footprint,
            "excavation covers {dug} of {footprint} cells, too much of the reach"
        );
    }

    // Without a marker the whole footprint is dug out, as for the tower.
    #[test]
    fn markerless_landmark_excavates_everything() {
        let model = Model::load(landmark(TOWER)).expect("tower parses");
        assert!(model.excavated(model.min_x, model.min_z));
        assert!(model.excavated(model.max_x, model.max_z));
        assert!(!model.excavated(model.min_x - 1, model.min_z));
    }

    // A cell the model does not fill must not resolve to a neighbour's column.
    #[test]
    fn empty_columns_resolve_to_none() {
        let model = Model::load(landmark(TOWER)).expect("tower parses");
        assert!(model.column(model.min_x - 1, model.min_z).is_none());
        assert!(model.column(model.max_x + 1, model.max_z).is_none());
        let filled: HashSet<(i16, i16)> = model.voxels.iter().map(|v| (v.0, v.2)).collect();
        for z in model.min_z..=model.max_z {
            for x in model.min_x..=model.max_x {
                let has = filled.contains(&(x as i16, z as i16));
                assert_eq!(has, model.column(x, z).is_some(), "column ({x}, {z})");
            }
        }
    }
}
