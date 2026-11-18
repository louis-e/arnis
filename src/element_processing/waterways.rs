use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::field_texture::PLOT_PLANTS;
use crate::floodfill_cache::{CoordinateBitmap, RoadMaskBitmap};
use crate::mapillary::geometry::parse_length_m;
use crate::osm_parser::{ProcessedNode, ProcessedWay};
use crate::water_depth::{at_bore, carve_water_column, BigWaterField};
use crate::world_editor::WorldEditor;
use std::collections::HashMap;

/// What a channel needs to dig its `--river-bed`; the field marks no river when that is off.
pub struct ChannelBed<'a> {
    pub field: &'a BigWaterField,
    pub road_mask: &'a RoadMaskBitmap,
    pub tunnel_footprint: &'a RoadMaskBitmap,
    /// Decks over the channel; under one that clears the water the bed is dug as elsewhere.
    pub decks: &'a BridgeSurfaceMap,
}

/// How line waterways are drawn. Their drawing and every mask of their water take the same
/// one, so the water a mask marks is the water drawn.
#[derive(Clone, Copy)]
pub struct ChannelShape<'a> {
    /// Blocks per metre (`--scale`): widths are lengths.
    pub scale: f64,
    /// Narrows every channel (`--water-detail scaled` on small maps), in blocks.
    pub width_cap: Option<i32>,
    /// With a water option on, the road and path surfaces at grade a channel is piped under:
    /// nothing is drawn around a centreline cell on one, so a path along a stream leaves no
    /// strip of water on either side (`Ground::pipe_channels_under_roads`).
    pub roads_at_grade: Option<&'a CoordinateBitmap>,
}

impl Default for ChannelShape<'_> {
    fn default() -> Self {
        Self {
            scale: 1.0,
            width_cap: None,
            roads_at_grade: None,
        }
    }
}

impl ChannelShape<'_> {
    /// Width of a channel of this type and tags, in blocks.
    pub fn width(&self, waterway_type: &str, tags: &HashMap<String, String>) -> i32 {
        capped_waterway_width(waterway_type, tags, self.scale, self.width_cap)
    }
}

pub fn generate_waterways(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    shape: ChannelShape,
    bed: &ChannelBed,
) {
    let Some(waterway_type) = element.tags.get("waterway") else {
        return;
    };
    // waterway=* structures are not channels; outlining a dam draws canals down it.
    // Culverts and pipes are not open water; they would cut channels through banks.
    if !is_channel_waterway(waterway_type) || is_underground_waterway(&element.tags) {
        return;
    }
    let width = shape.width(waterway_type, &element.tags);
    let nodes = &element.nodes;
    // The water surface at the segment ends, and at the centre point drawn last.
    let mut ends: Option<(usize, i32, i32)> = None;
    let mut surface: Option<(Centre, i32)> = None;
    let everywhere = (i32::MIN, i32::MAX, i32::MIN, i32::MAX);
    channel_walk(nodes, width, shape.roads_at_grade, everywhere, |c, x, z| {
        let water_y = match surface {
            Some((at, y)) if at == c => y,
            _ => {
                let (y0, y1) = match ends {
                    Some((segment, y0, y1)) if segment == c.segment => (y0, y1),
                    _ => {
                        let (a, b) = (nodes[c.segment].xz(), nodes[c.segment + 1].xz());
                        let y0 = editor.get_water_level(a.x, a.z);
                        let y1 = editor.get_water_level(b.x, b.z);
                        ends = Some((c.segment, y0, y1));
                        (y0, y1)
                    }
                };
                let y = ramped_surface(y0, y1, c).min(editor.get_water_level(c.x, c.z));
                surface = Some((c, y));
                y
            }
        };
        fill_channel_cell(editor, x, z, water_y, bed);
    });
}

/// The surface of a segment from water level `y0` at its start to `y1` at its end, at
/// centre point `c`. A flat Y per segment leaves puddles near the lower node, so it ramps
/// between the ends; the caller clamps it to the local surface so the sheet never floats.
fn ramped_surface(y0: i32, y1: i32, c: Centre) -> i32 {
    let last = c.steps as i64;
    if last == 0 {
        return y0.min(y1);
    }
    let dy = i64::from(y1 - y0);
    let num = dy * c.step as i64;
    y0 + ((2 * num + last * dy.signum()) / (2 * last)) as i32
}

