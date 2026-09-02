//! Jet bridges from OSM `aeroway=jet_bridge`. The model is L shaped, so its terminal to cab
//! diagonal follows the way. Most such ways also carry highway and bridge tags, which is why
//! data_processing, the road mask and is_bridge_way all skip them.

use std::sync::OnceLock;

use super::schematic::{place_structure_yaw, ColumnSchematic};
use crate::floodfill_cache::CoordinateBitmap;
use crate::osm_parser::{ProcessedNode, ProcessedWay};
use crate::world_editor::WorldEditor;

static BYTES: &[u8] = include_bytes!("../../assets/structures/jetbridge.schem");

/// Model (x, z) of the terminal aperture, the mouth that butts the building wall.
const TERMINAL_ATTACH: (f64, f64) = (2.0, 0.0);
/// Model (x, z) of the cab mouth, at the aircraft end.
const CAB_MOUTH: (f64, f64) = (11.0, 14.0);
/// Shortest way the model fits on, in blocks because the model does not scale.
const MIN_LENGTH_BLOCKS: f64 = 12.0;
/// Half-side of the square probed around each endpoint when neither sits on a building outline.
const FOOTPRINT_PROBE: i32 = 4;

fn jetbridge() -> Option<&'static ColumnSchematic> {
    static CELL: OnceLock<Option<ColumnSchematic>> = OnceLock::new();
    CELL.get_or_init(|| match ColumnSchematic::load(BYTES) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("jetbridge load failed: {e}");
            None
        }
    })
    .as_ref()
}

/// Yaw that swings model vector `model` onto world direction `world`.
fn yaw_for(model: (f64, f64), world: (f64, f64)) -> f64 {
    world.1.atan2(world.0).to_degrees() - model.1.atan2(model.0).to_degrees()
}

/// True when this module owns the way. Areas, closed ways and indoor ways fall through.
pub fn claims(way: &ProcessedWay) -> bool {
    if way.tags.get("aeroway").map(String::as_str) != Some("jet_bridge") {
        return false;
    }
    if way.tags.get("area").is_some_and(|v| v == "yes")
        || way.tags.get("indoor").is_some_and(|v| v == "yes")
    {
        return false;
    }
    match (way.nodes.first(), way.nodes.last()) {
        (Some(a), Some(b)) => a.id != b.id,
        _ => false,
    }
}

/// Which endpoint meets the terminal. Node order is a coin flip, the building footprint is not.
fn terminal_end<'a>(
    a: &'a ProcessedNode,
    b: &'a ProcessedNode,
    building_footprints: &CoordinateBitmap,
) -> (&'a ProcessedNode, &'a ProcessedNode) {
    let (on_a, on_b) = (
        building_footprints.contains(a.x, a.z),
        building_footprints.contains(b.x, b.z),
    );
    if on_a != on_b {
        return if on_a { (a, b) } else { (b, a) };
    }
    let nearby = |n: &ProcessedNode| {
        let mut hits = 0usize;
        for dz in -FOOTPRINT_PROBE..=FOOTPRINT_PROBE {
            for dx in -FOOTPRINT_PROBE..=FOOTPRINT_PROBE {
                if building_footprints.contains(n.x + dx, n.z + dz) {
                    hits += 1;
                }
            }
        }
        hits
    };
    if nearby(b) > nearby(a) {
        (b, a)
    } else {
        (a, b)
    }
}

/// Both sides are world blocks, see MIN_LENGTH_BLOCKS for why this is not metres.
fn is_long_enough(dir: (f64, f64)) -> bool {
    dir.0.hypot(dir.1) >= MIN_LENGTH_BLOCKS
}

/// World anchor, the model centre, that lands model point `attach` on `(tx, tz)` under `yaw`.
fn anchor_for(
    schem: &ColumnSchematic,
    attach: (f64, f64),
    tx: i32,
    tz: i32,
    yaw: f64,
) -> (i32, i32) {
    let (u, v) = (
        f64::from(schem.width - 1) / 2.0 - attach.0,
        f64::from(schem.length - 1) / 2.0 - attach.1,
    );
    let (s, c) = yaw.to_radians().sin_cos();
    (
        (f64::from(tx) + u * c - v * s).round() as i32,
        (f64::from(tz) + u * s + v * c).round() as i32,
    )
}

