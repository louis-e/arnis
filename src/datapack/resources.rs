//! The extras' share of the world resource pack: the logo at the top of the pause menu
//! dialog, drawn as one glyph of a bitmap font.

use super::dialogs::BANNER_KEY;
use crate::mapillary::displays::{
    install_world_pack, is_arnis_pack, PACK_MARKER, RESOURCEPACK_FORMAT,
};
use serde_json::json;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const BANNER_PNG: &[u8] = include_bytes!("../../assets/minecraft/menu_banner.png");
const GLYPH: char = '\u{e000}';
/// Rendered height in GUI pixels; the image is 256 by 76.
const GLYPH_HEIGHT: u32 = 40;

/// The font `BANNER_FONT` names, and the texture its provider reads.
const FONT_PATH: &str = "assets/arnis/font/banner.json";
const TEXTURE_PATH: &str = "assets/arnis/textures/font/banner.png";
const LANG_PATH: &str = "assets/arnis/lang/en_us.json";

/// Adds the banner to `<world>/resources.zip`: into the pack Arnis already wrote there, or
/// into a new one. A pack the user put there is left alone, and the menu shows no banner.
pub fn install_banner(world: &Path) -> Result<(), String> {
    let path = world.join("resources.zip");
    if path.exists() && !is_arnis_pack(&path) {
        return Ok(());
    }
    let staged = path.with_extension("zip.tmp");
    let written = write_pack(&path, &staged);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    install_world_pack(&path, &staged)
}

fn write_pack(existing: &Path, staged: &Path) -> Result<(), String> {
    let err = |e: zip::result::ZipError| e.to_string();
    let file = File::create(staged).map_err(|e| format!("create {}: {e}", staged.display()))?;
    let mut zip = ZipWriter::new(BufWriter::new(file));
    let deflated = FileOptions::default().compression_method(CompressionMethod::Deflated);

    if existing.exists() {
        // The facade pack can be hundreds of megabytes: its entries are copied compressed.
        let source = File::open(existing).map_err(|e| e.to_string())?;
        let mut old = ZipArchive::new(source).map_err(err)?;
        for i in 0..old.len() {
            let entry = old.by_index_raw(i).map_err(err)?;
            if [FONT_PATH, TEXTURE_PATH, LANG_PATH].contains(&entry.name()) {
                continue;
            }
            zip.raw_copy_file(entry).map_err(err)?;
        }
    } else {
        let mcmeta = json!({
            "pack": {
                "pack_format": RESOURCEPACK_FORMAT,
                "supported_formats": [RESOURCEPACK_FORMAT, 999],
                "min_format": RESOURCEPACK_FORMAT,
                "max_format": [999, 0],
                "description": "Arnis",
            }
        });
        zip.start_file("pack.mcmeta", deflated).map_err(err)?;
        zip.write_all(mcmeta.to_string().as_bytes())
            .map_err(|e| e.to_string())?;
        zip.start_file(PACK_MARKER, deflated).map_err(err)?;
        zip.write_all(b"arnis world pack\n")
            .map_err(|e| e.to_string())?;
    }

    let font = json!({
        "providers": [{
            "type": "bitmap",
            "file": "arnis:font/banner.png",
            "height": GLYPH_HEIGHT,
            "ascent": GLYPH_HEIGHT,
            "chars": [GLYPH.to_string()],
        }]
    });
    // Four empty lines above the glyph give its 40 pixels the room of five text lines.
    let lang = json!({ BANNER_KEY: format!("\n\n\n\n{GLYPH}") });
    for (path, bytes, opts) in [
        (FONT_PATH, font.to_string().into_bytes(), deflated),
        (
            TEXTURE_PATH,
            BANNER_PNG.to_vec(),
            FileOptions::default().compression_method(CompressionMethod::Stored),
        ),
        (LANG_PATH, lang.to_string().into_bytes(), deflated),
    ] {
        zip.start_file(path, opts).map_err(err)?;
        zip.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    let mut out = zip.finish().map_err(err)?;
    out.flush().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn names(path: &Path) -> Vec<String> {
        let mut zip = ZipArchive::new(File::open(path).unwrap()).unwrap();
        (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect()
    }

    #[test]
    fn a_world_without_a_pack_gets_one_with_the_banner() {
        let dir = tempfile::tempdir().unwrap();
        install_banner(dir.path()).unwrap();
        let pack = dir.path().join("resources.zip");
        assert!(is_arnis_pack(&pack));
        let names = names(&pack);
        for want in ["pack.mcmeta", FONT_PATH, TEXTURE_PATH, LANG_PATH] {
            assert!(names.iter().any(|n| n == want), "{want}");
        }
        let mut zip = ZipArchive::new(File::open(&pack).unwrap()).unwrap();
        let mut font = String::new();
        zip.by_name(FONT_PATH)
            .unwrap()
            .read_to_string(&mut font)
            .unwrap();
        assert!(font.contains("arnis:font/banner.png"), "{font}");
    }

    #[test]
    fn an_arnis_pack_keeps_its_files_and_the_banner_is_not_added_twice() {
        let dir = tempfile::tempdir().unwrap();
        install_banner(dir.path()).unwrap();
        install_banner(dir.path()).unwrap();
        let names = names(&dir.path().join("resources.zip"));
        assert_eq!(names.iter().filter(|n| *n == FONT_PATH).count(), 1);
        assert_eq!(names.iter().filter(|n| *n == "pack.mcmeta").count(), 1);
    }

    #[test]
    fn a_pack_the_user_made_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let pack = dir.path().join("resources.zip");
        let mut zip = ZipWriter::new(File::create(&pack).unwrap());
        zip.start_file("pack.mcmeta", FileOptions::default())
            .unwrap();
        zip.write_all(b"{}").unwrap();
        zip.finish().unwrap();
        install_banner(dir.path()).unwrap();
        assert_eq!(names(&pack), vec!["pack.mcmeta".to_string()]);
    }
}
