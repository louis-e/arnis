use serde::Deserialize;
use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Debug, Deserialize, Copy, Clone)]
pub struct XZPoint {
    pub x: i32,
    pub z: i32,
}

impl XZPoint {
    #[inline]
    pub fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }
}

#[derive(Debug, Deserialize, Copy, Clone)]
pub struct XZVector {
    pub dx: i32,
    pub dz: i32,
}

#[derive(Debug, Deserialize, Copy, Clone)]
pub struct XZBBox {
    pub point1: XZPoint,
    pub point2: XZPoint,
}

impl XZBBox {
    pub fn from_scale_factors(scale_factor_x: f64, scale_factor_z: f64) -> Self {
        XZBBox {
            point1: XZPoint {
                x: 0,
                z: 0,
            },
            point2: XZPoint {
                x: scale_factor_x as i32,
                z: scale_factor_z as i32,
            },
        }
    }

    pub fn nblock(&self) -> u64 {
        let nx = self.point2.x - self.point1.x + 1;
        let nz = self.point2.z - self.point1.z + 1;

        println!("nx nz {} {}", nx, nz);

        (nx as u64) * (nz as u64)
    }

    pub fn nblock_x(&self) -> u32 {
        let nx = self.point2.x - self.point1.x + 1;
        nx as u32
    }

    pub fn nblock_z(&self) -> u32 {
        let nz = self.point2.z - self.point1.z + 1;
        nz as u32
    }

    pub fn contains(&self, xzpoint: &XZPoint) -> bool {
        xzpoint.x >= self.point1.x && xzpoint.x <= self.point2.x &&
        xzpoint.z >= self.point1.z && xzpoint.z <= self.point2.z
    }
}

// below are associated +- operators
impl Add<XZVector> for XZPoint {
    type Output = XZPoint;

    fn add(self, other: XZVector) -> XZPoint {
        XZPoint {
            x: self.x + other.dx,
            z: self.z + other.dz,
        }
    }
}

impl AddAssign<XZVector> for XZPoint {
    fn add_assign(&mut self, other: XZVector) {
        self.x += other.dx;
        self.z += other.dz;
    }
}

impl Sub for XZPoint {
    type Output = XZVector;

    fn sub(self, other: XZPoint) -> XZVector {
        XZVector {
            dx: self.x - other.x,
            dz: self.z - other.z,
        }
    }
}

impl Sub<XZVector> for XZPoint {
    type Output = XZPoint;

    fn sub(self, other: XZVector) -> XZPoint {
        XZPoint {
            x: self.x - other.dx,
            z: self.z - other.dz,
        }
    }
}

impl SubAssign<XZVector> for XZPoint {
    fn sub_assign(&mut self, other: XZVector) {
        self.x -= other.dx;
        self.z -= other.dz;
    }
}

impl Add<XZVector> for XZBBox {
    type Output = XZBBox;

    fn add(self, other: XZVector) -> XZBBox {
        XZBBox {
            point1: self.point1 + other,
            point2: self.point2 + other,
        }
    }
}

impl AddAssign<XZVector> for XZBBox {
    fn add_assign(&mut self, other: XZVector) {
        self.point1 += other;
        self.point2 += other;
    }
}

impl Sub<XZVector> for XZBBox {
    type Output = XZBBox;

    fn sub(self, other: XZVector) -> XZBBox {
        XZBBox {
            point1: self.point1 - other,
            point2: self.point2 - other,
        }
    }
}

impl SubAssign<XZVector> for XZBBox {
    fn sub_assign(&mut self, other: XZVector) {
        self.point1 -= other;
        self.point2 -= other;
    }
}
