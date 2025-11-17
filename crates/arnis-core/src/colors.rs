pub type RGBTuple = (u8, u8, u8);

pub fn color_text_to_rgb_tuple(text: &str) -> Option<RGBTuple> {
    if let Some(rgb) = full_hex_color_to_rgb_tuple(text) {
        return Some(rgb);
    }

    if let Some(rgb) = short_hex_color_to_rgb_tuple(text) {
        return Some(rgb);
    }

    if let Some(rgb) = color_name_to_rgb_tuple(text) {
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
    Some(match text {
        "aqua" | "cyan" => (0, 255, 255),
        "beige" => (187, 173, 142),
        "black" => (0, 0, 0),
        "blue" => (0, 0, 255),
        "brown" => (128, 64, 0),
        // darkgrey
        "fuchsia" | "magenta" => (255, 0, 255),
        "gray" | "grey" => (128, 128, 128),
        "green" => (0, 128, 0),
        // lightgrey
        "lime" => (0, 255, 0),
        "maroon" => (128, 0, 0),
        "navy" => (0, 0, 128),
        "olive" => (128, 128, 0),
        "orange" => (255, 128, 0),
        "purple" => (128, 0, 128),
        "red" => (255, 0, 0),
        "silver" => (192, 192, 192),
        "teal" => (0, 128, 0),
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