/// False for `waterway=*` values that are structures or points, not a channel.
pub fn is_channel_waterway(waterway_type: &str) -> bool {
    !matches!(
        waterway_type,
        "dam"
            | "weir"
            | "lock_gate"
            | "waterfall"
            | "rapids"
            | "boatyard"
            | "fuel"
            | "dock"
            | "riverbank"
            | "water_point"
            | "turning_point"
            | "sluice_gate"
            | "fish_pass"
            | "security_lock"
            | "milestone"
            | "check_dam"
            | "floating_barrier"
    )
}

/// True for waterways underground: any `tunnel=*` other than `no`, or a negative layer.
pub fn is_underground_waterway(tags: &std::collections::HashMap<String, String>) -> bool {
    if tags
        .get("tunnel")
        .is_some_and(|v| !matches!(v.as_str(), "no" | "0" | "false"))
    {
        return true;
    }
    tags.get("layer")
        .and_then(|l| l.trim().parse::<i32>().ok())
        .is_some_and(|l| l < 0)
}

/// Determines channel width based on waterway type.
pub fn get_waterway_width(waterway_type: &str) -> i32 {
    match waterway_type {
        "river" => 8,
        "canal" => 6,
        // Village streams in Bavaria measure 1-2 m, but a two-block creek is lost under
        // its trees and banks; three reads as a stream and stays narrower than a road.
        "stream" => 3,
        "fairway" => 12,
        "flowline" => 2,
        "brook" => 3,
        "ditch" => 1,
        "drain" => 1,
        _ => 4,
    }
}

/// Widest channel a `width=*` tag may ask for. Every renderer of a waterway walks its
/// width squared per centreline point, so an unbounded tag value hangs generation.
pub const MAX_WATERWAY_WIDTH: i32 = 128;

/// Channel width in blocks at `scale` blocks a metre, from `width=*` when it parses and the
/// type default (metres) otherwise. The tag reads as a length (`1,5`, `2 m`, `6 ft`); of a
/// list such as `3;4` the first value counts. Any positive width draws at least one block.
pub fn waterway_width(waterway_type: &str, tags: &HashMap<String, String>, scale: f64) -> i32 {
    let metres = tags
        .get("width")
        .and_then(|s| s.split(';').next())
        .and_then(parse_length_m)
        .filter(|w| w.is_finite() && *w > 0.0)
        .unwrap_or_else(|| f64::from(get_waterway_width(waterway_type)));
    ((metres * scale).round() as i64).clamp(1, i64::from(MAX_WATERWAY_WIDTH)) as i32
}

/// Widest a stream or ditch is drawn under the small-map cap, so it still reads apart from
/// a river drawn at the cap.
const SMALL_CHANNEL_CAP: i32 = 3;

/// `waterway_width` under the small-map cap: `cap` for rivers and canals, and at most
/// `SMALL_CHANNEL_CAP` for streams and ditches, so the two still read apart.
pub fn capped_waterway_width(
    waterway_type: &str,
    tags: &HashMap<String, String>,
    scale: f64,
    cap: Option<i32>,
) -> i32 {
    let width = waterway_width(waterway_type, tags, scale);
    match cap {
        Some(cap) if get_waterway_width(waterway_type) <= 3 => {
            width.min(cap).min(SMALL_CHANNEL_CAP)
        }
        Some(cap) => width.min(cap),
        None => width,
    }
}

/// Cells a channel `width` blocks wide covers before and after its centreline, across the
/// axis it runs along: `width` cells in all, an even width's extra one on the far side.
/// Every mask of a drawn channel stamps this, so they agree on its banks.
pub fn channel_reach(width: i32) -> (i32, i32) {
    let width = width.max(1);
    ((width - 1) / 2, width / 2)
}

