//! Fronts of the street machines: ATMs, ticket and vending machines built as a two-block
//! cabinet, and the small dispensers built as a box on a post. Drawn in map palette colours,
//! so the quantizer keeps them as they are; a replacement of the same name and size in
//! `assets/decorations/machines/` wins.

/// Fronts of the dispensers built as a box on a post, a single block tall.
pub const SMALL: &[&str] = &["dispenser", "dog_bags"];

/// Front size in map tiles: one block wide, two tall for a cabinet (128x256 px), one for
/// the box of a small dispenser (128x128 px).
pub fn tiles(name: &str) -> (u32, u32) {
    if SMALL.contains(&name) {
        (1, 1)
    } else {
        (1, 2)
    }
}

macro_rules! machines {
    ($($name:literal),* $(,)?) => {
        /// Every bundled machine front.
        #[allow(dead_code)]
        pub const NAMES: &[&str] = &[$($name),*];

        /// PNG bytes of a bundled machine front.
        pub fn asset(name: &str) -> Option<&'static [u8]> {
            match name {
                $($name => Some(include_bytes!(concat!(
                    "../../assets/decorations/machines/", $name, ".png"
                ))),)*
                _ => None,
            }
        }
    };
}

machines!(
    "atm",
    "coin_press",
    "dispenser",
    "dog_bags",
    "ticket_machine",
    "vending_drinks",
    "vending_snacks",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_machine_decodes_at_its_panel_size() {
        for name in NAMES {
            let img = image::load_from_memory(asset(name).unwrap()).unwrap();
            let (cols, rows) = tiles(name);
            assert_eq!(
                (img.width(), img.height()),
                (128 * cols, 128 * rows),
                "{name}"
            );
        }
    }
}
