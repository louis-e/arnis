use crate::block_definitions::{
    Block, BLACK_TERRACOTTA, BRICK, COBBLESTONE, CYAN_TERRACOTTA, DIRT, GLASS, GRASS_BLOCK,
    GRAY_CONCRETE_POWDER, GREEN_WOOL, IRON_BLOCK, MOSS_BLOCK, MUD, OAK_PLANKS, PACKED_ICE, PODZOL,
    RED_TERRACOTTA, SAND, SANDSTONE, SMOOTH_SANDSTONE, SNOW_BLOCK, STONE, TERRACOTTA, WHITE_CARPET,
    WHITE_TERRACOTTA,
};
use crate::osm_parser::ProcessedWay;

/// Checks whether `surface` starts with any of the given prefixes.
#[inline]
fn starts_with_any(surface: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| surface.starts_with(p))
}

pub fn get_blocks_for_surface(surface_type: &str) -> Option<&'static [Block]> {
    let s = surface_type;

    // 1. Asphalt / concrete / paving stones / gravel / compacted …
    if starts_with_any(s, &[
        "asphalt", "concrete", "paving_stones", "sett", "cobblestone",
        "unhewn_cobblestone", "fine_gravel", "gravel", "compacted", "crushed",
        "grit", "ballast", "track_ballast", "loose_gravel", "packed_gravel",
        "shingle", "stone_dust", "rock_dust", "decomposed_granite",
        "crusher_fines", "saibro", "blaes", "karral", "grus", "щебень",
        "gravel_turf", "pebblestone", "resin_gravel",
        "paving_slabs", "paving_slab", "pavement_stones", "paved_stones",
        "paving_block", "block_paving", "slabs", "flag", "interlock",
        "roman_paving", "tiles", "ceramic", "pavers", "paver",
        "stone_slabs", "stone:slabs", "stone:plates", "stone_plates",
        "sett:plates", "clinker_plates", "trylinka", "adoquín",
        "empedrado", "tactile_paving", "block",
    ]) {
        return Some(&[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA]);
    }

    // 2. Dirt / ground / earth / unpaved
    // Some mixed strings that start with “paved;unpaved” are treated as dirt.
    if s == "paved;unpaved" || s == "paved; unpaved"
        || s == "unpaved;paved" || s == "unpaved; paved"
        || s == "unpaved_and_paved"
    {
        return Some(&[DIRT]);
    }
    if starts_with_any(s, &[
        "dirt", "ground", "earth", "soil", "bare_ground", "baregound",
        "terra", "tierra", "toprak", "грунт", "tanah", "sterrato",
        "terre_battue", "terraway", "murram", "loose_earth", "rammed_earth",
        "dirt_rock", "dirt_sand", "earth_grass", "ground:lanes",
        "unpaved", "sin_pavimentar", "não_pavimentado", "ohne_straßenbelag",
    ]) {
        return Some(&[DIRT]);
    }
    // exact strings that are dirt but don’t start with the above
    if s == "red earth" || s == "dirt/grass" || s == "dirt+rocks"
        || s == "ground,grass" || s == "ground,_sand" || s == "ground,gravel"
        || s == "ground,rocks" || s == "ground,mud" || s == "ground,grass,rocks"
        || s == "ground_,_grass" || s == "dirt/ground" || s == "ground/grass"
        || s == "dirt;sand" || s == "dirt;gravel" || s == "unpaved2"
        || s == "sin_pavimentarw" || s == "não_pavimentadoc"
        || s == "ohne_straßenbelag3" || s == "unpaved,_dirt"
    {
        return Some(&[DIRT]);
    }

    // 3. Grass
    if starts_with_any(s, &[
        "grass", "meadow", "short_grass", "turf", "overgrown", "garden",
        "park", "plants", "flowerbed", "shrub", "scrub", "bushes",
        "grass_scrub", "grassneeds", "green", "cesped", "grama",
        "grass_paver", "green_paver", "green_parking", "leaves",
    ]) || s == "Gras + scrub" || s == "grass,_sand,_ground"
        || s == "grass, ground" || s == "grass; grass"
        || s == "grass/trees" || s == "bushes/trees"
        || s == "grass_+_bogland" || s == "grassland"
    {
        return Some(&[GRASS_BLOCK]);
    }

    // 4. Wood
    if starts_with_any(s, &[
        "wood", "boardwalk", "railway_sleepers", "bamboo", "log", "planks",
    ]) {
        return Some(&[OAK_PLANKS]);
    }

    // 5. Sand
    if starts_with_any(s, &[
        "sand", "fine_sand", "loose_sand", "coral_sand", "salt", "arena",
        "areia", "coral",
    ]) {
        return Some(&[SAND]);
    }

    // 6. Stone / rock (the ones not already caught by asphalt)
    if starts_with_any(s, &[
        "stone", "rock", "bare_rock", "rocky", "scree", "rubble", "boulder",
        "bedrock", "slate", "shale", "granite", "marble", "quartz", "tuff",
        "flagstone", "stones", "stepping_stones",
    ]) || s == "basalt, sandstone" || s == "rocks" || s == "boulders"
    {
        return Some(&[STONE, COBBLESTONE]);
    }

    // 7. Metal
    if starts_with_any(s, &[
        "metal", "steel", "iron", "aluminium", "aluminum", "frp_grate",
        "fibre_reinforced_polymer_grate", "tin", "copper",
    ]) {
        return Some(&[IRON_BLOCK]);
    }

    // 8. Artificial turf
    if s == "artificial_turf" || s == "synthetic" || s == "artificial"
        || s == "astroturf" || s == "greenset" || s == "artificial_turf;sand"
    {
        return Some(&[GREEN_WOOL]);
    }

    // 9. Rubber
    if starts_with_any(s, &["rubber", "polyurethane", "poliuretan"]) {
        return Some(&[BLACK_TERRACOTTA]);
    }

    // 10. Acrylic / hard court
    if s == "acrylic" || s == "hard_court" || s == "hard"
        || s == "hardcourt" || s == "decoturf"
    {
        return Some(&[CYAN_TERRACOTTA]);
    }

    // 11. Bricks
    if s == "bricks" || s == "brick" || s == "brick_weave" || s == "bricklayer" {
        return Some(&[BRICK]);
    }

    // 12. Tartan
    if s.starts_with("tartan") {
        return Some(&[RED_TERRACOTTA]);
    }

    // 13. Clay
    if s == "clay" || s == "cray" || s == "artificial_clay" {
        return Some(&[TERRACOTTA]);
    }

    // 14. Mulch / bark
    if starts_with_any(s, &[
        "mulch", "bark", "multch", "peat", "woodchips", "sawdust",
    ]) {
        return Some(&[PODZOL]);
    }

    // 15. Snow
    if s.starts_with("snow") || s == "winter_road" {
        return Some(&[SNOW_BLOCK]);
    }

    // 16. Ice
    if s.starts_with("ice") || s == "glacier" {
        return Some(&[PACKED_ICE]);
    }

    // 17. Mud
    if s.starts_with("mud") || s == "marsh" || s == "lahar" {
        return Some(&[MUD]);
    }

    // 18. Carpet
    if s == "carpet" || s == "linoleum" || s == "vinyl" {
        return Some(&[WHITE_CARPET]);
    }

    // 19. Glass
    if s == "glass" || s == "glazing" {
        return Some(&[GLASS]);
    }

    // 20. Sandstone
    if s == "sandstone" || s == "smooth_sandstone" {
        return Some(&[SANDSTONE, SMOOTH_SANDSTONE]);
    }

    // 21. Chalk
    if s == "chalk" {
        return Some(&[WHITE_TERRACOTTA]);
    }

    // 22. Moss
    if s == "moss" {
        return Some(&[MOSS_BLOCK]);
    }

    // 23. Paved (only the variants that didn’t match earlier, e.g. paved;asphalt)
    if s.starts_with("paved") {
        return Some(&[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA]);
    }

    None
}

pub fn get_blocks_for_surface_way<'a>(way: &ProcessedWay, default: &'a [Block]) -> &'a [Block] {
    way.tags
        .get("surface")
        .and_then(|s| get_blocks_for_surface(s))
        .unwrap_or(default)
}

#[inline]
pub fn semirandom_surface(x: i32, z: i32, block_types: &[Block]) -> Block {
    let mut h = (x as u32).wrapping_mul(0x9E3779B9) ^ (z as u32).wrapping_mul(0x517CC1B7);
    h ^= h >> 16;
    h = h.wrapping_mul(0x45D9F3B);
    h ^= h >> 16;
    block_types[(h as usize) % block_types.len()]
}