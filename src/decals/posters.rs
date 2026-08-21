//! Poster art for billboards and advertising columns. Plain images with no map data in
//! them, so a replacement of the same name and size in `assets/decorations/posters/` wins.

/// Billboard panel size in map tiles (3 wide, 2 tall = 384x256 px).
pub const BILLBOARD_TILES: (u32, u32) = (3, 2);
/// Advertising column panel size in map tiles (1 wide, 2 tall = 128x256 px).
pub const COLUMN_TILES: (u32, u32) = (1, 2);

macro_rules! posters {
    ($konst:ident, $lookup:ident, $prefix:literal, $($n:literal),* $(,)?) => {
        /// Number of variants; keys are taken modulo this.
        pub const $konst: u8 = [$($n),*].len() as u8;

        /// PNG bytes for a variant.
        pub fn $lookup(variant: u8) -> Option<&'static [u8]> {
            match variant % $konst {
                $($n => Some(include_bytes!(concat!(
                    "../../assets/decorations/posters/", $prefix, stringify!($n), ".png"
                ))),)*
                _ => None,
            }
        }
    };
}

posters!(BILLBOARD_COUNT, billboard, "billboard_", 0, 1, 2, 3, 4, 5);
posters!(COLUMN_COUNT, column, "column_", 0, 1, 2, 3, 4);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_poster_decodes_at_its_panel_size() {
        for v in 0..BILLBOARD_COUNT {
            let img = image::load_from_memory(billboard(v).unwrap()).unwrap();
            assert_eq!((img.width(), img.height()), (384, 256), "billboard {v}");
        }
        for v in 0..COLUMN_COUNT {
            let img = image::load_from_memory(column(v).unwrap()).unwrap();
            assert_eq!((img.width(), img.height()), (128, 256), "column {v}");
        }
    }

    #[test]
    fn variants_wrap_around() {
        assert_eq!(billboard(0).unwrap(), billboard(BILLBOARD_COUNT).unwrap());
        assert_eq!(column(1).unwrap(), column(COLUMN_COUNT + 1).unwrap());
    }
}
