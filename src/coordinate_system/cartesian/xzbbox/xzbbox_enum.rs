use super::rectangle::XZBBoxRect;
use super::shape::{XZBBoxShape, XZPolygon};
use crate::coordinate_system::cartesian::{XZPoint, XZVector};
use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};

/// Bounding Box in minecraft XZ space with varied shapes.
#[derive(Clone, Debug)]
pub enum XZBBox {
    Rect(XZBBoxRect),
    /// The columns of a drawn outline inside its rectangle.
    Shape(XZBBoxShape),
}

impl XZBBox {
    /// Construct rectangle shape bbox from the x and z lengths of the world, originated at (0, 0)
    pub fn rect_from_xz_lengths(length_x: f64, length_z: f64) -> Result<Self, String> {
        let lenx_ge_0 = length_x >= 0.0;
        let lenz_ge_0 = length_z >= 0.0;
        let lenx_overflow = length_x > i32::MAX as f64;
        let lenz_overflow = length_z > i32::MAX as f64;

        if !lenx_ge_0 {
            return Err(format!(
                "Invalid XZBBox::Rect from xz lengths: length x should >=0 , but encountered {length_x}"
            ));
        }

        if !lenz_ge_0 {
            return Err(format!(
                "Invalid XZBBox::Rect from xz lengths: length z should >=0 , but encountered {length_x}"
            ));
        }

        if lenx_overflow {
            return Err(format!(
                "Invalid XZBBox::Rect from xz lengths: length x too large for i32: {length_x}"
            ));
        }

        if lenz_overflow {
            return Err(format!(
                "Invalid XZBBox::Rect from xz lengths: length z too large for i32: {length_z}"
            ));
        }

        Ok(Self::Rect(XZBBoxRect::new(
            XZPoint { x: 0, z: 0 },
            XZPoint {
                x: length_x as i32,
                z: length_z as i32,
            },
        )?))
    }

    /// Construct rectangle shape bbox from explicit min and max coordinates.
    /// Used by projections that don't originate at (0, 0).
    pub fn rect_from_min_max(
        min_x: i32,
        min_z: i32,
        max_x: i32,
        max_z: i32,
    ) -> Result<Self, String> {
        Ok(Self::Rect(XZBBoxRect::new(
            XZPoint { x: min_x, z: min_z },
            XZPoint { x: max_x, z: max_z },
        )?))
    }

    /// Check whether an XZPoint is covered
    #[inline]
    pub fn contains(&self, xzpoint: &XZPoint) -> bool {
        match self {
            Self::Rect(r) => r.contains(xzpoint),
            Self::Shape(s) => s.contains(xzpoint),
        }
    }

    /// Return the circumscribed rectangle of the current XZBBox shape
    #[inline]
    pub fn bounding_rect(&self) -> XZBBoxRect {
        match self {
            Self::Rect(r) => *r,
            Self::Shape(s) => s.rect(),
        }
    }

    /// Keeps only the columns of this rectangle that lie in the outline.
    pub fn with_outline(self, polygons: Vec<XZPolygon>) -> Result<Self, String> {
        Ok(Self::Shape(XZBBoxShape::new(
            self.bounding_rect(),
            polygons,
        )?))
    }

    /// The drawn outline, when there is one.
    pub fn outline(&self) -> Option<&XZBBoxShape> {
        match self {
            Self::Rect(_) => None,
            Self::Shape(s) => Some(s),
        }
    }

    /// This area seen through an inclusive rectangle, which keeps an outline's mask.
    pub fn crop(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> Result<Self, String> {
        let rect = XZBBoxRect::new(XZPoint::new(min_x, min_z), XZPoint::new(max_x, max_z))?;
        Ok(match self {
            Self::Rect(_) => Self::Rect(rect),
            Self::Shape(s) => Self::Shape(s.crop(rect)),
        })
    }

    /// Whether every column of the inclusive rectangle is covered.
    pub fn covers_rect(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> bool {
        match self {
            Self::Rect(r) => {
                r.contains(&XZPoint::new(min_x, min_z)) && r.contains(&XZPoint::new(max_x, max_z))
            }
            Self::Shape(s) => s.covers(min_x, min_z, max_x, max_z),
        }
    }

    /// Whether any column of the inclusive rectangle is covered.
    pub fn touches_rect(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> bool {
        match self {
            Self::Rect(r) => {
                min_x <= r.max().x && max_x >= r.min().x && min_z <= r.max().z && max_z >= r.min().z
            }
            Self::Shape(s) => s.touches(min_x, min_z, max_x, max_z),
        }
    }

    /// The middle of the area, or the column of a drawn outline nearest to it.
    pub fn center_spawn(&self) -> (i32, i32) {
        let center = XZPoint::new(
            (self.min_x() + self.max_x()) / 2,
            (self.min_z() + self.max_z()) / 2,
        );
        let p = self
            .outline()
            .and_then(|s| s.nearest_inside(center))
            .unwrap_or(center);
        (p.x, p.z)
    }

    /// Where a world starts without a spawn picked: a block in from the north-west corner.
    pub fn default_spawn(&self) -> (i32, i32) {
        match self.outline().and_then(XZBBoxShape::first_inside) {
            Some(p) => (p.x, p.z),
            None => (self.min_x() + 1, self.min_z() + 1),
        }
    }

    /// Return the min x in all covered blocks
    pub fn min_x(&self) -> i32 {
        self.bounding_rect().min().x
    }

    /// Return the max x in all covered blocks
    pub fn max_x(&self) -> i32 {
        self.bounding_rect().max().x
    }

    /// Return the min z in all covered blocks
    pub fn min_z(&self) -> i32 {
        self.bounding_rect().min().z
    }

    /// Return the max z in all covered blocks
    pub fn max_z(&self) -> i32 {
        self.bounding_rect().max().z
    }
}

impl fmt::Display for XZBBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rect(r) => write!(f, "XZBBox::{r}"),
            Self::Shape(s) => write!(f, "XZBBox::{s}"),
        }
    }
}