/// Stamps one jet bridge per way, anchored on its terminal end. Pure in the node coordinates.
pub fn generate_jet_bridge(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    building_footprints: &CoordinateBitmap,
) {
    if !editor.place_schematics() || !claims(way) {
        return;
    }
    let Some(schem) = jetbridge() else {
        return;
    };
    let (Some(first), Some(last)) = (way.nodes.first(), way.nodes.last()) else {
        return;
    };
    let (terminal, apron) = terminal_end(first, last, building_footprints);
    // These ways are near straight, so the endpoint vector is the bearing.
    let dir = (
        f64::from(apron.x - terminal.x),
        f64::from(apron.z - terminal.z),
    );
    if !is_long_enough(dir) {
        return;
    }

    let model_axis = (
        CAB_MOUTH.0 - TERMINAL_ATTACH.0,
        CAB_MOUTH.1 - TERMINAL_ATTACH.1,
    );
    let yaw = yaw_for(model_axis, dir);
    let (base_x, base_z) = anchor_for(schem, TERMINAL_ATTACH, terminal.x, terminal.z, yaw);
    if editor.is_lc_water(base_x, base_z) {
        return;
    }
    // Model y=0 is the apron. The walkway climbs to the terminal aperture inside the model.
    let base_y = editor.get_absolute_y(base_x, 1, base_z);
    place_structure_yaw(editor, schem, base_x, base_z, base_y, yaw, 0.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap as StdMap;

    fn node(id: u64, x: i32, z: i32) -> ProcessedNode {
        ProcessedNode {
            id,
            tags: StdMap::new(),
            x,
            z,
        }
    }

    fn bridge_way(nodes: Vec<ProcessedNode>, extra: &[(&str, &str)]) -> ProcessedWay {
        let mut tags = StdMap::new();
        tags.insert("aeroway".to_string(), "jet_bridge".to_string());
        tags.insert("highway".to_string(), "corridor".to_string());
        for (k, v) in extra {
            tags.insert((*k).to_string(), (*v).to_string());
        }
        ProcessedWay { id: 1, nodes, tags }
    }

    #[test]
    fn jetbridge_asset_parses() {
        let schem = jetbridge().expect("jetbridge parses");
        assert_eq!((schem.width, schem.length), (12, 18));
        assert_eq!(schem.height(), 11);
    }

    #[test]
    fn accepts_a_plain_jet_bridge_and_rejects_the_rest() {
        assert!(claims(&bridge_way(
            vec![node(1, 0, 0), node(2, 30, 0)],
            &[]
        )));
        assert!(!claims(&bridge_way(
            vec![node(1, 0, 0), node(2, 30, 0)],
            &[("indoor", "yes")]
        )));
        assert!(!claims(&bridge_way(
            vec![node(1, 0, 0), node(2, 30, 0)],
            &[("area", "yes")]
        )));
        // A closed way is an area representation, not a walkway.
        assert!(!claims(&bridge_way(
            vec![node(1, 0, 0), node(2, 30, 0), node(1, 0, 0)],
            &[]
        )));
        let mut not_a_bridge = bridge_way(vec![node(1, 0, 0), node(2, 30, 0)], &[]);
        not_a_bridge
            .tags
            .insert("aeroway".to_string(), "taxiway".to_string());
        assert!(!claims(&not_a_bridge));
    }

    #[test]
    fn terminal_end_comes_from_the_footprint_not_node_order() {
        use crate::coordinate_system::cartesian::XZBBox;

        let (a, b) = (node(1, 0, 0), node(2, 40, 0));
        // Nothing mapped: the first node wins by convention.
        let empty = CoordinateBitmap::new_empty();
        assert_eq!(terminal_end(&a, &b, &empty).0.id, 1);

        // Second node is the building one, so it wins even though it is last.
        let xz = XZBBox::rect_from_xz_lengths(64.0, 64.0).unwrap();
        let mut fp = CoordinateBitmap::new(&xz);
        fp.set(40, 0);
        assert_eq!(terminal_end(&a, &b, &fp).0.id, 2);
        assert_eq!(terminal_end(&b, &a, &fp).0.id, 2);
    }

    /// The gate protects a fixed size model, so it stays in blocks.
    #[test]
    fn length_gate_is_measured_against_the_models_own_span() {
        let span = (CAB_MOUTH.0 - TERMINAL_ATTACH.0).hypot(CAB_MOUTH.1 - TERMINAL_ATTACH.1);
        assert!(
            MIN_LENGTH_BLOCKS < span && MIN_LENGTH_BLOCKS > span * 0.5,
            "the gate should sit just under the model's {span}-block span"
        );
        assert!(is_long_enough((30.0, 0.0)));
        assert!(is_long_enough((0.0, -20.0)));
        // A 30 m bridge at --scale 0.3 is 9 blocks of way: too short for a 16-block model.
        assert!(!is_long_enough((9.0, 0.0)));
        assert!(!is_long_enough((3.0, 4.0)));
    }

    #[test]
    fn yaw_puts_the_cab_at_the_apron_end() {
        let schem = jetbridge().expect("jetbridge parses");
        let axis = (
            CAB_MOUTH.0 - TERMINAL_ATTACH.0,
            CAB_MOUTH.1 - TERMINAL_ATTACH.1,
        );
        // The aperture lands on the terminal node and the cab ends up downrange of it.
        for dir in [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.7, -0.7)] {
            let yaw = yaw_for(axis, dir);
            let (bx, bz) = anchor_for(schem, TERMINAL_ATTACH, 100, 200, yaw);
            let (s, c) = yaw.to_radians().sin_cos();
            let place = |m: (f64, f64)| {
                let (u, v) = (
                    m.0 - f64::from(schem.width - 1) / 2.0,
                    m.1 - f64::from(schem.length - 1) / 2.0,
                );
                (f64::from(bx) + u * c - v * s, f64::from(bz) + u * s + v * c)
            };
            let (tx, tz) = place(TERMINAL_ATTACH);
            assert!(
                (tx - 100.0).abs() < 1.0 && (tz - 200.0).abs() < 1.0,
                "terminal aperture drifted to ({tx}, {tz}) for dir {dir:?}"
            );
            // The cab must lie downrange of the terminal along `dir`, never behind it.
            let (cx, cz) = place(CAB_MOUTH);
            let along = (cx - 100.0) * dir.0 + (cz - 200.0) * dir.1;
            assert!(along > 0.0, "cab ended up behind the terminal for {dir:?}");
        }
    }
}
