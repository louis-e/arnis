use crate::block_definitions::{
    Block, BRICK, COBBLESTONE, CYAN_TERRACOTTA, DIRT, GRASS_BLOCK, GRAVEL, GRAY_CONCRETE_POWDER,
    GREEN_WOOL, IRON_BLOCK, MUD, OAK_PLANKS, PACKED_ICE, PODZOL, RED_CONCRETE, RED_TERRACOTTA,
    SAND, STONE, TERRACOTTA,
};
use crate::osm_parser::ProcessedWay;
use std::collections::HashMap;

/// Red paving for bicycle paths, the way the Netherlands, Denmark and much of Germany lay
/// them: brick-red asphalt flecked with a deeper red, the same speckle the road mix uses.
pub const CYCLEWAY_MIX: &[Block] = &[RED_TERRACOTTA, RED_TERRACOTTA, RED_CONCRETE];

/// Palette of a `highway=cycleway`: red wherever it is paved, and when the surface is not
/// tagged at all. An unpaved surface keeps its own blocks, and so does a colour tag that says
/// anything but red, so a mapped grey or green path stays that way.
pub fn cycleway_palette(tags: &HashMap<String, String>) -> Option<&'static [Block]> {
    // Where the path crosses a road the road keeps its surface.
    if tags.get("cycleway").is_some_and(|v| v == "crossing") || tags.contains_key("crossing") {
        return None;
    }
    let colour = tags.get("surface:colour").or_else(|| tags.get("colour"));
    if colour.is_some_and(|c| !is_red_colour(c)) {
        return None;
    }
    match tags.get("surface").map(String::as_str) {
        None
        | Some(
            "asphalt" | "paved" | "concrete" | "concrete:plates" | "concrete:lanes" | "cement"
            | "chipseal" | "bitmac" | "paving_stones" | "sett" | "bricks" | "brick",
        ) => Some(CYCLEWAY_MIX),
        _ => None,
    }
}

/// Whether an OSM colour value (a name or `#rgb`/`#rrggbb`) is a red.
fn is_red_colour(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        // Also keeps the byte slicing below on character boundaries.
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
        let len = match hex.len() {
            3 => 1,
            6 => 2,
            _ => return false,
        };
        let channel = |i: usize| {
            let v = u32::from_str_radix(&hex[i * len..(i + 1) * len], 16).unwrap_or(0);
            if len == 1 {
                v * 17
            } else {
                v
            }
        };
        let (r, g, b) = (channel(0), channel(1), channel(2));
        return r >= 0x80 && 2 * r > 3 * g && 2 * r > 3 * b;
    }
    value.contains("red") || matches!(value.as_str(), "maroon" | "crimson" | "brick")
}

pub fn get_blocks_for_surface(surface_type: &str) -> Option<&'static [Block]> {
    match surface_type {
        "clay" => Some(&[TERRACOTTA]),
        "sand" => Some(&[SAND]),
        "tartan" => Some(&[RED_TERRACOTTA]),
        "grass" | "grass_paver" => Some(&[GRASS_BLOCK]),
        "artificial_turf" => Some(&[GREEN_WOOL]),
        // `unpaved` is the second most common surface value in OSM; it and its
        // synonyms render as bare dirt like tracks and fields.
        "dirt" | "ground" | "earth" | "soil" | "unpaved" => Some(&[DIRT]),
        "mud" => Some(&[MUD]),
        "mulch" | "woodchips" => Some(&[PODZOL]),
        "pebblestone" | "cobblestone" | "unhewn_cobblestone" | "stepping_stones" => {
            Some(&[COBBLESTONE])
        }
        "stone" | "rock" => Some(&[STONE]),
        "ice" => Some(&[PACKED_ICE]),
        // Paving-stones, sett and poured concrete roads render with the
        // same asphalt mix as `surface=asphalt`. Using the mix directly
        // (rather than a palette that also includes stone_bricks /
        // light_gray_concrete) is what guarantees these surfaces never
        // place L/S blocks that could later show up as islands inside
        // adjacent major roads — the road-overwrite blacklist already
        // protects the asphalt mix, so overlap resolves cleanly. `paved`,
        // `concrete:*`, `cement`, `chipseal` and `bitmac` are all hard road
        // surfaces and share the mix for the same reason.
        "paving_stones" | "sett" | "paved" | "cement" | "chipseal" | "bitmac"
        | "concrete:plates" | "concrete:lanes" => Some(&[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA]),
        "bricks" | "brick" => Some(&[BRICK]),
        "metal" => Some(&[IRON_BLOCK]),
        "wood" => Some(&[OAK_PLANKS]),
        "asphalt" => Some(&[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA]),
        "gravel" | "fine_gravel" | "compacted" => Some(&[GRAVEL]),
        "concrete" => Some(&[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA]),
        _ => None,
    }
}

