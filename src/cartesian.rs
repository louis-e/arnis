#[derive(Copy, Clone)]
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
