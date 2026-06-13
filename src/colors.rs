pub type RGBTuple = (u8, u8, u8);

pub fn color_text_to_rgb_tuple(text: &str) -> Option<RGBTuple> {
    let trimmed = text.trim();

    if let Some(rgb) = full_hex_color_to_rgb_tuple(trimmed) {
        return Some(rgb);
    }

    if let Some(rgb) = short_hex_color_to_rgb_tuple(trimmed) {
        return Some(rgb);
    }

    if let Some(rgb) = color_name_to_rgb_tuple(trimmed) {
        return Some(rgb);
    }

    None
}

fn full_hex_color_to_rgb_tuple(text: &str) -> Option<RGBTuple> {
    if text.len() != 7
        || !text.starts_with("#")
        || !text.chars().skip(1).all(|c: char| c.is_ascii_hexdigit())
    {
        return None;
    }
    let r: u8 = u8::from_str_radix(&text[1..3], 16).unwrap();
    let g: u8 = u8::from_str_radix(&text[3..5], 16).unwrap();
    let b: u8 = u8::from_str_radix(&text[5..7], 16).unwrap();
    Some((r, g, b))
}

fn short_hex_color_to_rgb_tuple(text: &str) -> Option<RGBTuple> {
    if text.len() != 4
        || !text.starts_with("#")
        || !text.chars().skip(1).all(|c: char| c.is_ascii_hexdigit())
    {
        return None;
    }
    let r: u8 = u8::from_str_radix(&text[1..2], 16).unwrap();
    let r: u8 = r | (r << 4);
    let g: u8 = u8::from_str_radix(&text[2..3], 16).unwrap();
    let g: u8 = g | (g << 4);
    let b: u8 = u8::from_str_radix(&text[3..4], 16).unwrap();
    let b: u8 = b | (b << 4);
    Some((r, g, b))
}

// https://wiki.openstreetmap.org/wiki/Key:colour
// https://wiki.openstreetmap.org/wiki/Key:roof:colour
fn color_name_to_rgb_tuple(text: &str) -> Option<RGBTuple> {
    let normalized: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect();

    Some(match normalized.as_str() {
        "aqua" | "cyan" => (0, 255, 255),
        "beige" => (187, 173, 142),
        "black" => (0, 0, 0),
        "blue" => (0, 0, 255),
        "brown" => (128, 64, 0),
        "darkgray" | "darkgrey" => (96, 96, 96),
        "darkbrown" => (90, 50, 20),
        "darkred" => (139, 0, 0),
        "dimgray" | "dimgrey" => (105, 105, 105),
        "firebrick" => (178, 34, 34),
        "fuchsia" | "magenta" => (255, 0, 255),
        "gold" => (255, 215, 0),
        "gray" | "grey" => (128, 128, 128),
        "green" => (0, 128, 0),
        "ivory" => (255, 255, 240),
        "khaki" => (240, 230, 140),
        "lightblue" => (173, 216, 230),
        "lightgray" | "lightgrey" => (211, 211, 211),
        "lightgreen" => (144, 238, 144),
        "lightyellow" => (255, 255, 224),
        "lime" => (0, 255, 0),
        "limestone" => (246, 240, 208),
        "maroon" => (128, 0, 0),
        "navy" => (0, 0, 128),
        "olive" => (128, 128, 0),
        "orange" => (255, 128, 0),
        "pink" => (255, 192, 203),
        "purple" => (128, 0, 128),
        "red" => (255, 0, 0),
        "salmon" => (250, 128, 114),
        "sandstone" => (215, 188, 138),
        "silver" => (192, 192, 192),
        "tan" => (210, 180, 140),
        "teal" => (0, 128, 128),
        "white" => (255, 255, 255),
        "yellow" => (255, 255, 0),
        _ => {
            return None;
        }
    })
}

pub fn rgb_distance(from: &RGBTuple, to: &RGBTuple) -> u32 {
    // i32 because .pow(2) returns the same data type as self and 255^2 wouldn't fit
    let difference: (i32, i32, i32) = (
        from.0 as i32 - to.0 as i32,
        from.1 as i32 - to.1 as i32,
        from.2 as i32 - to.2 as i32,
    );
    let distance: i32 = difference.0.pow(2) + difference.1.pow(2) + difference.2.pow(2);
    distance as u32
}

/// Squared perceptual distance (Oklab) between two sRGB colors.
pub fn oklab_distance(from: &RGBTuple, to: &RGBTuple) -> f32 {
    let a = rgb_to_oklab(from.0, from.1, from.2);
    let b = rgb_to_oklab(to.0, to.1, to.2);
    let dl = a.0 - b.0;
    let da = a.1 - b.1;
    let db = a.2 - b.2;
    dl * dl + da * da + db * db
}

#[inline]
fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn rgb_to_oklab(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = srgb_to_linear(r);
    let g = srgb_to_linear(g);
    let b = srgb_to_linear(b);
    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_84 * g + 0.629_978_7 * b;
    let l = l.cbrt();
    let m = m.cbrt();
    let s = s.cbrt();
    (
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklab_identical_colors_zero_distance() {
        let c = (128, 64, 200);
        assert!(oklab_distance(&c, &c) < 1e-6);
    }

    #[test]
    fn oklab_prefers_perceptual_neighbor() {
        // Iron-brown target color.
        let target = (139, 90, 60);
        // Both pure red and a brown shade are 90 away in raw RGB.
        let red = (229, 0, 0);
        let brown = (115, 70, 40);
        let d_red = oklab_distance(&target, &red);
        let d_brown = oklab_distance(&target, &brown);
        assert!(
            d_brown < d_red,
            "expected brown closer than red in Oklab, got brown={d_brown} red={d_red}"
        );
    }
}
