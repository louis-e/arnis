use super::rectangle::XZBBoxRect;
use crate::coordinate_system::cartesian::{XZPoint, XZVector};
use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};

/// Bounding Box in minecraft XZ space with varied shapes.
#[derive(Clone, Debug)]
pub enum XZBBox {
    Rect(XZBBoxRect),
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
                "Invalid XZBBox::Rect from xz lengths: length x should >=0 , but encountered {}",
                length_x
            ));
        }

        if !lenz_ge_0 {
            return Err(format!(
                "Invalid XZBBox::Rect from xz lengths: length z should >=0 , but encountered {}",
                length_x
            ));
        }

        if lenx_overflow {
            return Err(format!(
                "Invalid XZBBox::Rect from xz lengths: length x too large for i32: {}",
                length_x
            ));
        }

        if lenz_overflow {
            return Err(format!(
                "Invalid XZBBox::Rect from xz lengths: length z too large for i32: {}",
                length_z
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

    /// Check whether an XZPoint is covered
    pub fn contains(&self, xzpoint: &XZPoint) -> bool {
        match self {
            Self::Rect(r) => r.contains(xzpoint),
        }
    }

    /// Return the circumscribed rectangle of the current XZBBox shape
    pub fn circumscribed_rect(&self) -> XZBBoxRect {
        match self {
            Self::Rect(r) => *r,
        }
    }

    /// Return the min x in all covered blocks
    pub fn min_x(&self) -> i32 {
        self.circumscribed_rect().point1().x
    }

    /// Return the max x in all covered blocks
    pub fn max_x(&self) -> i32 {
        self.circumscribed_rect().point2().x
    }

    /// Return the min z in all covered blocks
    pub fn min_z(&self) -> i32 {
        self.circumscribed_rect().point1().z
    }

    /// Return the max z in all covered blocks
    pub fn max_z(&self) -> i32 {
        self.circumscribed_rect().point2().z
    }
}

impl fmt::Display for XZBBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rect(r) => write!(f, "XZBBox::{}", r),
        }
    }
}

// below are associated +- operators
impl Add<XZVector> for XZBBox {
    type Output = XZBBox;

    fn add(self, other: XZVector) -> XZBBox {
        match self {
            Self::Rect(r) => Self::Rect(r + other),
        }
    }
}

impl AddAssign<XZVector> for XZBBox {
    fn add_assign(&mut self, other: XZVector) {
        match self {
            Self::Rect(r) => *r += other,
        }
    }
}

impl Sub<XZVector> for XZBBox {
    type Output = XZBBox;

    fn sub(self, other: XZVector) -> XZBBox {
        match self {
            Self::Rect(r) => Self::Rect(r - other),
        }
    }
}

impl SubAssign<XZVector> for XZBBox {
    fn sub_assign(&mut self, other: XZVector) {
        match self {
            Self::Rect(r) => *r -= other,
        }
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
        assert_eq!(obj.circumscribed_rect().total_blocks_x(), 2);
        assert_eq!(obj.circumscribed_rect().total_blocks_z(), 2);
        assert_eq!(obj.circumscribed_rect().total_blocks(), 4);
        assert_eq!(obj.min_x(), 0);
        assert_eq!(obj.max_x(), 1);
        assert_eq!(obj.min_z(), 0);
        assert_eq!(obj.max_z(), 1);

        // edge cases
        // 1 * 2
        let obj = XZBBox::rect_from_xz_lengths(0.0, 1.0);
        assert!(obj.is_ok());
        let obj = obj.unwrap();
        assert_eq!(obj.circumscribed_rect().total_blocks_x(), 1);
        assert_eq!(obj.circumscribed_rect().total_blocks_z(), 2);
        assert_eq!(obj.circumscribed_rect().total_blocks(), 2);
        assert_eq!(obj.min_x(), 0);
        assert_eq!(obj.max_x(), 0);
        assert_eq!(obj.min_z(), 0);
        assert_eq!(obj.max_z(), 1);

        // 2 * 1
        let obj = XZBBox::rect_from_xz_lengths(1.0, 0.0);
        assert!(obj.is_ok());
        let obj = obj.unwrap();
        assert_eq!(obj.circumscribed_rect().total_blocks_x(), 2);
        assert_eq!(obj.circumscribed_rect().total_blocks_z(), 1);
        assert_eq!(obj.circumscribed_rect().total_blocks(), 2);
        assert_eq!(obj.min_x(), 0);
        assert_eq!(obj.max_x(), 1);
        assert_eq!(obj.min_z(), 0);
        assert_eq!(obj.max_z(), 0);

        // normal case
        let obj = XZBBox::rect_from_xz_lengths(123.4, 322.5);
        assert!(obj.is_ok());
        let obj = obj.unwrap();
        assert_eq!(obj.circumscribed_rect().total_blocks_x(), 124);
        assert_eq!(obj.circumscribed_rect().total_blocks_z(), 323);
        assert_eq!(obj.circumscribed_rect().total_blocks(), 124 * 323);
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
}
