//! `--cave-zone-map <PREFIX>`: render the cave BIOME ZONE layout for `--bbox` and exit,
//! without generating a world. Two top-down PNGs cover the two blotch-scale bands the zone
//! picker uses: `<PREFIX>-upper.png` samples y=-20 (lush/dripstone/mushroom/amethyst/ice
//! band) and `<PREFIX>-deep.png` samples y=-48 (where deep dark and volcanic are live).
//! Plain rock is transparent so the images work as map overlays; a JSON line with the
//! measured share of every theme goes to stdout (prefix `ZONEMAP `). Uses the exact same
//! `Decor::zone()` the real carve uses — same seed, same `--cave-biomes` multipliers — so
//! the preview IS the layout the world will get. The bands are vanilla Y; a raised floor moves
//! the whole layout up with it, so the picture is the same.
//!
//! Ice honours a mountains-only surface gate during real generation; here every column is
//! sampled with a high surface height so the ice blotches are VISIBLE — the map shows where
//! ice would go wherever the terrain is mountainous enough.

use super::decoration::{BiomeAmounts, Decor, Zone};
use crate::args::Args;
use crate::coordinate_system::transformation::CoordTransformer;
use image::{Rgba, RgbaImage};

/// (color, name) per zone; Normal stays transparent.
fn style(z: Zone) -> Option<(Rgba<u8>, &'static str)> {
    match z {
        Zone::Normal => None,
        Zone::Lush => Some((Rgba([108, 207, 95, 210]), "lush")),
        Zone::Dripstone => Some((Rgba([201, 141, 90, 210]), "dripstone")),
        Zone::DeepDark => Some((Rgba([31, 66, 92, 220]), "deepdark")),
        Zone::Mushroom => Some((Rgba([176, 123, 168, 210]), "mushroom")),
        Zone::Ice => Some((Rgba([159, 216, 255, 210]), "ice")),
        Zone::Amethyst => Some((Rgba([154, 107, 216, 210]), "amethyst")),
        Zone::Volcanic => Some((Rgba([224, 102, 60, 220]), "volcanic")),
    }
}

const CORAL_COLOR: Rgba<u8> = Rgba([255, 126, 168, 160]);
const MAX_SIDE: u32 = 1536;

/// Center of the `i`th `step`-wide sample square, measured over the part of it inside the bbox:
/// the last square is usually cut short, and its full-size center can lie outside the bbox.
fn square_center(min: i32, max: i32, i: u32, step: i32) -> i32 {
    let start = min + i as i32 * step;
    let len = (start + step - 1).min(max) - start + 1;
    start + len / 2
}

pub fn render(args: &Args) -> Result<(), String> {
    let prefix = args
        .cave_zone_map
        .as_ref()
        .expect("render() is only called when --cave-zone-map is set");
    let bbox = args.bbox.as_ref().ok_or("--cave-zone-map needs --bbox")?;
    let (_, xzbbox) = CoordTransformer::llbbox_to_xzbbox(bbox, args.scale)
        .map_err(|e| format!("bbox transform failed: {e}"))?;
    let (min_x, max_x, min_z, max_z) = (
        xzbbox.min_x(),
        xzbbox.max_x(),
        xzbbox.min_z(),
        xzbbox.max_z(),
    );
    let amounts = match args.cave_biomes.as_deref() {
        Some(spec) => BiomeAmounts::parse(spec).map_err(|e| format!("--cave-biomes: {e}"))?,
        None => BiomeAmounts::default(),
    };
    let decor = Decor::new(super::SEED, amounts);

    let span_x = (max_x - min_x + 1).max(1) as u32;
    let span_z = (max_z - min_z + 1).max(1) as u32;
    // explicit step = one sample per STEP×STEP square (chunky noise-cell view, upscaled
    // crisply by the viewer); otherwise fine sampling capped at MAX_SIDE.
    let step = match args.cave_zone_map_step {
        Some(s) => (s.clamp(1, 512)) as i32,
        None => (span_x.max(span_z)).div_ceil(MAX_SIDE).max(1) as i32,
    };
    let w = ((span_x as i32 + step - 1) / step).max(1) as u32;
    let h = ((span_z as i32 + step - 1) / step).max(1) as u32;

    let mut out = serde_json::Map::new();
    for (tag, y) in [("upper", super::vy(-20)), ("deep", super::vy(-48))] {
        let mut img = RgbaImage::new(w, h);
        let mut counts: std::collections::HashMap<&'static str, u64> = Default::default();
        let mut total: u64 = 0;
        for pz in 0..h {
            let bz = square_center(min_z, max_z, pz, step);
            for px in 0..w {
                let bx = square_center(min_x, max_x, px, step);
                total += 1;
                // a high surface keeps the mountains-only ice gate open for VISIBILITY (see header)
                let zone = decor.zone(bx, y, bz, super::vy(200));
                let px_color = match style(zone) {
                    Some((c, name)) => {
                        *counts.entry(name).or_insert(0) += 1;
                        c
                    }
                    None => {
                        if decor.coral_zone(bx, bz) {
                            *counts.entry("coral").or_insert(0) += 1;
                            CORAL_COLOR
                        } else {
                            *counts.entry("plain").or_insert(0) += 1;
                            Rgba([0, 0, 0, 0])
                        }
                    }
                };
                img.put_pixel(px, pz, px_color);
            }
        }
        let path = format!("{}-{tag}.png", prefix.display());
        img.save(&path).map_err(|e| format!("write {path}: {e}"))?;
        let mut stats = serde_json::Map::new();
        for (name, n) in counts {
            let pct = (n as f64) * 100.0 / (total as f64);
            stats.insert(
                name.to_string(),
                serde_json::json!((pct * 10.0).round() / 10.0),
            );
        }
        stats.insert("_file".into(), serde_json::json!(path));
        out.insert(tag.to_string(), serde_json::Value::Object(stats));
    }
    out.insert(
        "_bbox_blocks".into(),
        serde_json::json!([min_x, min_z, max_x, max_z]),
    );
    println!("ZONEMAP {}", serde_json::Value::Object(out));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::square_center;

    #[test]
    fn samples_stay_inside_the_bbox() {
        // A one-block bbox with a 512-block step samples that block, not one 256 blocks away.
        assert_eq!(square_center(10, 10, 0, 512), 10);
        // Full squares keep their center; the cut-short last one uses what is left of it.
        assert_eq!(square_center(0, 99, 0, 64), 32);
        assert_eq!(square_center(0, 99, 1, 64), 82);
        // 46 blocks at step 16 is three squares, the last one 14 wide.
        for i in 0..3 {
            let c = square_center(-5, 40, i, 16);
            assert!((-5..=40).contains(&c), "square {i} sampled at {c}");
        }
    }
}
