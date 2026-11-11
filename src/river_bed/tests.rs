use super::*;
use crate::element_processing::building_test_support::tag_map as tags;
use crate::osm_parser::ProcessedRelation;

/// Region whose geometry is complete in every fixture.
const BB: (i32, i32, i32, i32) = (0, 199, 0, 199);

fn way(id: u64, kv: &[(&str, &str)], pts: &[(i32, i32)]) -> ProcessedElement {
    ProcessedElement::Way(ProcessedWay {
        id,
        tags: tags(kv),
        nodes: pts
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: id * 1000 + i as u64,
                tags: HashMap::new(),
                x,
                z,
            })
            .collect(),
    })
}

/// A closed rectangle, first node repeated last.
fn rect(id: u64, kv: &[(&str, &str)], x0: i32, x1: i32, z0: i32, z1: i32) -> ProcessedElement {
    let mut w = way(id, kv, &[(x0, z0), (x1, z0), (x1, z1), (x0, z1), (x0, z0)]);
    if let ProcessedElement::Way(ref mut ww) = w {
        let last = ww.nodes.len() - 1;
        ww.nodes[last].id = ww.nodes[0].id;
    }
    w
}

fn no_lc(_x: i32, _z: i32) -> bool {
    false
}

fn no_legacy(_x: i32, _z: i32) -> i32 {
    0
}

fn build_capped(
    elements: &[ProcessedElement],
    scale: f64,
    cap: Option<i32>,
    lc: &dyn Fn(i32, i32) -> bool,
    legacy: &dyn Fn(i32, i32) -> i32,
) -> RiverBedField {
    build_field(
        elements,
        &FieldInputs {
            is_lc_water: lc,
            legacy_depth: legacy,
            bb: BB,
            scale,
            channel_width_cap: cap,
        },
    )
}

fn build(
    elements: &[ProcessedElement],
    scale: f64,
    lc: &dyn Fn(i32, i32) -> bool,
    legacy: &dyn Fn(i32, i32) -> i32,
) -> RiverBedField {
    build_capped(elements, scale, None, lc, legacy)
}

/// A straight river polygon `width` blocks wide centred on x = 100.
fn straight_polygon_river(width: i32) -> Vec<ProcessedElement> {
    let x0 = 100 - width / 2;
    vec![rect(
        1,
        &[("natural", "water"), ("water", "river")],
        x0,
        x0 + width - 1,
        20,
        180,
    )]
}

#[test]
fn cross_section_is_a_smooth_symmetric_u() {
    // Centre depth off the cap table: hw 3 -> 1.2, hw 6 -> 1.9, hw 20 -> 4.5.
    for (width, want_centre) in [(6i32, 1i32), (12, 2), (40, 5)] {
        let f = build(&straight_polygon_river(width), 1.0, &no_lc, &no_legacy);
        let x0 = 100 - width / 2;
        let row: Vec<_> = (x0 - 2..=x0 + width + 1)
            .map(|x| f.depth_override(x, 100))
            .collect();
        assert_eq!(row[0], None, "w={width}: override outside the polygon");
        assert_eq!(*row.last().unwrap(), None, "w={width}: override outside");

        let inside: Vec<i32> = row.iter().flatten().copied().collect();
        assert_eq!(inside.len(), width as usize, "w={width}: mask width");
        assert_eq!((inside[0], *inside.last().unwrap()), (0, 0), "w={width}");
        for i in 1..inside.len() {
            assert!(
                (inside[i] - inside[i - 1]).abs() <= 1,
                "w={width}: {inside:?}"
            );
            if i < inside.len() / 2 {
                assert!(inside[i] >= inside[i - 1], "w={width}: {inside:?}");
            }
        }
        let mirrored: Vec<i32> = inside.iter().rev().copied().collect();
        assert_eq!(inside, mirrored, "w={width}: asymmetric");
        assert_eq!(*inside.iter().max().unwrap(), want_centre, "{inside:?}");
    }
}

#[test]
fn off_and_lake_only_renders_build_nothing() {
    assert!(RiverBedField::default().depth_override(100, 100).is_none());
    let els = vec![
        rect(1, &[("natural", "water")], 40, 160, 40, 160),
        rect(2, &[("natural", "water"), ("water", "oxbow")], 0, 30, 0, 30),
        way(3, &[("highway", "residential")], &[(0, 0), (199, 199)]),
    ];
    assert_eq!(build(&els, 1.0, &no_lc, &no_legacy).override_count(), 0);
}

#[test]
fn a_centreline_through_a_mapped_lake_is_clipped_out() {
    let els = vec![
        way(1, &[("waterway", "river")], &[(20, 100), (180, 100)]),
        rect(2, &[("natural", "water")], 80, 120, 70, 130),
    ];
    let f = build(&els, 1.0, &no_lc, &no_legacy);
    for x in 82..=118 {
        for z in 72..=128 {
            assert_eq!(f.depth_override(x, z), None, "lake column ({x},{z})");
        }
    }
    assert!(f.depth_override(40, 100).is_some(), "upstream lost its bed");
    assert!(
        f.depth_override(160, 100).is_some(),
        "downstream lost its bed"
    );
}

