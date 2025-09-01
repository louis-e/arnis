use super::angle_rotator::AngleRotator;
use super::Operator;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint, XZVector};
use crate::osm_parser::ProcessedElement;

/// Create a rotate operator (rotator) from json
pub fn rotator_from_json(config: &serde_json::Value) -> Result<Box<dyn Operator>, String> {
    let type_str = config
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or("Expected a string field 'type' in an rotator dict:\n{}".to_string())?;

    let rotator_config = config
        .get("config")
        .ok_or("Expected a dict field 'config' in an rotator dict")?;

    let rotator_result: Result<Box<dyn Operator>, String> = match type_str {
        "angle" => {
            let upper_result: Result<Box<AngleRotator>, _> =
                serde_json::from_value(rotator_config.clone())
                    .map(Box::new)
                    .map_err(|e| e.to_string());
            upper_result.map(|o| o as Box<dyn Operator>)
        }
        _ => Err(format!("Unrecognized rotator type '{type_str}'")),
    };

    rotator_result.map_err(|e| format!("Rotator config format error:\n{e}"))
}

fn rotate_vector(vector: XZVector, deg: f64) -> XZVector {
    let rad = deg.to_radians();
    let fdx = vector.dx as f64;
    let fdz = vector.dz as f64;
    XZVector {
        dx: (fdx * rad.cos() + fdz * rad.sin()) as i32,
        dz: (fdx * -rad.sin() + fdz * rad.cos()) as i32,
    }
}

fn rotate_point(point: XZPoint, center: XZPoint, deg: f64) -> XZPoint {
    center + rotate_vector(point - center, deg)
}

/// Rotate elements and bounding box by a degree (axis y, passing center)
pub fn rotate_by_angle(
    center: XZPoint,
    deg: f64,
    elements: &mut Vec<ProcessedElement>,
    xzbbox: &mut XZBBox,
) {
    match xzbbox {
        XZBBox::Rect(r) => {
            let points = vec![
                rotate_point(XZPoint::new(r.min().x, r.min().z), center, deg),
                rotate_point(XZPoint::new(r.max().x, r.min().z), center, deg),
                rotate_point(XZPoint::new(r.max().x, r.max().z), center, deg),
                rotate_point(XZPoint::new(r.min().x, r.max().z), center, deg),
            ];
            *xzbbox = XZBBox::poly_from_xz_list(points).unwrap();
        }
        XZBBox::Poly(p) => {
            let points = p
                .points()
                .iter()
                .map(|p| rotate_point(*p, center, deg))
                .collect();
            *xzbbox = XZBBox::poly_from_xz_list(points).unwrap();
        }
    }

    for element in elements {
        match element {
            ProcessedElement::Node(n) => {
                let newpoint = rotate_point(XZPoint::new(n.x, n.z), center, deg);
                n.x = newpoint.x;
                n.z = newpoint.z;
            }
            ProcessedElement::Way(w) => {
                for n in &mut w.nodes {
                    let newpoint = rotate_point(XZPoint::new(n.x, n.z), center, deg);
                    n.x = newpoint.x;
                    n.z = newpoint.z;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZVector;
    use crate::test_utilities::generate_default_example;

    #[test]
    fn test_rotate_vector() {
        let a = XZVector { dx: 10, dz: 0 }; // east
        let b = rotate_vector(a, 90.0);
        assert_eq!(b, XZVector { dx: 0, dz: -10 }); // north

        let a = XZVector { dx: 10, dz: 0 }; //east
        let b = rotate_vector(a, 45.0);
        assert_eq!(b, XZVector { dx: 7, dz: -7 }); // ne

        let a = XZVector { dx: 10, dz: -10 }; // ne
        let b = rotate_vector(a, 90.0);
        assert_eq!(b, XZVector { dx: -10, dz: -10 }); // nw

        let a = XZVector { dx: 10, dz: -10 }; // ne
        let b = rotate_vector(a, 45.0);
        assert_eq!(b, XZVector { dx: 0, dz: -14 }); // n
    }

    // this ensures rotate_by_angle function is correct
    #[test]
    fn test_rotate_by_angle() {
        let center = XZPoint::new(100, 200);
        let deg = 30.0;

        let (xzbbox1, elements1) = generate_default_example();

        let mut xzbbox2 = xzbbox1.clone();
        let mut elements2 = elements1.clone();

        rotate_by_angle(center, deg, &mut elements2, &mut xzbbox2);

        // 1. Elem type should not change
        // 2. For node,
        //      2.1 id and tags should not change
        //      2.2 x, z should be rotated as required
        // 3. For way,
        //      3.1 id and tags should not change
        //      3.2 For every node included, satisfies (2)
        // 4. For relation, everything is unchanged
        for (original, rotated) in elements1.iter().zip(elements2.iter()) {
            match (original, rotated) {
                (ProcessedElement::Node(a), ProcessedElement::Node(b)) => {
                    let newpoint = rotate_point(XZPoint::new(a.x, a.z), center, deg);
                    assert_eq!(a.id, b.id);
                    assert_eq!(a.tags, b.tags);
                    assert_eq!(b.x, newpoint.x);
                    assert_eq!(b.z, newpoint.z);
                }
                (ProcessedElement::Way(a), ProcessedElement::Way(b)) => {
                    assert_eq!(a.id, b.id);
                    assert_eq!(a.tags, b.tags);
                    for (nodea, nodeb) in a.nodes.iter().zip(b.nodes.iter()) {
                        let newpoint = rotate_point(XZPoint::new(nodea.x, nodea.z), center, deg);
                        assert_eq!(nodea.id, nodeb.id);
                        assert_eq!(nodea.tags, nodeb.tags);
                        assert_eq!(nodeb.x, newpoint.x);
                        assert_eq!(nodeb.z, newpoint.z);
                    }
                }
                (ProcessedElement::Relation(a), ProcessedElement::Relation(b)) => {
                    assert_eq!(a, b);
                }
                _ => {
                    panic!(
                        "Element type changed: original {} to {}",
                        original.kind(),
                        rotated.kind()
                    );
                }
            }
        }
    }
}
