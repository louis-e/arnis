use super::rectangle::XZBBoxRect;
use crate::coordinate_system::cartesian::{XZPoint, XZVector};
use geo::{Area, BoundingRect, Intersects, LineString, Point, Polygon};
use ndarray::Array2;
use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};

/// An underlying shape polygon of XZBBox enum.
#[derive(Clone, Debug)]
pub struct XZBBoxPoly {
    /// XZPoint list of the polygon
    points: Vec<XZPoint>,

    /// Mask on its bounding rect
    mask: Array2<bool>,

    /// Curcumscribed rect
    rect: XZBBoxRect,

    /// Total blocks covered by polygon (block center on the edge is valid)
    total_valid_blocks: u64,
}

impl XZBBoxPoly {
    pub fn new(points: Vec<XZPoint>) -> Result<Self, String> {
        let len_ge_3 = points.len() >= 3;
        if !len_ge_3 {
            return Err(format!(
                "Points too few to construct a polygon, minimal 3 but has only {}",
                points.len()
            ));
        }

        let linestring: LineString = points.iter().map(|p| (p.x as f64, p.z as f64)).collect();
        let geopolygon = Polygon::new(linestring.clone(), vec![]);

        // find any intersections, use geopolygon
        let segments: Vec<_> = geopolygon.exterior().lines().collect();
        let total_segments = segments.len();
        let no_intersect = !segments.iter().enumerate().any(|(i, seg1)| {
            segments
                .iter()
                .skip(i + 2) // skip self-self and self-neighbor (they must intersect)
                .take(total_segments - 3) // avoid start-last (they form a loop so must intersect)
                .any(|seg2| seg1.intersects(seg2))
        });
        if !no_intersect {
            return Err("Polygon self intersect".to_string());
        }

        let area_nonzero = geopolygon.unsigned_area() > 0.0;
        if !area_nonzero {
            return Err("Polygon degenerate to zero area".to_string());
        }

        let bbox = geopolygon
            .bounding_rect()
            .ok_or("Failed to obtain polygon bounding rect from geo".to_string())?;
        let rect = XZBBoxRect::new(
            XZPoint::new(bbox.min().x as i32, bbox.min().y as i32),
            XZPoint::new(bbox.max().x as i32, bbox.max().y as i32),
        )
        .map_err(|e| format!("Polygon bounding box error:\n{}", e))?;

        let mut mask = Array2::from_elem(
            (
                rect.total_blocks_x() as usize,
                rect.total_blocks_z() as usize,
            ),
            false,
        );
        for i in 0..rect.total_blocks_x() as usize {
            for k in 0..rect.total_blocks_z() as usize {
                let point = Point::new(
                    (i as i32 + rect.min().x) as f64,
                    (k as i32 + rect.min().z) as f64,
                );
                mask[(i, k)] = geopolygon.intersects(&point);
            }
        }

        let total_valid_blocks: u64 = mask.mapv(|v| v as u64).sum();
        let has_valid_blocks = total_valid_blocks > 0;
        if !has_valid_blocks {
            return Err(format!(
                "Expected at least one valid blocks, but has {}",
                total_valid_blocks
            ));
        }

        Ok(Self {
            points,
            mask,
            rect,
            total_valid_blocks,
        })
    }

    pub fn points(&self) -> &Vec<XZPoint> {
        &self.points
    }

    pub fn bounding_rect(&self) -> &XZBBoxRect {
        &self.rect
    }

    /// Check whether an XZPoint is covered
    pub fn contains(&self, xzpoint: &XZPoint) -> bool {
        if !self.rect.contains(xzpoint) {
            return false;
        }

        let ikpoint = xzpoint.to_relative(self.rect.min());
        self.mask[(ikpoint.x as usize, ikpoint.z as usize)]
    }
}

impl fmt::Display for XZBBoxPoly {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Poly()")
    }
}

// below are associated +- operators
impl Add<XZVector> for XZBBoxPoly {
    type Output = XZBBoxPoly;

