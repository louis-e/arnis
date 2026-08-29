use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::deterministic_rng::coord_rng;
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::surfaces::get_blocks_for_surface;
use crate::element_processing::tree::Tree;
use crate::floodfill_cache::{is_oversized_ring, BuildingFootprintBitmap, FloodFillCache};
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::Rng;

/// Salt folded into the element id for the per-block park/garden scatter.
///
/// The scatter is seeded from the absolute (x, z) instead of streamed from a
/// single per-element RNG, so a block decorates identically no matter which
/// tile generated it. The value is arbitrary; only its distinctness from the
/// salts other processors use matters.
const SALT_SCATTER: u64 = 0x2545_f491_4f6c_dd1d;

pub fn generate_leisure(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    bridge_surface: &BridgeSurfaceMap,
) {
    if let Some(leisure_type) = element.tags.get("leisure") {
        let mut previous_node: Option<(i32, i32)> = None;
        let mut corner_count: i32 = 0;
        let mut current_leisure: Vec<(i32, i32)> = vec![];

        // Determine block type based on leisure type
        let mut block_type: Block = match leisure_type.as_str() {
            "park" | "nature_reserve" | "garden" | "disc_golf_course" | "golf_course" => {
                GRASS_BLOCK
            }
            "schoolyard" => BLACK_CONCRETE,
            "playground" | "recreation_ground" | "pitch" | "beach_resort" | "dog_park" => {
                GREEN_STAINED_HARDENED_CLAY
            }
            "swimming_pool" | "swimming_area" => WATER, // Swimming area: Area in a larger body of water for swimming
            "marina" => WATER, // A sort of parking lot for small watercraft
            "bathing_place" => SMOOTH_SANDSTONE, // Could be sand or concrete
            "outdoor_seating" => SMOOTH_STONE, //Usually stone or stone bricks
            "water_park" | "slipway" => LIGHT_GRAY_CONCRETE, // Water park area, not the pool. Usually is concrete
            "ice_rink" => PACKED_ICE, // TODO: Ice for Ice Rink, needs building defined
            _ => GRASS_BLOCK,
        };
        // Explicit surface=* overrides the category default. Leave
        // `block_type` untouched for unknown surface values so existing
        // behaviour is preserved.
        if let Some(surface) = element.tags.get("surface") {
            if let Some(blocks) = get_blocks_for_surface(surface) {
                block_type = blocks[0];
            }
        }

        // Resolve the fill before painting the edge, for the same reason as in natural.rs:
        // a closed ring the fill refused must not leave a border around unfilled ground.
        let filled_area = flood_fill_cache.get_or_compute(element, args.timeout.as_ref());
        if filled_area.is_empty() && is_oversized_ring(element) {
            return;
        }

        // Process leisure area nodes
        for node in &element.nodes {
            if let Some(prev) = previous_node {
                // Draw a line between the current and previous node
                let bresenham_points: Vec<(i32, i32, i32)> =
                    bresenham_line(prev.0, 0, prev.1, node.x, 0, node.z);
                for (bx, _, bz) in bresenham_points {
                    editor.set_block(
                        block_type,
                        bx,
                        0,
                        bz,
                        Some(&[
                            GRASS_BLOCK,
                            STONE_BRICKS,
                            SMOOTH_STONE,
                            LIGHT_GRAY_CONCRETE,
                            COBBLESTONE,
                            GRAY_CONCRETE,
                        ]),
                        None,
                    );
                }

                current_leisure.push((node.x, node.z));
                corner_count += 1;
            }
            previous_node = Some((node.x, node.z));
        }

        // Flood-fill the interior of the leisure area using cache
        if corner_count > 0 {
            for &(x, z) in filled_area.iter() {
                editor.set_block(block_type, x, 0, z, Some(&[GRASS_BLOCK]), None);

                // Land-cover water is skipped because a park often spans its
                // own lake, and the carve after this leaves plants floating.
                if matches!(leisure_type.as_str(), "park" | "garden" | "nature_reserve")
                    && editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK]))
                    && !editor.is_lc_water(x, z)
                {
                    // Seeded from the absolute position, not from a streamed
                    // per-element RNG, so the same block always draws the same
                    // value regardless of which tile generated it.
                    let random_choice: i32 =
                        coord_rng(x, z, element.id ^ SALT_SCATTER).random_range(0..1000);

                    match random_choice {
                        0..30 => {
                            // Plants
                            let plant_choice = match random_choice {
                                0..5 => RED_FLOWER,
                                5..10 => YELLOW_FLOWER,
                                10..16 => BLUE_FLOWER,
                                16..22 => WHITE_FLOWER,
                                22..30 => FERN,
                                _ => unreachable!(),
                            };
                            editor.set_block(plant_choice, x, 1, z, None, None);
                        }
                        30..90 => {
                            // Grass
                            editor.set_block(GRASS, x, 1, z, None, None);
                        }
                        90..105 => {
                            // Oak leaves
                            editor.set_block(OAK_LEAVES, x, 1, z, None, None);
                        }
                        105..120 => {
                            // Only where land cover says woody, else a park
                            // canopies its own meadows. 1/1000 for specimens.
                            if random_choice == 105 || editor.land_cover_backs_trees(x, z) {
                                Tree::create(
                                    editor,
                                    (x, 1, z),
                                    Some(building_footprints),
                                    Some(bridge_surface),
                                );
                            } else {
                                editor.set_block(GRASS, x, 1, z, None, None);
                            }
                        }
                        _ => {}
                    }
                }
            }

            // Stamp bundled playground structures (replaces the old procedural props).
            if matches!(leisure_type.as_str(), "playground" | "recreation_ground") {
                crate::structures::playground::scatter_playgrounds(editor, filled_area.as_slice());
            }

            if leisure_type == "pitch" {
                // Clear park/ground vegetation scattered onto the pitch before marking.
                let vegetation: &[Block] = &[
                    GRASS,
                    FERN,
                    RED_FLOWER,
                    YELLOW_FLOWER,
                    BLUE_FLOWER,
                    WHITE_FLOWER,
                    OAK_LEAVES,
                ];
                for &(x, z) in filled_area.iter() {
                    editor.set_block(AIR, x, 1, z, Some(vegetation), None);
                }
                crate::element_processing::sport_pitches::draw_pitch_markings(
                    editor,
                    element,
                    filled_area.as_slice(),
                    block_type,
                );
            }
        }
    }
}