#[test]
fn a_centreline_inside_wide_land_cover_water_is_suppressed() {
    // An untagged river is 8 wide (hw 4); 20 blocks from land is no 8-wide river.
    let els = vec![way(1, &[("waterway", "river")], &[(20, 100), (180, 100)])];
    let wide = |_x: i32, z: i32| (z - 100).abs() <= 20;
    assert_eq!(build(&els, 1.0, &wide, &no_legacy).override_count(), 0);

    let narrow = |_x: i32, z: i32| (z - 100).abs() <= 5;
    let g = build(&els, 1.0, &narrow, &no_legacy);
    assert!(
        g.depth_override(100, 100).is_some(),
        "matching water lost its bed"
    );

    // A mapped river polygon is evidence of its own and is never suppressed.
    let poly = vec![rect(
        1,
        &[("natural", "water"), ("water", "river")],
        20,
        180,
        90,
        110,
    )];
    let very_wide = |_x: i32, z: i32| (z - 100).abs() <= 40;
    assert!(build(&poly, 1.0, &very_wide, &no_legacy)
        .depth_override(100, 100)
        .is_some());
}

#[test]
fn riverbank_ways_and_relations_are_rivers() {
    let els = vec![rect(1, &[("waterway", "riverbank")], 80, 119, 20, 180)];
    let f = build(&els, 1.0, &no_lc, &no_legacy);
    let row: Vec<i32> = (80..=119)
        .filter_map(|x| f.depth_override(x, 100))
        .collect();
    assert_eq!(row.len(), 40);
    assert_eq!(*row.iter().max().unwrap(), 5, "{row:?}");

    let ring = match &rect(2, &[], 80, 119, 20, 180) {
        ProcessedElement::Way(w) => std::sync::Arc::new(w.clone()),
        _ => unreachable!(),
    };
    let rel = ProcessedElement::Relation(ProcessedRelation {
        id: 3,
        tags: tags(&[("natural", "water"), ("water", "river")]),
        members: vec![crate::osm_parser::ProcessedMember {
            role: ProcessedMemberRole::Outer,
            way: ring,
        }],
    });
    let g = build(&[rel], 1.0, &no_lc, &no_legacy);
    assert_eq!(g.depth_override(100, 100), f.depth_override(100, 100));
}

#[test]
fn ditches_drains_and_culverts_stay_out() {
    for kind in ["ditch", "drain"] {
        let els = vec![way(1, &[("waterway", kind)], &[(20, 100), (180, 100)])];
        assert_eq!(build(&els, 1.0, &no_lc, &no_legacy).override_count(), 0);
    }
    let culvert = vec![way(
        1,
        &[("waterway", "river"), ("tunnel", "culvert")],
        &[(20, 100), (180, 100)],
    )];
    assert_eq!(build(&culvert, 1.0, &no_lc, &no_legacy).override_count(), 0);
}

/// A 20-wide river polygon running into a lake that starts at z = 119.
fn lake_confluence() -> Vec<ProcessedElement> {
    vec![
        rect(
            1,
            &[("natural", "water"), ("water", "river")],
            90,
            109,
            10,
            120,
        ),
        rect(2, &[("natural", "water")], 40, 160, 119, 190),
    ]
}

#[test]
fn a_river_entering_a_lake_arrives_at_the_lake_bed() {
    let legacy = |_x: i32, z: i32| if z >= 120 { 3 } else { 2 };
    let f = build(&lake_confluence(), 1.0, &no_lc, &legacy);
    for x in 42..=158 {
        for z in 122..=188 {
            assert_eq!(f.depth_override(x, z), None, "lake column ({x},{z})");
        }
    }
    // The mouth column keeps its own legacy depth, so there is no step at the join.
    for x in 92..=107 {
        assert_eq!(f.depth_override(x, 119), Some(legacy(x, 119)), "x={x}");
    }
    // Past the band (at most 32 blocks) the legacy field no longer matters.
    let hot = build(&lake_confluence(), 1.0, &no_lc, &|_x, _z| 6);
    let cold = build(&lake_confluence(), 1.0, &no_lc, &|_x, _z| 0);
    assert!((100..=119).any(|z| hot.depth_override(100, z) != cold.depth_override(100, z)));
    for z in 10..=(119 - 32) {
        assert_eq!(
            hot.depth_override(100, z),
            cold.depth_override(100, z),
            "z={z}"
        );
    }
    assert!(f.depth_override(100, 40).unwrap() >= 2);
}

