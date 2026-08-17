//! Hand-built models for landmarks whose OpenStreetMap geometry is too coarse.
//! Matched by Wikidata QID so a model only replaces that one object, and placed
//! from the anchor lat/lon because ways get clipped to the selected area.

use std::collections::HashSet;
use std::sync::Arc;

use colored::Colorize;

use crate::args::Args;
use crate::block_definitions::{
    Block, BlockWithProperties, AIR, GLASS, GREEN_STAINED_HARDENED_CLAY, GREEN_WOOL, STONE,
    WHITE_CONCRETE,
};
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use crate::osm_parser::ProcessedElement;
use crate::structures::schematic::{load_palettized, rotate_props};
use crate::world_editor::WorldEditor;

/// One bundled landmark model.
struct Landmark {
    /// Shown in the placement log line.
    name: &'static str,
    /// Wikidata QID of the object this model replaces.
    qid: &'static str,
    /// Fallback identity if the element lost its `wikidata` tag.
    osm_ids: &'static [(&'static str, u64)],
    /// Gzipped Sponge `.schem`, north-up at one block per metre.
    schematic: &'static [u8],
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
}

/// Anchors are measured by matching each model against its real OSM footprint.
const LANDMARKS: &[Landmark] = &[
    Landmark {
        name: "Olympiastadion München",
        qid: "Q131610",
        osm_ids: &[("way", 419_656_920)],
        schematic: include_bytes!("../assets/structures/landmarks/olympiastadion_munich.schem"),
        // Centre of the leisure=stadium footprint; model bowl 230x261 vs 241x260 m.
        lat: 48.173_101_2,
        lon: 11.546_483_3,
        anchor_x: 224.5,
        anchor_z: 238.0,
        // Pitch slab, with 12 layers of below-grade structure under it.
        ground_y: 12,
        ground_offset: -20,
        // The tribune ring encloses track and pitch, so filling it gives the bowl.
        interior_marker: &[GREEN_WOOL, GREEN_STAINED_HARDENED_CLAY],
        ground_overlap: 10,
        // Stops short of the retail units 155 m east and the roofs 210 m north.
        suppress_half_x: 135.0,
        suppress_half_z: 145.0,
        reach_m: 240,
    },
    Landmark {
        name: "Olympiahalle",
        qid: "Q48849",
        osm_ids: &[("way", 303_099_272)],
        schematic: include_bytes!("../assets/structures/landmarks/olympiahalle_munich.schem"),
        // Fitted by overlapping the model body with the OSM polygon: 16847/16860 m2.
        lat: 48.174_905_8,
        lon: 11.550_030_8,
        anchor_x: 115.0,
        anchor_z: 130.0,
        // Terrain-following base; its columns want to rest on Y -52.
        ground_y: 0,
        ground_offset: -9,
        // The white cladding wraps the hall; roof cables outside stay buried.
        interior_marker: &[WHITE_CONCRETE],
        ground_overlap: 0,
        // Takes in the admin wing but not the Kleine Olympiahalle 168 m east.
        suppress_half_x: 95.0,
        suppress_half_z: 75.0,
        reach_m: 160,
    },
    Landmark {
        name: "Olympia-Schwimmhalle",
        qid: "Q3882013",
        osm_ids: &[("way", 227_012_665)],
        schematic: include_bytes!(
            "../assets/structures/landmarks/olympia_schwimmhalle_munich.schem"
        ),
        // The glazed facade encloses 83x124 blocks against the real 82x123 m.
        lat: 48.173_572_3,
        lon: 11.551_479_7,
        anchor_x: 99.0,
        anchor_z: 126.5,
        // Terrain-following base. The sampled ground reads high because the hall
        // is on a rise while its cables anchor on the lower lawn.
        ground_y: 0,
        ground_offset: -6,
        // Outside the facade ring it is roof cable standing in the park.
        interior_marker: &[GLASS],
        // No overlap, or a fringe of terrain would be left inside the hall.
        ground_overlap: 0,
        suppress_half_x: 50.0,
        suppress_half_z: 70.0,
        reach_m: 160,
    },
    Landmark {
        name: "Olympiaturm",
        qid: "Q599148",
        osm_ids: &[("way", 164_084_344)],
        schematic: include_bytes!("../assets/structures/landmarks/olympiaturm_munich.schem"),
        // Centre of the tower's OSM footprint.
        lat: 48.174_409_5,
        lon: 11.553_740_1,
        anchor_x: 11.5,
        anchor_z: 13.5,
        ground_y: 0,
        ground_offset: 0,
        // All structure, no hole.
        interior_marker: &[],
        ground_overlap: 0,
        suppress_half_x: 22.0,
        suppress_half_z: 22.0,
        reach_m: 30,
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

    /// Regions a landmark may write to, so stream-to-disk keeps them resident.
    pub fn deferred_region_keys(&self, scale: f64) -> Vec<(i32, i32)> {
        self.placements
            .iter()
            .flat_map(|p| {
                let r = (p.landmark.reach_m as f64 * scale).ceil() as i32;
                crate::models_3d::region_keys_around(p.world_x, p.world_z, r)
            })
            .collect()
    }
}

/// Resolve the landmarks inside this world and collect what they replace.
pub fn prescan(
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    llbbox: LLBBox,
    args: &Args,
) -> LandmarkPrescan {
    let mut placements: Vec<LandmarkPlacement> = Vec::new();
    let mut suppressed: HashSet<(&'static str, u64)> = HashSet::new();

    // --no-3d means OpenStreetMap only, and terrain-only renders no objects.
    if !args.use_3d || args.skip_objects() {
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
    let (transformer, pre_rotation_bbox) = match args.projection {
        crate::projection::ProjectionKind::WebMercator => {
            let origin_lat = (llbbox.min().lat() + llbbox.max().lat()) / 2.0;
            let origin_lon = (llbbox.min().lng() + llbbox.max().lng()) / 2.0;
            let proj =
                crate::projection::WebMercatorProjection::new(origin_lat, origin_lon, args.scale);
            CoordTransformer::with_projection(&llbbox, args.scale, &proj)
        }
        crate::projection::ProjectionKind::Local => {
            CoordTransformer::llbbox_to_xzbbox(&llbbox, args.scale)
        }
    }
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
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    /// Highest occupied layer, so how far terrain must be cleared.
    max_y: i32,
}

impl Model {
    fn load(landmark: &Landmark) -> Result<Self, String> {
        let parsed = load_palettized(landmark.schematic)?;
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

        Ok(Model {
            palette: parsed.palette,
            voxels,
            columns,
            excavated,
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
fn fill_enclosed(mask: &[bool], span_x: usize, span_z: usize) -> Vec<bool> {
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
    /// Stamp the landmarks, after ground generation so terrain Y is final.
    pub fn place(&self, editor: &mut WorldEditor, args: &Args) {
        if self.placements.is_empty() || !editor.place_schematics() {
            return;
        }
        for placement in &self.placements {
            place_one(editor, args, placement);
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
                    .map(|p| Arc::new(rotate_props(p, quarter)));
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
            let clear_from = if model.excavated(model_x, model_z) {
                column_bottom.min(terrain + 1)
            } else if column_top > terrain {
                terrain + 1
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
    let base = levels[levels.len() / 2] + 1 + landmark.ground_offset;
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
            .filter(|v| model.palette[v.3 as usize].block == GLASS)
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