pub fn generate_leisure_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    bridge_surface: &BridgeSurfaceMap,
) {
    if rel.tags.get("leisure").map(String::as_str) == Some("park") {
        // Process each outer member way individually using cached flood fill.
        // We intentionally do not combine all outer nodes into one mega-way,
        // because that creates a nonsensical polygon spanning the whole relation
        // extent, misses the flood fill cache, and can cause multi-GB allocations.
        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                // Use relation tags so the member inherits the relation's leisure=* type
                let way_with_rel_tags = ProcessedWay {
                    id: member.way.id,
                    nodes: member.way.nodes.clone(),
                    tags: rel.tags.clone(),
                };
                generate_leisure(
                    editor,
                    &way_with_rel_tags,
                    args,
                    flood_fill_cache,
                    building_footprints,
                    bridge_surface,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZBBox;
    use crate::element_processing::building_test_support::{rect_way, test_editor};
    use clap::Parser as _;

    // A tile is generated with a margin and only its inner part is kept, so the
    // seam test below renders one 64x16 area three ways: once whole, and once
    // per 32-wide half with a 16-block margin on the seam side. A decoration
    // anchored just outside a half still lands inside it thanks to the margin,
    // so the two inner halves must reassemble into exactly the whole.
    const AREA_MAX_X: i32 = 63;
    const AREA_MAX_Z: i32 = 15;
    const HALF: i32 = 32;
    const MARGIN: i32 = 16;

    /// Every block the editor holds in `min_x..=max_x` over the test area, in a
    /// fixed (x, z, y) walk order. Test editors have no elevation data, so
    /// ground level is 0 and offset Y equals absolute Y.
    fn snapshot(editor: &WorldEditor, min_x: i32, max_x: i32) -> Vec<(i32, i32, i32, u16)> {
        let mut out: Vec<(i32, i32, i32, u16)> = Vec::new();
        for x in min_x..=max_x {
            for z in 0..=AREA_MAX_Z {
                // Widest vertical span these processors write: a couple of
                // blocks below ground, and the tallest procedural tree tops
                // out around +31.
                for y in -8..=40 {
                    if let Some(block) = editor.get_block_absolute(x, y, z) {
                        out.push((x, y, z, block.id()));
                    }
                }
            }
        }
        out
    }

    /// An empty bridge deck map, the shape these processors expect when no
    /// bridge is anywhere near.
    fn empty_bridge_surface(xzbbox: &XZBBox) -> BridgeSurfaceMap {
        let editor = test_editor(xzbbox);
        let outlines = crate::element_processing::bridge_styles::BridgeOutlineIndex::build(&[]);
        let structures =
            crate::element_processing::bridges::BridgeStructureMap::build(&[], &editor, &outlines);
        BridgeSurfaceMap::build(&[], &structures, 1.0)
    }

    fn seam_test_args() -> Args {
        Args::parse_from(["arnis", "--bbox", "1,2,3,4"])
    }

    /// Asserts the two inner halves reassemble into the whole, and that the
    /// render is varied enough for a misalignment to have shown up.
    fn assert_seam_free(
        label: &str,
        whole: Vec<(i32, i32, i32, u16)>,
        halves: Vec<(i32, i32, i32, u16)>,
    ) {
        assert!(!whole.is_empty(), "{label} rendered nothing");
        let distinct: std::collections::BTreeSet<u16> = whole.iter().map(|b| b.3).collect();
        assert!(
            distinct.len() >= 2,
            "{label} rendered a single block type, so the seam check is vacuous"
        );
        assert_eq!(
            whole, halves,
            "{label} renders differently either side of a tile seam"
        );
    }

    fn render_leisure(
        xzbbox: &XZBBox,
        way: &ProcessedWay,
        min_x: i32,
        max_x: i32,
    ) -> Vec<(i32, i32, i32, u16)> {
        let bridges = empty_bridge_surface(xzbbox);
        let footprints = BuildingFootprintBitmap::new_empty();
        let mut editor = test_editor(xzbbox);
        generate_leisure(
            &mut editor,
            way,
            &seam_test_args(),
            &FloodFillCache::new(),
            &footprints,
            &bridges,
        );
        snapshot(&editor, min_x, max_x)
    }

    /// Scatter is addressed by absolute position, so splitting an area at a
    /// tile seam must not shift a single block.
    #[test]
    fn scatter_is_identical_across_a_tile_seam() {
        let whole_bbox = XZBBox::rect_from_min_max(0, 0, AREA_MAX_X, AREA_MAX_Z).unwrap();
        let left_bbox = XZBBox::rect_from_min_max(0, 0, HALF - 1 + MARGIN, AREA_MAX_Z).unwrap();
        let right_bbox =
            XZBBox::rect_from_min_max(HALF - MARGIN, 0, AREA_MAX_X, AREA_MAX_Z).unwrap();

        for leisure_type in ["park", "garden", "nature_reserve"] {
            let way = rect_way(
                8001,
                0,
                0,
                AREA_MAX_X,
                AREA_MAX_Z,
                &[("leisure", leisure_type)],
            );

            let whole = render_leisure(&whole_bbox, &way, 0, AREA_MAX_X);
            let mut halves = render_leisure(&left_bbox, &way, 0, HALF - 1);
            halves.extend(render_leisure(&right_bbox, &way, HALF, AREA_MAX_X));

            assert_seam_free(&format!("leisure={leisure_type}"), whole, halves);
        }
    }
}