/// The cells a channel `width` blocks wide draws for its centreline cell (x, z), reached
/// from centreline cell `prev`: the `channel_reach` square around it, and for a one-block
/// channel that steps diagonally, the corner that joins the two cells side to side.
pub fn channel_cells_at(
    x: i32,
    z: i32,
    prev: Option<(i32, i32)>,
    width: i32,
) -> impl Iterator<Item = (i32, i32)> {
    let (before, after) = channel_reach(width);
    let corner = prev
        .filter(|&(px, pz)| width <= 1 && px != x && pz != z)
        .map(|(px, _)| (px, z));
    corner.into_iter().chain(
        ((x - before)..=(x + after))
            .flat_map(move |cx| ((z - before)..=(z + after)).map(move |cz| (cx, cz))),
    )
}

/// Every cell a channel `width` blocks wide draws along `nodes`, as `channel_walk` gives
/// them: the footprint the water masks share with the drawn channel.
pub fn channel_cells(
    nodes: &[ProcessedNode],
    width: i32,
    roads_at_grade: Option<&CoordinateBitmap>,
    area: (i32, i32, i32, i32),
    mut cell: impl FnMut(i32, i32),
) {
    channel_walk(nodes, width, roads_at_grade, area, |_, x, z| cell(x, z));
}

/// A centreline cell of a channel, on the line from node `segment` to the next, `step`
/// cells along it of `steps`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Centre {
    pub x: i32,
    pub z: i32,
    pub segment: usize,
    pub step: usize,
    pub steps: usize,
}

/// Every cell a channel `width` blocks wide draws along `nodes`, with the centreline cell it
/// draws it for: `channel_cells_at` for each cell of its centreline, except one on
/// `roads_at_grade`, where it is piped under the road. The drawing and every mask of its
/// water walk this, so they cover the same cells. Segments that come no nearer than the
/// channel's reach to the inclusive `area` (min x, max x, min z, max z) are skipped. A
/// cell may come more than once.
pub fn channel_walk(
    nodes: &[ProcessedNode],
    width: i32,
    roads_at_grade: Option<&CoordinateBitmap>,
    area: (i32, i32, i32, i32),
    mut cell: impl FnMut(Centre, i32, i32),
) {
    let (before, after) = channel_reach(width);
    let reach = before.max(after) + 1;
    let mut prev = None;
    for (segment, pair) in nodes.windows(2).enumerate() {
        let (a, b) = (pair[0].xz(), pair[1].xz());
        if a.x.max(b.x).saturating_add(reach) < area.0
            || a.x.min(b.x).saturating_sub(reach) > area.1
            || a.z.max(b.z).saturating_add(reach) < area.2
            || a.z.min(b.z).saturating_sub(reach) > area.3
        {
            prev = None;
            continue;
        }
        let line = bresenham_line(a.x, 0, a.z, b.x, 0, b.z);
        let steps = line.len().saturating_sub(1);
        for (step, (x, _, z)) in line.into_iter().enumerate() {
            if roads_at_grade.is_some_and(|roads| roads.contains(x, z)) {
                // No corner joins the cells either side of the road.
                prev = None;
                continue;
            }
            let centre = Centre {
                x,
                z,
                segment,
                step,
                steps,
            };
            for (cx, cz) in channel_cells_at(x, z, prev, width) {
                cell(centre, cx, cz);
            }
            prev = Some((x, z));
        }
    }
}

