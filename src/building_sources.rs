//! The buildings a world is built from, every source together: OSM's parsed
//! elements, the heights and garden buildings the laser measured, and Overture
//! Maps. The CLI and the window both go through [`merge`], so neither can leave
//! a step out.

use crate::coordinate_system::cartesian::XZBBox;
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;
use crate::overture::{self, OvertureData};
use colored::Colorize;

/// Adds to the parsed OSM elements, in this order, each filling only what the
/// ones before left open:
///
/// 1. heights the laser measured, for buildings OSM gives none (a roof it saw
///    beats Overture's estimate);
/// 2. Overture's heights for the buildings still without;
/// 3. Overture's own buildings where OSM has none;
/// 4. the sheds, garages and pools garden detail measured where nothing mapped
///    stands.
///
/// `overture_wanted` says whether the run asked Overture for buildings, for the
/// line saying it had none here; `scale` is the world's, for the roads' width.
pub fn merge(
    parsed_elements: &mut Vec<ProcessedElement>,
    overture: OvertureData,
    ground: &Ground,
    xzbbox: &XZBBox,
    overture_wanted: bool,
    scale: f64,
) {
    let OvertureData {
        elements: overture_elements,
        hints: overture_hints,
        ..
    } = overture;

    // A roof at the highest height the survey records waits for Overture's.
    let (mut measured_heights, deferred) =
        crate::survey::heights::apply(parsed_elements, ground, xzbbox, scale);

    // Before the footprints are merged: Overture's own ways carry their tags.
    let enriched = overture_hints.apply(parsed_elements);
    measured_heights += crate::survey::heights::apply_deferred(parsed_elements, deferred);
    if measured_heights > 0 {
        println!(
            "  Measured heights for {} buildings from laser scans",
            measured_heights.to_string().bright_white().bold()
        );
    }
    if enriched > 0 {
        println!(
            "  Filled heights on {} OSM buildings from Overture Maps",
            enriched.to_string().bright_white().bold()
        );
    }

    if !overture_elements.is_empty() {
        let before_count = parsed_elements.len();
        let unique_overture = overture::deduplicate_against_osm(overture_elements, parsed_elements);
        parsed_elements.extend(unique_overture);
        let added = parsed_elements.len() - before_count;
        println!(
            "  Added {} buildings from Overture Maps",
            added.to_string().bright_white().bold()
        );
    } else if overture_wanted && enriched == 0 {
        println!("  No additional buildings from Overture Maps for this area");
    }

    // Mapped water and the land cover's: no shed or pool of the laser's stands there.
    let mapped_water =
        crate::floodfill_cache::collect_tree_keep_clear(parsed_elements, xzbbox, scale, None, 0);
    let water = |x: i32, z: i32| {
        mapped_water.as_ref().is_some_and(|m| m.contains(x, z))
            || ground.cover_class(crate::coordinate_system::cartesian::XZPoint::new(
                x - xzbbox.min_x(),
                z - xzbbox.min_z(),
            )) == crate::land_cover::LC_WATER
    };
    let (measured, on_paved) = crate::survey::elements::merge(
        crate::survey::elements::candidates(ground, xzbbox),
        parsed_elements,
        scale,
        &water,
    );
    if !measured.is_empty() {
        println!(
            "  Added {} sheds, garages and pools from garden detail",
            measured.len().to_string().bright_white().bold()
        );
        parsed_elements.extend(measured);
    }
    if on_paved > 0 {
        println!(
            "  Left out {on_paved} on roads, tracks, squares or car parks, as vehicles or stalls"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::{ProcessedNode, ProcessedWay};
    use crate::survey::{Features, SurveyData};
    use std::collections::HashMap;

    /// A square way from (x0, z0) to (x1, z1), closed.
    fn square(
        id: u64,
        tags: &[(&str, &str)],
        x0: i32,
        z0: i32,
        x1: i32,
        z1: i32,
    ) -> ProcessedElement {
        let node = |k: u64, x: i32, z: i32| ProcessedNode {
            id: id * 10 + k,
            tags: HashMap::new(),
            x,
            z,
        };
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes: vec![
                node(1, x0, z0),
                node(2, x1, z0),
                node(3, x1, z1),
                node(4, x0, z1),
                node(1, x0, z0),
            ],
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        })
    }

    /// The laser over a 60 x 60 block world: a flat roof 6 m up over a house
    /// OSM gives no height, and a garage-sized box 2.6 m tall in its garden.
    fn ground() -> Ground {
        let (w, h) = (60usize, 60usize);
        let mut cells = vec![[0u8; 5]; w * h];
        for z in 10..20 {
            for x in 10..20 {
                cells[z * w + x] = [crate::survey::classify::BUILDING, 30, 0, 0, 0];
            }
        }
        for z in 35..40 {
            for x in 35..39 {
                cells[z * w + x] = [crate::survey::classify::SHED, 13, 120, 120, 120];
            }
        }
        let features = Features {
            garden: true,
            trees: false,
            heights: true,
        };
        let mut ground = Ground::new_flat(0);
        ground.set_world_dims(w, h);
        ground.set_survey(Some(SurveyData::from_cells(cells, w, h, features)));
        ground
    }

    #[test]
    fn measured_heights_and_garden_buildings_are_both_merged() {
        let mut elements = vec![square(1, &[("building", "house")], 10, 10, 19, 19)];
        let xzbbox = XZBBox::rect_from_xz_lengths(59.0, 59.0).unwrap();
        merge(
            &mut elements,
            OvertureData::default(),
            &ground(),
            &xzbbox,
            false,
            1.0,
        );
        let tags = elements[0].tags();
        assert_eq!(tags.get("building:levels").map(String::as_str), Some("2"));
        assert_eq!(tags.get("roof:shape").map(String::as_str), Some("flat"));
        let added: Vec<_> = elements[1..]
            .iter()
            .filter_map(|e| e.tags().get("building").cloned())
            .collect();
        assert_eq!(added, vec!["garage".to_string()]);
    }

    /// The window has no headless entry to drive, so this holds it to the same
    /// step as the CLI: both call [`merge`] once, and neither runs its parts on
    /// its own.
    #[test]
    fn the_cli_and_the_window_merge_through_here() {
        for (name, src) in [
            ("main.rs", include_str!("main.rs")),
            ("gui.rs", include_str!("gui.rs")),
        ] {
            assert_eq!(
                src.matches("building_sources::merge(").count(),
                1,
                "{name} merges its buildings once"
            );
            for step in [
                "heights::apply(",
                "elements::merge(",
                "hints.apply(",
                "deduplicate_against_osm(",
            ] {
                assert!(!src.contains(step), "{name} calls {step} on its own");
            }
        }
    }
}