#[test]
fn depth_stays_capped_at_every_scale() {
    for scale in [0.25, 0.5, 1.0, 2.0] {
        let line = vec![way(
            1,
            &[("waterway", "river"), ("width", "120")],
            &[(20, 100), (180, 100)],
        )];
        let f = build(&line, scale, &no_lc, &no_legacy);
        assert!(f.override_count() > 0, "scale {scale}");
        assert!(f.max_override_depth() <= MAX_WATER_DEPTH, "scale {scale}");
        let p = build(&straight_polygon_river(90), scale, &no_lc, &no_legacy);
        assert!(p.max_override_depth() <= MAX_WATER_DEPTH, "scale {scale}");
    }
    assert!(river_profile_depth(3.0, 0.0, 0.2).is_finite());
}

#[test]
fn a_width_cap_narrows_the_line_mask() {
    let line = vec![way(1, &[("waterway", "river")], &[(20, 100), (180, 100)])];
    let wide = build(&line, 0.25, &no_lc, &no_legacy).override_count();
    let capped = build_capped(&line, 0.25, Some(1), &no_lc, &no_legacy).override_count();
    // Untagged river: 8 wide -> ribbon of 2 * (4 + 1) + 1 = 11; capped to 1 -> 3.
    assert_eq!(wide / capped, 11 / 3, "{wide} vs {capped}");
}

#[test]
fn the_profile_is_flat_at_both_ends_and_never_steps_more_than_a_block() {
    let hw = 30.0;
    let p = |d: f64| river_profile_depth(d, hw, 1.0);
    let mid = p(hw * 0.55) - p(hw * 0.45);
    assert!(p(hw * 0.1) - p(0.0) < mid);
    assert!(p(hw) - p(hw * 0.9) < mid);
    let worst = (0..120)
        .map(|i| p(f64::from(i) * 0.25 + 1.0) - p(f64::from(i) * 0.25))
        .fold(0.0, f64::max);
    assert!(worst <= 1.0, "peak bank slope {worst}");
    assert!((river_depth_cap_for_hw(10.0) - 2.8).abs() < 0.1);
    assert!((river_depth_cap_for_hw(80.0) - 6.0).abs() < 1e-9);
}

#[test]
fn element_order_does_not_change_the_field() {
    let a = way(1, &[("waterway", "stream")], &[(20, 100), (180, 100)]);
    let b = way(
        2,
        &[("waterway", "river"), ("width", "40")],
        &[(20, 100), (180, 110)],
    );
    let f1 = build(&[a.clone(), b.clone()], 1.0, &no_lc, &no_legacy);
    let f2 = build(&[b, a], 1.0, &no_lc, &no_legacy);
    for x in (0..200).step_by(3) {
        for z in 60..=150 {
            assert_eq!(
                f1.depth_override(x, z),
                f2.depth_override(x, z),
                "({x},{z})"
            );
        }
    }
}

#[test]
fn the_field_does_not_depend_on_where_the_geometry_region_ends() {
    // A seam at x = 100: each side sees the river clipped at its own region edge, as One
    // World pieces do. Columns well inside both regions must agree with the whole run.
    let river = |x0: i32, x1: i32| {
        vec![rect(
            1,
            &[("natural", "water"), ("water", "river")],
            x0,
            x1,
            90,
            110,
        )]
    };
    let whole = build(&river(0, 199), 1.0, &no_lc, &no_legacy);
    let left = build_field(
        &river(0, 140),
        &FieldInputs {
            is_lc_water: &no_lc,
            legacy_depth: &no_legacy,
            bb: (0, 140, 0, 199),
            scale: 1.0,
            channel_width_cap: None,
        },
    );
    for x in 60..=110 {
        for z in 85..=115 {
            assert_eq!(
                whole.depth_override(x, z),
                left.depth_override(x, z),
                "({x},{z})"
            );
        }
    }
}

#[test]
fn river_columns_get_no_dunes() {
    use crate::block_definitions::WATER;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::floodfill_cache::RoadMaskBitmap;
    use crate::world_editor::WorldEditor;

    let bbox = XZBBox::rect_from_min_max(0, 0, 199, 199).unwrap();
    let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
    let mask = RoadMaskBitmap::new(&bbox);
    // Count columns with something other than water or plants right above the bed.
    let bumps = |river: bool| {
        let mut editor =
            WorldEditor::new(std::path::PathBuf::from("/dev/null/unused"), &bbox, llbbox);
        let mut bwf = BigWaterField::empty();
        if river {
            bwf.set_river_bed(build(&straight_polygon_river(80), 1.0, &no_lc, &no_legacy));
        }
        let mut n = 0;
        for x in 80..120 {
            for z in 40..160 {
                crate::water_depth::carve_water_column(&mut editor, x, z, 20, 6, &mask, &bwf);
                let above = editor.get_block_absolute(x, 20 - 6, z);
                let plant = |b: crate::block_definitions::Block| {
                    b.name().contains("sea") || b.name().contains("kelp")
                };
                if above.is_some_and(|b| b != WATER && !plant(b)) {
                    n += 1;
                }
            }
        }
        n
    };
    assert!(
        bumps(false) > 0,
        "no dunes without the field: test is vacuous"
    );
    assert_eq!(bumps(true), 0);
}