/// Water in one cell of a channel whose surface is `target_water_y` there. Skips the cell
/// where the terrain is above that surface, with a small tolerance to avoid gaps on gentle
/// slopes (can create stepped banks).
fn fill_channel_cell(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    target_water_y: i32,
    bed: &ChannelBed,
) {
    const BANK_TOLERANCE: i32 = 2;
    let ground_y = editor.get_ground_level(x, z);
    // Only place water where terrain is at or below the water surface,
    // but allow small elevation steps to avoid gaps on gentle slopes.
    let water_y = if ground_y <= target_water_y {
        target_water_y
    } else if ground_y <= target_water_y + BANK_TOLERANCE
        && !editor.block_exists_absolute(x, ground_y, z)
    {
        ground_y
    } else {
        return;
    };

    editor.set_block_absolute(WATER, x, water_y, z, None, None);
    // A river's or missed canal's depth and bed, unless a road or bore owns it.
    let missed = || !editor.is_lc_water(x, z) && editor.is_mapped_water(x, z);
    let under_deck = || bed.decks.deck_clears(x, z, water_y) && !bed.decks.over_grade_way(x, z);
    if (bed.field.is_river(x, z) || missed())
        && (!bed.road_mask.contains(x, z) || under_deck())
        && !at_bore(bed.field, bed.tunnel_footprint, x, z)
        && editor.check_for_block_absolute(x, water_y, z, Some(&[WATER]), None)
    {
        let depth = bed.field.depth_at(x, z);
        carve_water_column(editor, x, z, water_y, depth, bed.road_mask, bed.field);
    }

    // Clear vegetation above the water, a sunflower's top half too.
    editor.set_block_absolute(AIR, x, water_y + 1, z, Some(PLOT_PLANTS), None);
    editor.set_block_absolute(AIR, x, water_y + 2, z, Some(&[SUNFLOWER_UPPER]), None);
}