/// Returns the block slice for a way's `surface=*` tag, or `default` when
/// the tag is missing or unknown. Takes and returns `&[Block]` so the hot
/// paths don't allocate — the tables in `get_blocks_for_surface` are all
/// `&'static [Block]`.
pub fn get_blocks_for_surface_way<'a>(way: &ProcessedWay, default: &'a [Block]) -> &'a [Block] {
    way.tags
        .get("surface")
        .and_then(|s| get_blocks_for_surface(s))
        .unwrap_or(default)
}

/// Pick a surface block deterministically from `block_types` based on
/// coordinates. The same `(x, z)` always returns the same block (so a
/// later overwrite pass sees a stable result), while adjacent cells
/// scatter across the palette for a varied, speckled look.
/// A 1-element slice effectively returns that single block everywhere.
#[inline]
pub fn semirandom_surface(x: i32, z: i32, block_types: &[Block]) -> Block {
    // Combine coordinates into a single value and apply bit mixing for a scattered look
    let mut h = (x as u32).wrapping_mul(0x9E3779B9) ^ (z as u32).wrapping_mul(0x517CC1B7);
    h ^= h >> 16;
    h = h.wrapping_mul(0x45D9F3B);
    h ^= h >> 16;
    block_types[(h as usize) % block_types.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_definitions::{DIRT, GRASS_BLOCK, GRAVEL};

    #[test]
    fn common_unmapped_surfaces_now_resolve() {
        // Previously fell through to None; these are high-use OSM surface values.
        assert_eq!(get_blocks_for_surface("unpaved"), Some(&[DIRT][..]));
        assert_eq!(get_blocks_for_surface("compacted"), Some(&[GRAVEL][..]));
        assert_eq!(
            get_blocks_for_surface("grass_paver"),
            Some(&[GRASS_BLOCK][..])
        );
    }

    #[test]
    fn hard_road_surfaces_share_the_asphalt_mix() {
        // Must stay identical to asphalt so they never place island-forming blocks.
        let asphalt = get_blocks_for_surface("asphalt");
        for s in [
            "paved",
            "concrete:plates",
            "concrete:lanes",
            "cement",
            "chipseal",
        ] {
            assert_eq!(
                get_blocks_for_surface(s),
                asphalt,
                "{s} should use the asphalt mix"
            );
        }
    }

    #[test]
    fn unknown_surface_still_returns_none() {
        assert_eq!(get_blocks_for_surface("definitely_not_a_surface"), None);
    }

    #[test]
    fn cycleways_are_red_where_paved_and_keep_other_surfaces() {
        let tags = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        for paved in [
            &[][..],
            &[("surface", "asphalt")],
            &[("surface", "paving_stones")],
        ] {
            assert_eq!(
                cycleway_palette(&tags(paved)),
                Some(CYCLEWAY_MIX),
                "{paved:?}"
            );
        }
        for unpaved in ["gravel", "compacted", "dirt", "grass", "sand"] {
            assert_eq!(cycleway_palette(&tags(&[("surface", unpaved)])), None);
        }
        // A mapped colour wins over the default red, whichever way it is written.
        for grey in ["grey", "#808080", "black", "green"] {
            assert_eq!(cycleway_palette(&tags(&[("surface:colour", grey)])), None);
        }
        for red in ["red", "darkred", "#c00", "#B03020", "maroon"] {
            assert_eq!(
                cycleway_palette(&tags(&[("surface:colour", red)])),
                Some(CYCLEWAY_MIX),
                "{red}"
            );
        }
        assert_eq!(cycleway_palette(&tags(&[("colour", "blue")])), None);
        // Nonsense never panics.
        assert!(!is_red_colour("#é1"));
        assert!(!is_red_colour("#12345"));
    }
}