    fn add(self, other: XZVector) -> Self {
        Self {
            points: self.points.iter().map(|p| *p + other).collect(),
            mask: self.mask.clone(),
            rect: self.rect + other,
            total_valid_blocks: self.total_valid_blocks,
        }
    }
}

impl AddAssign<XZVector> for XZBBoxPoly {
    fn add_assign(&mut self, other: XZVector) {
        self.points.iter_mut().for_each(|p| *p += other);
        self.rect += other;
    }
}

impl Sub<XZVector> for XZBBoxPoly {
    type Output = XZBBoxPoly;

    fn sub(self, other: XZVector) -> Self {
        Self {
            points: self.points.iter().map(|p| *p - other).collect(),
            mask: self.mask.clone(),
            rect: self.rect - other,
            total_valid_blocks: self.total_valid_blocks,
        }
    }
}

impl SubAssign<XZVector> for XZBBoxPoly {
    fn sub_assign(&mut self, other: XZVector) {
        self.points.iter_mut().for_each(|p| *p -= other);
        self.rect -= other;
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use ndarray::array;

    #[test]
    fn test_construct() {
        // 0,0 -> 1,1 rect
        let points = vec![
            XZPoint::new(0, 0),
            XZPoint::new(1, 0),
            XZPoint::new(1, 1),
            XZPoint::new(0, 1),
        ];
        let obj = XZBBoxPoly::new(points);
        assert!(obj.is_ok());

        let poly = obj.unwrap();
        assert_eq!(poly.rect.min().x, 0);
        assert_eq!(poly.rect.min().z, 0);
        assert_eq!(poly.rect.max().x, 1);
        assert_eq!(poly.rect.max().z, 1);

        assert_eq!(poly.mask, Array2::from_elem((2, 2), true));
        assert_eq!(poly.total_valid_blocks, 4);

        // 0,0 -> 2,2 rect
        let points = vec![
            XZPoint::new(0, 0),
            XZPoint::new(2, 0),
            XZPoint::new(2, 2),
            XZPoint::new(0, 2),
        ];
        let obj = XZBBoxPoly::new(points);
        assert!(obj.is_ok());

        let poly = obj.unwrap();
        assert_eq!(poly.rect.min().x, 0);
        assert_eq!(poly.rect.min().z, 0);
        assert_eq!(poly.rect.max().x, 2);
        assert_eq!(poly.rect.max().z, 2);

        assert_eq!(poly.mask, Array2::from_elem((3, 3), true));
        assert_eq!(poly.total_valid_blocks, 9);

        // -10,-5 -> 7,3 rect
        let points = vec![
            XZPoint::new(-10, -5),
            XZPoint::new(7, -5),
            XZPoint::new(7, 3),
            XZPoint::new(-10, 3),
        ];
        let obj = XZBBoxPoly::new(points);
        assert!(obj.is_ok());

        let poly = obj.unwrap();
        assert_eq!(poly.rect.min().x, -10);
        assert_eq!(poly.rect.min().z, -5);
        assert_eq!(poly.rect.max().x, 7);
        assert_eq!(poly.rect.max().z, 3);

        assert_eq!(poly.mask, Array2::from_elem((18, 9), true));
        assert_eq!(poly.total_valid_blocks, 162);

        // |x| + |y| = 1 diamond
        let points = vec![
            XZPoint::new(0, -1),
            XZPoint::new(1, 0),
            XZPoint::new(0, 1),
            XZPoint::new(-1, 0),
        ];
        let obj = XZBBoxPoly::new(points);
        assert!(obj.is_ok());

        let poly = obj.unwrap();
        assert_eq!(poly.rect.min().x, -1);
        assert_eq!(poly.rect.min().z, -1);
        assert_eq!(poly.rect.max().x, 1);
        assert_eq!(poly.rect.max().z, 1);

        #[rustfmt::skip]
        assert_eq!(
            poly.mask,
            array![
                [0, 1, 0], 
                [1, 1, 1], 
                [0, 1, 0],
            ].mapv(|v| v != 0)
        );
        assert_eq!(poly.total_valid_blocks, 5);

        // triangle
        let points = vec![
            XZPoint::new(0, 1),
            XZPoint::new(-2, -1),
            XZPoint::new(2, -1),
        ];
        let obj = XZBBoxPoly::new(points);
        assert!(obj.is_ok());

        let poly = obj.unwrap();
        assert_eq!(poly.rect.min().x, -2);
        assert_eq!(poly.rect.min().z, -1);
        assert_eq!(poly.rect.max().x, 2);
        assert_eq!(poly.rect.max().z, 1);

        // edges exactly pass the center of (-1, 0) (1, 0)
        #[rustfmt::skip]
        assert_eq!(
            poly.mask,
            array![
                [1, 1, 1, 1, 1], 
                [0, 1, 1, 1, 0], 
                [0, 0, 1, 0, 0],
            ].mapv(|v| v != 0).t()
        );
        assert_eq!(poly.total_valid_blocks, 9);
    }

    #[test]
    fn test_invalid_construct() {
        // 0,0 -> 1,0 single line, not enough points
        #[rustfmt::skip]
        let points = vec![
            XZPoint::new(0, 0),
            XZPoint::new(1, 0),
        ];
        let obj = XZBBoxPoly::new(points);
        assert!(obj.is_err());
        let errmsg = obj.unwrap_err();
        assert!(errmsg.contains("too few"));

        // one single line area 0
        #[rustfmt::skip]
        let points = vec![
            XZPoint::new(0, 0), 
            XZPoint::new(1, 0), 
            XZPoint::new(1, 0)
        ];
        let obj = XZBBoxPoly::new(points);
        assert!(obj.is_err());
        let errmsg = obj.unwrap_err();
        assert!(!errmsg.contains("intersect")); // 0-length edges are not geo::intersections
        assert!(errmsg.contains("degenerate")); // but if all following edges 0-len || parallel, it's degenerate

        // 0,0 -> 2,0 -> 1,0 area 0
        #[rustfmt::skip]
        let points = vec![
            XZPoint::new(0, 0), 
            XZPoint::new(2, 0), 
            XZPoint::new(1, 0)
        ];
        let obj = XZBBoxPoly::new(points);
        assert!(obj.is_err());
        let errmsg = obj.unwrap_err();
        assert!(!errmsg.contains("intersect")); // parallel covered edges are not geo::intersections
        assert!(errmsg.contains("degenerate")); // but if all following edges 0-len || parallel, it's degenerate

        // 0,0 -> 2,0 -> 1,0 -> 1,1 self intersect but non-zero area
        let points = vec![
            XZPoint::new(0, 0),
            XZPoint::new(2, 0),
            XZPoint::new(1, 0),
            XZPoint::new(1, 1),
        ];
        let obj = XZBBoxPoly::new(points);
        assert!(obj.is_err());
        let errmsg = obj.unwrap_err();
        assert!(errmsg.contains("intersect"));
        assert!(!errmsg.contains("degenerate")); // intersect is checked before area

        // duplicate point
        let points = vec![
            XZPoint::new(0, 0),
            XZPoint::new(2, 0),
            XZPoint::new(2, 0),
            XZPoint::new(2, 1),
        ];
        let obj = XZBBoxPoly::new(points);
        assert!(obj.is_err());
        let errmsg = obj.unwrap_err();
        assert!(errmsg.contains("intersect"));
        assert!(!errmsg.contains("degenerate")); // intersect is checked before area

        // |x| + |y| = 1 diamond but wrong order
        let points = vec![
            XZPoint::new(0, -1),
            XZPoint::new(-1, 0),
            XZPoint::new(1, 0),
            XZPoint::new(0, 1),
        ];
        let obj = XZBBoxPoly::new(points);
        assert!(obj.is_err());
        let errmsg = obj.unwrap_err();
        assert!(errmsg.contains("intersect"));

        // non symmetric diamond with wrong order
        let points = vec![
            XZPoint::new(0, -1),
            XZPoint::new(-1, 0),
            XZPoint::new(2, 0),
            XZPoint::new(0, 1),
        ];
        let obj = XZBBoxPoly::new(points);
        assert!(obj.is_err());
        let errmsg = obj.unwrap_err();
        assert!(errmsg.contains("intersect"));
    }
}