#[cfg(test)]
mod waterway_profile_tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::ground::Ground;
    use crate::osm_parser::ProcessedNode;
    use std::path::PathBuf;
    use std::sync::Arc;

    const SIDE: usize = 64;
    const CENTER_X: i32 = 32;
    const FIRST_Z: i32 = 4;
    const LAST_Z: i32 = 52;

    fn world_bbox() -> XZBBox {
        XZBBox::rect_from_min_max(0, 0, SIDE as i32 - 1, SIDE as i32 - 1).unwrap()
    }

    /// Terrain dropping `per_block` blocks per step along Z, gentle enough that
    /// `water_level` never snaps, so a column's expected surface is its own level.
    fn editor_over_slope(bbox: &XZBBox, top: f64, per_block: f64) -> WorldEditor<'_> {
        let heights: Vec<Vec<f32>> = (0..SIDE)
            .map(|z| vec![(top - per_block * z as f64) as f32; SIDE])
            .collect();
        let ground = Ground::new_elevation_test(heights, SIDE, SIDE);
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor = WorldEditor::new(PathBuf::from("/dev/null/unused"), bbox, llbbox);
        editor.set_ground(Arc::new(ground));
        editor
    }

    fn stream(from_z: i32, to_z: i32) -> ProcessedWay {
        let node = |id: u64, z: i32| ProcessedNode {
            id,
            tags: HashMap::new(),
            x: CENTER_X,
            z,
        };
        let mut tags = HashMap::new();
        tags.insert("waterway".to_string(), "stream".to_string());
        ProcessedWay {
            id: 1,
            nodes: vec![node(1, from_z), node(2, to_z)],
            tags,
        }
    }

    fn no_bed() -> ChannelBed<'static> {
        static FIELD: std::sync::LazyLock<BigWaterField> =
            std::sync::LazyLock::new(BigWaterField::empty);
        static NONE: std::sync::LazyLock<RoadMaskBitmap> =
            std::sync::LazyLock::new(RoadMaskBitmap::new_empty);
        static DECKS: std::sync::LazyLock<BridgeSurfaceMap> =
            std::sync::LazyLock::new(BridgeSurfaceMap::empty);
        ChannelBed {
            field: &FIELD,
            road_mask: &NONE,
            tunnel_footprint: &NONE,
            decks: &DECKS,
        }
    }

    fn water_ys(editor: &WorldEditor, x: i32, z: i32) -> Vec<i32> {
        (-8..40)
            .filter(|&y| editor.check_for_block_absolute(x, y, z, Some(&[WATER]), None))
            .collect()
    }

    #[test]
    fn a_dropping_channel_follows_the_terrain_instead_of_pooling_at_the_lower_node() {
        let bbox = world_bbox();
        let mut editor = editor_over_slope(&bbox, 16.0, 0.25);
        generate_waterways(
            &mut editor,
            &stream(FIRST_Z, LAST_Z),
            ChannelShape::default(),
            &no_bed(),
        );

        let upper = editor.get_water_level(CENTER_X, FIRST_Z);
        let lower = editor.get_water_level(CENTER_X, LAST_Z);
        assert_eq!((upper, lower), (15, 3), "test fixture no longer drops 12");

        for z in FIRST_Z..=LAST_Z {
            let local = editor.get_water_level(CENTER_X, z);
            let ys = water_ys(&editor, CENTER_X, z);
            assert!(!ys.is_empty(), "dry gap at z={z}, local level {local}");
            let top = *ys.iter().max().unwrap();
            assert!(
                (top - local).abs() <= 1,
                "z={z}: surface {top} does not track the local level {local}"
            );
        }
    }

    /// With a river bed field a line-only channel is dug to the field's depth and gets a
    /// bed, except under a road.
    #[test]
    fn a_line_channel_is_dug_to_the_river_bed() {
        let bbox = world_bbox();
        let mut editor = editor_over_slope(&bbox, 8.0, 0.0);
        let mut field = BigWaterField::empty();
        field.set_river_bed(crate::river_bed::RiverBedField::uniform(
            0, 0, SIDE, SIDE, 2,
        ));
        let mut road_mask = RoadMaskBitmap::new(&bbox);
        road_mask.set(CENTER_X, 30);
        let none = RoadMaskBitmap::new_empty();
        let decks = BridgeSurfaceMap::empty();
        let bed = ChannelBed {
            field: &field,
            road_mask: &road_mask,
            tunnel_footprint: &none,
            decks: &decks,
        };
        // The road is at grade, so the channel is piped under it.
        let shape = ChannelShape {
            roads_at_grade: Some(&road_mask),
            ..Default::default()
        };
        generate_waterways(&mut editor, &stream(FIRST_Z, LAST_Z), shape, &bed);

        // Two blocks deeper; the lowest may hold seagrass instead of plain water.
        assert!(water_ys(&editor, CENTER_X, 20).ends_with(&[7, 8]));
        assert!(editor.block_exists_absolute(CENTER_X, 6, 20), "not dug");
        assert!(
            !editor.check_for_block_absolute(CENTER_X, 5, 20, Some(&[WATER]), None)
                && editor.block_exists_absolute(CENTER_X, 5, 20),
            "no bed"
        );
        assert_eq!(water_ys(&editor, CENTER_X, 30), vec![8], "dug under a road");
    }

    #[test]
    fn a_canal_the_land_cover_missed_is_dug_past_its_banks() {
        use crate::land_cover::{InlandWater, LandCoverData};
        let bbox = world_bbox();
        let mut mapped = InlandWater::new(SIDE, SIDE);
        for z in 0..SIDE {
            for x in 24..=40 {
                mapped.set(x, z);
            }
        }
        let lc = LandCoverData {
            grid: vec![vec![10; SIDE]; SIDE],
            water_distance: vec![vec![0; SIDE]; SIDE],
            water_blend_cache: Default::default(),
            beach_tiles: Default::default(),
            inland_water: mapped,
            width: SIDE,
            height: SIDE,
            cells_per_meter: 1.0,
        };
        let ground = Ground::new_elevation_test(vec![vec![8.0; SIDE]; SIDE], SIDE, SIDE)
            .with_land_cover_test(lc);
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        let mut editor = WorldEditor::new(PathBuf::from("/dev/null/unused"), &bbox, llbbox);
        editor.set_ground(Arc::new(ground));
        let mut canal = stream(FIRST_Z, LAST_Z);
        canal
            .tags
            .insert("waterway".to_string(), "canal".to_string());
        generate_waterways(&mut editor, &canal, ChannelShape::default(), &no_bed());
        assert_eq!(water_ys(&editor, CENTER_X, 30), vec![6, 7, 8]);
    }

    fn wet_columns_in_row(editor: &WorldEditor, z: i32) -> Vec<i32> {
        (0..SIDE as i32)
            .filter(|&x| !water_ys(editor, x, z).is_empty())
            .collect()
    }

    /// A channel is as wide as its tag or its type's default, not two blocks more.
    #[test]
    fn a_channel_draws_exactly_its_width() {
        let bbox = world_bbox();
        for (kind, width, want) in [
            ("stream", None, vec![31, 32, 33]),
            ("stream", Some("5"), vec![30, 31, 32, 33, 34]),
            ("ditch", None, vec![32]),
            ("river", None, (29..=36).collect()),
        ] {
            let mut editor = editor_over_slope(&bbox, 8.0, 0.0);
            let mut way = stream(FIRST_Z, LAST_Z);
            way.tags.insert("waterway".to_string(), kind.to_string());
            if let Some(w) = width {
                way.tags.insert("width".to_string(), w.to_string());
            }
            generate_waterways(&mut editor, &way, ChannelShape::default(), &no_bed());
            assert_eq!(wet_columns_in_row(&editor, 30), want, "{kind} {width:?}");
        }
    }

    /// A one-block ditch running diagonally stays joined side to side.
    #[test]
    fn a_diagonal_ditch_stays_joined() {
        let bbox = world_bbox();
        let mut editor = editor_over_slope(&bbox, 8.0, 0.0);
        let mut way = stream(10, 40);
        way.tags.insert("waterway".to_string(), "ditch".to_string());
        way.nodes[0].x = 10;
        way.nodes[1].x = 40;
        generate_waterways(&mut editor, &way, ChannelShape::default(), &no_bed());
        let wet = |x: i32, z: i32| !water_ys(&editor, x, z).is_empty();
        for i in 10..40 {
            assert!(wet(i, i), "gap at {i}");
            assert!(
                wet(i, i + 1) || wet(i + 1, i),
                "only a corner joins {i} and {}",
                i + 1
            );
        }
        // Two cells per step along the diagonal, no more.
        let total = (0..SIDE as i32)
            .flat_map(|x| (0..SIDE as i32).map(move |z| (x, z)))
            .filter(|&(x, z)| wet(x, z))
            .count();
        assert_eq!(total, 31 + 30);
    }

    /// Widths are lengths, tagged or the type's own, so they follow the scale; the drawing
    /// and the masks take them from the same shape.
    #[test]
    fn widths_follow_the_scale() {
        let tags = |w: Option<&str>| -> HashMap<String, String> {
            w.map(|w| HashMap::from([("width".to_string(), w.to_string())]))
                .unwrap_or_default()
        };
        for (kind, width, scale, want) in [
            ("ditch", None, 1.0, 1),
            ("ditch", None, 2.0, 2),
            ("stream", None, 2.0, 6),
            ("stream", None, 0.5, 2),
            ("river", None, 0.25, 2),
            ("ditch", Some("1.1"), 2.0, 2),
            ("ditch", Some("2.8"), 2.0, 6),
            ("stream", Some("0.3"), 0.5, 1),
            ("river", Some("100"), 2.0, MAX_WATERWAY_WIDTH),
        ] {
            assert_eq!(
                waterway_width(kind, &tags(width), scale),
                want,
                "{kind} {width:?} at {scale}"
            );
            let shape = ChannelShape {
                scale,
                ..Default::default()
            };
            assert_eq!(shape.width(kind, &tags(width)), want);
        }
        // At full scale every width is as it was.
        for kind in [
            "river", "canal", "stream", "brook", "ditch", "drain", "fairway",
        ] {
            assert_eq!(
                waterway_width(kind, &tags(None), 1.0),
                get_waterway_width(kind)
            );
        }
    }

    /// Width tags read as lengths; of a list the first value counts, and any positive width
    /// draws at least one block.
    #[test]
    fn a_width_tag_reads_as_a_length() {
        let width = |tag: &str| {
            let tags = HashMap::from([("width".to_string(), tag.to_string())]);
            waterway_width("stream", &tags, 1.0)
        };
        for (tag, want) in [
            ("2", 2),
            ("2m", 2),
            ("2 m", 2),
            ("1,5", 2),
            ("2.4", 2),
            ("0.4", 1),
            ("0.5", 1),
            ("10 ft", 3),
            ("3 ft", 1),
            ("3;4", 3),
            ("4 m;6 m", 4),
            ("500", MAX_WATERWAY_WIDTH),
            // No width: the stream's own.
            ("0", 3),
            ("-2", 3),
            ("wide", 3),
            ("", 3),
        ] {
            assert_eq!(width(tag), want, "width={tag:?}");
        }
    }

    /// `--water-detail scaled` on a small map: rivers and canals at most 5 wide, streams
    /// and ditches at most 3, and the narrow ones as they are.
    #[test]
    fn the_small_map_cap_narrows_rivers_to_five_and_streams_to_three() {
        let cap = crate::water_detail::WaterDetail::Scaled.channel_width_cap(0.5);
        assert_eq!(cap, Some(5));
        assert_eq!(
            crate::water_detail::WaterDetail::Scaled.channel_width_cap(1.0),
            None
        );
        let tags = |width: Option<&str>| -> HashMap<String, String> {
            width
                .map(|w| HashMap::from([("width".to_string(), w.to_string())]))
                .unwrap_or_default()
        };
        for (kind, width, want) in [
            ("river", None, 5),
            ("river", Some("4"), 4),
            ("river", Some("30"), 5),
            ("canal", None, 5),
            ("stream", None, 3),
            ("stream", Some("5"), 3),
            ("stream", Some("2"), 2),
            ("brook", Some("8"), 3),
            ("ditch", None, 1),
            ("ditch", Some("4"), 3),
        ] {
            assert_eq!(
                capped_waterway_width(kind, &tags(width), 1.0, cap),
                want,
                "{kind} {width:?}"
            );
        }
        // The drawing takes the same cap.
        let bbox = world_bbox();
        let mut editor = editor_over_slope(&bbox, 8.0, 0.0);
        let mut river = stream(FIRST_Z, LAST_Z);
        river
            .tags
            .insert("waterway".to_string(), "river".to_string());
        let shape = ChannelShape {
            width_cap: cap,
            ..Default::default()
        };
        generate_waterways(&mut editor, &river, shape, &no_bed());
        assert_eq!(wet_columns_in_row(&editor, 30), vec![30, 31, 32, 33, 34]);
    }

    /// The water masks stamp `channel_cells`, so they must be the cells the channel draws.
    #[test]
    fn the_masks_footprint_is_the_drawn_channel() {
        let bbox = world_bbox();
        for (kind, width) in [("ditch", None), ("stream", None), ("stream", Some("2"))] {
            let mut editor = editor_over_slope(&bbox, 8.0, 0.0);
            let mut way = stream(10, 40);
            way.tags.insert("waterway".to_string(), kind.to_string());
            if let Some(w) = width {
                way.tags.insert("width".to_string(), w.to_string());
            }
            // A bend: diagonal, then straight.
            way.nodes[0].x = 10;
            way.nodes[1].x = 40;
            way.nodes.push(ProcessedNode {
                id: 3,
                tags: HashMap::new(),
                x: 40,
                z: 55,
            });
            generate_waterways(&mut editor, &way, ChannelShape::default(), &no_bed());
            let drawn: std::collections::BTreeSet<(i32, i32)> = (0..SIDE as i32)
                .flat_map(|x| (0..SIDE as i32).map(move |z| (x, z)))
                .filter(|&(x, z)| !water_ys(&editor, x, z).is_empty())
                .collect();
            let mut stamped = std::collections::BTreeSet::new();
            let w = capped_waterway_width(kind, &way.tags, 1.0, None);
            channel_cells(
                &way.nodes,
                w,
                None,
                (0, SIDE as i32 - 1, 0, SIDE as i32 - 1),
                |x, z| {
                    stamped.insert((x, z));
                },
            );
            assert_eq!(drawn, stamped, "{kind} {width:?}");
        }
    }

    /// Piped under the roads at grade, the channel and its masks leave out the same cells:
    /// a path along the stream gets no strip of water on either side, and the land cover
    /// marks no water there that the ground would paint.
    #[test]
    fn the_masks_leave_out_the_cells_the_channel_is_piped_under() {
        use crate::land_cover::{InlandWater, LandCoverData};
        let bbox = world_bbox();
        // A footway across the stream at z 20-21, and a path down its centreline below z 40.
        let mut roads = CoordinateBitmap::new(&bbox);
        for x in 0..SIDE as i32 {
            roads.set(x, 20);
            roads.set(x, 21);
        }
        for z in 40..=LAST_Z {
            roads.set(CENTER_X, z);
        }
        let shape = ChannelShape {
            roads_at_grade: Some(&roads),
            ..Default::default()
        };
        let way = stream(FIRST_Z, LAST_Z);
        let mut editor = editor_over_slope(&bbox, 8.0, 0.0);
        generate_waterways(&mut editor, &way, shape, &no_bed());
        let all = (0..SIDE as i32).flat_map(|x| (0..SIDE as i32).map(move |z| (x, z)));
        let drawn: std::collections::BTreeSet<(i32, i32)> = all
            .clone()
            .filter(|&(x, z)| !water_ys(&editor, x, z).is_empty())
            .collect();
        assert!(drawn.contains(&(CENTER_X, 30)), "the stream itself");
        for z in 41..=LAST_Z {
            for x in CENTER_X - 1..=CENTER_X + 1 {
                assert!(
                    !drawn.contains(&(x, z)),
                    "water beside the path at ({x}, {z})"
                );
            }
        }

        let mut stamped = std::collections::BTreeSet::new();
        let w = shape.width("stream", &way.tags);
        let area = (0, SIDE as i32 - 1, 0, SIDE as i32 - 1);
        channel_cells(&way.nodes, w, Some(&roads), area, |x, z| {
            stamped.insert((x, z));
        });
        assert_eq!(drawn, stamped);

        // The land cover water override, with no ESA water to guard it, marks just these.
        let mut lc = LandCoverData {
            grid: vec![vec![30; SIDE]; SIDE],
            water_distance: vec![vec![0; SIDE]; SIDE],
            water_blend_cache: Default::default(),
            beach_tiles: Default::default(),
            inland_water: InlandWater::new(SIDE, SIDE),
            width: SIDE,
            height: SIDE,
            cells_per_meter: 1.0,
        };
        let mut heights = vec![vec![8.0f32; SIDE]; SIDE];
        let elements = [crate::osm_parser::ProcessedElement::Way(way.clone())];
        crate::land_cover::osm_water_override::apply_osm_water_override(
            &mut lc,
            &mut heights,
            SIDE,
            SIDE,
            &elements,
            &bbox,
            shape,
        );
        let marked: std::collections::BTreeSet<(i32, i32)> = all
            .filter(|&(x, z)| lc.grid[z as usize][x as usize] == crate::land_cover::LC_WATER)
            .collect();
        assert_eq!(drawn, marked);
    }

    #[test]
    fn a_flat_channel_sits_at_the_single_shared_level() {
        let bbox = world_bbox();
        let mut editor = editor_over_slope(&bbox, 8.0, 0.0);
        generate_waterways(
            &mut editor,
            &stream(FIRST_Z, LAST_Z),
            ChannelShape::default(),
            &no_bed(),
        );

        for z in FIRST_Z..=LAST_Z {
            assert_eq!(
                water_ys(&editor, CENTER_X, z),
                vec![8],
                "flat terrain must place one sheet at the shared level, z={z}"
            );
        }
    }

    #[test]
    fn a_one_block_drop_moves_the_upper_half_by_at_most_one_block() {
        let bbox = world_bbox();
        let mut editor = editor_over_slope(&bbox, 8.0, 0.02);
        generate_waterways(
            &mut editor,
            &stream(FIRST_Z, LAST_Z),
            ChannelShape::default(),
            &no_bed(),
        );

        let upper = editor.get_water_level(CENTER_X, FIRST_Z);
        let lower = editor.get_water_level(CENTER_X, LAST_Z);
        assert_eq!((upper, lower), (8, 7), "test fixture no longer drops 1");

        let top_at =
            |editor: &WorldEditor, z: i32| *water_ys(editor, CENTER_X, z).iter().max().unwrap();
        assert_eq!(top_at(&editor, FIRST_Z), upper, "upstream end left behind");
        assert_eq!(top_at(&editor, LAST_Z), lower, "downstream end lifted");
        for z in FIRST_Z..=LAST_Z {
            let top = top_at(&editor, z);
            assert!(
                top == lower || top == lower + 1,
                "z={z}: {top} is more than one block off the pre-ramp level {lower}"
            );
        }
    }
}
