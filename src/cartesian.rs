#[derive(Copy, Clone)]
pub struct XZPoint {
    pub x: i32,
    pub z: i32,
}

impl XZPoint {
    pub fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }
}

pub struct XYZPoint {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl XYZPoint {
    pub fn from_xz(xz: XZPoint, y: i32) -> Self {
        Self {
            x: xz.x,
            y,
            z: xz.z,
        }
    }
}