// below are associated +- operators
impl Add<XZVector> for XZBBox {
    type Output = XZBBox;

    fn add(self, other: XZVector) -> XZBBox {
        match self {
            Self::Rect(r) => Self::Rect(r + other),
            Self::Shape(s) => Self::Shape(s + other),
        }
    }
}

impl AddAssign<XZVector> for XZBBox {
    fn add_assign(&mut self, other: XZVector) {
        *self = self.clone() + other;
    }
}

impl Sub<XZVector> for XZBBox {
    type Output = XZBBox;

    fn sub(self, other: XZVector) -> XZBBox {
        match self {
            Self::Rect(r) => Self::Rect(r - other),
            Self::Shape(s) => Self::Shape(s - other),
        }
    }
}

impl SubAssign<XZVector> for XZBBox {
    fn sub_assign(&mut self, other: XZVector) {
        *self = self.clone() - other;
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_valid_inputs() {
        // 2 * 2
        let obj = XZBBox::rect_from_xz_lengths(1.0, 1.0);
        assert!(obj.is_ok());
        let obj = obj.unwrap();
        assert_eq!(obj.bounding_rect().total_blocks_x(), 2);
        assert_eq!(obj.bounding_rect().total_blocks_z(), 2);
        assert_eq!(obj.bounding_rect().total_blocks(), 4);
        assert_eq!(obj.min_x(), 0);
        assert_eq!(obj.max_x(), 1);
        assert_eq!(obj.min_z(), 0);
        assert_eq!(obj.max_z(), 1);

        // edge cases
        // 1 * 2
        let obj = XZBBox::rect_from_xz_lengths(0.0, 1.0);
        assert!(obj.is_ok());
        let obj = obj.unwrap();
        assert_eq!(obj.bounding_rect().total_blocks_x(), 1);
        assert_eq!(obj.bounding_rect().total_blocks_z(), 2);
        assert_eq!(obj.bounding_rect().total_blocks(), 2);
        assert_eq!(obj.min_x(), 0);
        assert_eq!(obj.max_x(), 0);
        assert_eq!(obj.min_z(), 0);
        assert_eq!(obj.max_z(), 1);

        // 2 * 1
        let obj = XZBBox::rect_from_xz_lengths(1.0, 0.0);
        assert!(obj.is_ok());
        let obj = obj.unwrap();
        assert_eq!(obj.bounding_rect().total_blocks_x(), 2);
        assert_eq!(obj.bounding_rect().total_blocks_z(), 1);
        assert_eq!(obj.bounding_rect().total_blocks(), 2);
        assert_eq!(obj.min_x(), 0);
        assert_eq!(obj.max_x(), 1);
        assert_eq!(obj.min_z(), 0);
        assert_eq!(obj.max_z(), 0);

        // normal case
        let obj = XZBBox::rect_from_xz_lengths(123.4, 322.5);
        assert!(obj.is_ok());
        let obj = obj.unwrap();
        assert_eq!(obj.bounding_rect().total_blocks_x(), 124);
        assert_eq!(obj.bounding_rect().total_blocks_z(), 323);
        assert_eq!(obj.bounding_rect().total_blocks(), 124 * 323);
        assert_eq!(obj.min_x(), 0);
        assert_eq!(obj.max_x(), 123);
        assert_eq!(obj.min_z(), 0);
        assert_eq!(obj.max_z(), 322);
    }

    #[test]
    #[allow(clippy::excessive_precision)]
    fn test_invalid_inputs() {
        assert!(XZBBox::rect_from_xz_lengths(-1.0, 1.5).is_err());
        assert!(XZBBox::rect_from_xz_lengths(1323.5, -3287238791.395).is_err());
        assert!(XZBBox::rect_from_xz_lengths(-239928341323.29389498, -3287238791.938395).is_err());
        assert!(XZBBox::rect_from_xz_lengths(-0.1, 1.5).is_err());
        assert!(XZBBox::rect_from_xz_lengths(-0.5, 1.5).is_err());
        assert!(XZBBox::rect_from_xz_lengths(123948761293874123.2398, -0.5).is_err());

        assert!(XZBBox::rect_from_xz_lengths(i32::MAX as f64 + 10.0, -0.5).is_err());
        assert!(XZBBox::rect_from_xz_lengths(0.2, i32::MAX as f64 + 10.0).is_err());
    }

    #[test]
    fn test_rect_from_min_max() {
        let obj = XZBBox::rect_from_min_max(-100, -200, 100, 200);
        assert!(obj.is_ok());
        let obj = obj.unwrap();
        assert_eq!(obj.min_x(), -100);
        assert_eq!(obj.max_x(), 100);
        assert_eq!(obj.min_z(), -200);
        assert_eq!(obj.max_z(), 200);
        assert_eq!(obj.bounding_rect().total_blocks_x(), 201);
        assert_eq!(obj.bounding_rect().total_blocks_z(), 401);

        // Invalid: min > max
        assert!(XZBBox::rect_from_min_max(100, 0, 50, 100).is_err());
    }
}
