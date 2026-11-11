//! `--tree-pack-dir`: the user's own schematics, laid out like the bundled packs as
//! `<realm>/<community>/<tree type>/[<size>/]<file>.schem`.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::trees::schematic::{load_schem, load_schem_within, Schematic};
use crate::trees::size_weights::NAMES;
use crate::trees::tree_library::{size_for_height, TreeSize};
use crate::trees::tree_pack::{embedded_read, REALMS};

/// Whether the folder's trees join a realm's bundled ones or stand in for them.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum TreePackMode {
    /// User trees join the bundled ones.
    #[default]
    Add,
    /// A realm with any user tree uses only the user's trees.
    Replace,
}

/// Manifest paths of user files carry this prefix, so the pack reads them from disk.
pub const USER: &str = "user:";

/// Bigger files are skipped: a tree model is a few KB, the largest bundled one 0.2 MB.
const MAX_FILE_BYTES: u64 = 8 << 20;
/// Widest model: a tree stamps around its middle, so the halo of a One World piece must hold half of it.
const MAX_SIDE: i32 = 2 * crate::one_world::PIECE_HALO_BLOCKS;
const MAX_HEIGHT: i32 = 256;

struct UserTree {
    realm: String,
    community: String,
    species: String,
    /// Path under the root, `/`-separated.
    rel: String,
}

/// A scanned folder: the usable trees, sorted by path, and what was skipped.
pub struct PackDir {
    root: PathBuf,
    replace: bool,
    trees: Vec<UserTree>,
    /// One line per skipped file.
    pub skipped: Vec<String>,
}

fn manifest(realm: &str) -> Option<Value> {
    serde_json::from_slice(&embedded_read(&format!("{realm}/region.json"))?).ok()
}

fn communities(m: &Value) -> impl Iterator<Item = &Value> {
    m["communities"].as_array().into_iter().flatten()
}

fn species_files(sp: &Value) -> impl Iterator<Item = &str> {
    ["w1", "w2", "w3"]
        .into_iter()
        .flat_map(|w| sp[w].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
}

/// The size a folder name stands for.
pub fn tier_named(name: &str) -> Option<TreeSize> {
    clap::ValueEnum::from_str(name, true).ok()
}

/// The size a manifest path's folder sets, for user files in a size folder.
pub fn tier_of(rel: &str) -> Option<TreeSize> {
    // realm/community/species/size/file: the size folder is the fourth part.
    let parts: Vec<&str> = rel.strip_prefix(USER)?.split('/').collect();
    (parts.len() == 5).then(|| tier_named(parts[3])).flatten()
}

/// Every file under `dir`, recursively.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => walk(&path, out),
            Ok(_) => out.push(path),
            Err(_) => {}
        }
    }
}

/// A user file's bytes, refused past `MAX_FILE_BYTES`.
fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("over the {} MB limit", MAX_FILE_BYTES >> 20));
    }
    Ok(bytes)
}

/// A user tree, within the size limits.
pub fn load_user(bytes: &[u8]) -> Result<Schematic, String> {
    load_schem_within(bytes, MAX_SIDE, MAX_HEIGHT)
}

/// Why a schematic file cannot be used, or the size tier its height gives.
fn check(path: &Path) -> Result<TreeSize, String> {
    let schem = load_user(&read_capped(path)?)?;
    if !schem.has_leaves() {
        return Err("no leaves".into());
    }
    Ok(size_for_height(schem.height))
}

impl PackDir {
    /// Reads `root`, checking every `.schem` / `.schematic` file with the tree loader.
    /// Files are taken in path order, so every machine picks the same trees.
    pub fn scan(root: &Path, mode: TreePackMode) -> PackDir {
        let mut dir = PackDir {
            root: root.to_path_buf(),
            replace: mode == TreePackMode::Replace,
            trees: Vec::new(),
            skipped: Vec::new(),
        };
        if !root.is_dir() {
            dir.skipped
                .push(format!("{}: no such folder", root.display()));
            return dir;
        }
        let mut files = Vec::new();
        walk(root, &mut files);
        let mut rels: Vec<(String, PathBuf)> = files
            .into_iter()
            .filter(|p| {
                p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    e.eq_ignore_ascii_case("schem") || e.eq_ignore_ascii_case("schematic")
                })
            })
            .filter_map(|p| {
                let rel = p.strip_prefix(root).ok()?.to_str()?.replace('\\', "/");
                Some((rel, p))
            })
            .collect();
        rels.sort();
        let mut manifests: Vec<(&str, Option<Value>)> = Vec::new();
        for (rel, path) in rels {
            match dir.place(&rel, &path, &mut manifests) {
                Ok(tree) => dir.trees.push(tree),
                Err(why) => dir.skipped.push(format!("{rel}: {why}")),
            }
        }
        dir
    }

    fn place(
        &self,
        rel: &str,
        path: &Path,
        manifests: &mut Vec<(&'static str, Option<Value>)>,
    ) -> Result<UserTree, String> {
        let parts: Vec<&str> = rel.split('/').collect();
        if !(4..=5).contains(&parts.len()) {
            return Err("not in a realm/community/tree type folder".into());
        }
        if parts.len() == 5 && tier_named(parts[3]).is_none() {
            return Err(format!("{} is not a size folder", parts[3]));
        }
        let realm = REALMS[1..]
            .iter()
            .find(|r| r.eq_ignore_ascii_case(parts[0]))
            .ok_or_else(|| format!("{} is not a realm", parts[0]))?;
        let at = match manifests.iter().position(|(r, _)| r == realm) {
            Some(i) => i,
            None => {
                manifests.push((realm, manifest(realm)));
                manifests.len() - 1
            }
        };
        let community = manifests[at]
            .1
            .as_ref()
            .and_then(|m| {
                communities(m)
                    .filter_map(|c| c["name"].as_str())
                    .find(|n| n.eq_ignore_ascii_case(parts[1]))
            })
            .ok_or_else(|| format!("{} is not a community of {realm}", parts[1]))?
            .to_string();
        check(path)?;
        Ok(UserTree {
            realm: realm.to_string(),
            community,
            species: parts[2].to_string(),
            rel: rel.to_string(),
        })
    }

    /// How many trees the folder adds.
    pub fn found(&self) -> usize {
        self.trees.len()
    }

    /// Prints one warning line per skipped file, then the count.
    pub fn report(&self) {
        for line in &self.skipped {
            eprintln!("Warning: tree-pack-dir: skipped {line}");
        }
        println!(
            "  tree-pack-dir: {} custom trees ({} skipped, {})",
            self.found(),
            self.skipped.len(),
            if self.replace { "replace" } else { "add" }
        );
    }

    /// `embedded`, the realm's bundled manifest, with the folder's trees in it,
    /// or `None` when the folder has none for the realm.
    pub fn manifest(&self, realm: &str, embedded: &[u8]) -> Option<Vec<u8>> {
        let mine: Vec<&UserTree> = self.trees.iter().filter(|t| t.realm == realm).collect();
        if mine.is_empty() {
            return None;
        }
        let mut m: Value = serde_json::from_slice(embedded).ok()?;
        let list = m["communities"].as_array_mut()?;
        if self.replace {
            for c in list.iter_mut() {
                c["species"] = json!([]);
            }
        }
        for t in mine {
            let Some(c) = list.iter_mut().find(|c| c["name"] == t.community.as_str()) else {
                continue;
            };
            let file = Value::from(format!("{USER}{}", t.rel));
            let species = c["species"].as_array_mut()?;
            match species.iter_mut().find(|s| s["name"] == t.species.as_str()) {
                Some(s) => match s["w1"].as_array_mut() {
                    Some(w1) => w1.push(file),
                    None => s["w1"] = json!([file]),
                },
                None => species.push(json!({ "name": t.species, "w1": [file] })),
            }
        }
        serde_json::to_vec(&m).ok()
    }

    /// A user file's bytes, by its path under the root.
    pub fn read(&self, rel: &str) -> Option<Vec<u8>> {
        // Only paths the scan accepted, so a manifest cannot reach outside the folder.
        self.trees
            .binary_search_by(|t| t.rel.as_str().cmp(rel))
            .ok()
            .and_then(|_| read_capped(&self.root.join(rel)).ok())
    }
}

const ROOT_README: &str = "Arnis tree pack folder
======================

Put your own tree schematics (Sponge .schem files) here, one folder per kind:

  <realm>/<community>/<tree type>/<size>/any-name.schem

- realm: the region pack (afr, asn, aus, ena, eur, fl, ind, sam, wna,
  vanilla-plus). Arnis picks it from the area's location, or Tree Realm forces one.
- community: a forest type of that realm. Only the folders that are here are
  read; a community that is not one of them is skipped.
- tree type: the species. Any folder name works; a new one adds a tree type to
  the community, and its name before the first _ is taken as the genus.
- size: optional. small (up to 6 blocks tall), medium (7-12), big (13-20),
  tall (21-28), giant (29 and up). A file outside a size folder gets the size
  its height gives.

File names are free. Files are read in path order, so every machine places the
same trees. A file that does not load, has no leaves, is over 8 MB, more than
64 blocks wide or more than 256 tall is skipped with one warning line.

Mode (--tree-pack-mode, or Mode in the window):
- add (default): your trees join the built-in ones.
- replace: a realm with any tree here uses only the trees here; realms with
  none keep the built-in trees.

Export Built-in Trees (--export-tree-packs) writes the built-in trees into this
layout to edit, leaving files already there as they are. Exported trees all count
as narrow-trunked, so in add mode they double the built-in ones; use replace
mode, or delete what you keep as is.
";

fn realm_readme(realm: &str, communities: &[&str]) -> String {
    format!(
        "Realm {realm}\n\nOne folder per community (forest type), one folder per tree type inside,\n\
         and the size folders small, medium, big, tall and giant inside that.\n\
         See ../README.txt. Communities:\n\n{}\n",
        communities
            .iter()
            .map(|c| format!("  {c}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// Writes `contents` to `path` unless a file is there already.
fn write_new(path: &Path, contents: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))
}

/// Creates the empty layout of every bundled pack under `root`, with READMEs,
/// and returns how many tree type folders it holds.
pub fn init(root: &Path) -> Result<usize, String> {
    let mkdir = |p: &Path| fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()));
    mkdir(root)?;
    write_new(&root.join("README.txt"), ROOT_README)?;
    let mut types = 0;
    for realm in &REALMS[1..] {
        let m = manifest(realm).ok_or_else(|| format!("{realm}: no bundled manifest"))?;
        let names: Vec<&str> = communities(&m).filter_map(|c| c["name"].as_str()).collect();
        let realm_dir = root.join(realm);
        mkdir(&realm_dir)?;
        write_new(&realm_dir.join("README.txt"), &realm_readme(realm, &names))?;
        for c in communities(&m) {
            let cdir = realm_dir.join(c["name"].as_str().unwrap_or_default());
            for sp in c["species"].as_array().into_iter().flatten() {
                let sdir = cdir.join(sp["name"].as_str().unwrap_or_default());
                for tier in NAMES {
                    mkdir(&sdir.join(tier))?;
                }
                types += 1;
            }
        }
    }
    Ok(types)
}

/// Writes every bundled schematic into the layout under `root`, each in the size folder
/// its height gives, and returns how many it wrote and how many it left as they were.
pub fn export(root: &Path) -> Result<(usize, usize), String> {
    init(root)?;
    let (mut written, mut kept) = (0, 0);
    for realm in &REALMS[1..] {
        let Some(m) = manifest(realm) else { continue };
        for c in communities(&m) {
            let cdir = root
                .join(realm)
                .join(c["name"].as_str().unwrap_or_default());
            for sp in c["species"].as_array().into_iter().flatten() {
                let sdir = cdir.join(sp["name"].as_str().unwrap_or_default());
                for rel in species_files(sp) {
                    let Some(bytes) = embedded_read(&format!("{realm}/{rel}")) else {
                        continue;
                    };
                    // As the pack loader: trees that never load are not exported.
                    let Ok(schem) = load_schem(&bytes) else {
                        continue;
                    };
                    if !schem.has_leaves() {
                        continue;
                    }
                    let tier = NAMES[size_for_height(schem.height) as usize];
                    let name = rel.rsplit('/').next().unwrap_or(rel);
                    let dest = sdir.join(tier).join(name);
                    // The user may have edited it.
                    if dest.exists() {
                        kept += 1;
                        continue;
                    }
                    fs::write(&dest, &bytes).map_err(|e| format!("{}: {e}", dest.display()))?;
                    written += 1;
                }
            }
        }
    }
    Ok((written, kept))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_come_from_the_size_folder_only() {
        assert_eq!(tier_of("user:eur/C/Sp/big/x.schem"), Some(TreeSize::Big));
        assert_eq!(tier_of("user:eur/C/Sp/x.schem"), None);
        assert_eq!(tier_of("user:eur/C/Sp/odd/x.schem"), None);
        assert_eq!(tier_of("4. europe/big/x.schem"), None);
    }

    #[test]
    fn export_then_init_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (written, kept) = export(root).unwrap();
        assert!(written > 2400 && kept == 0, "{written} {kept}");
        let all = PackDir::scan(root, TreePackMode::Add);
        assert_eq!(
            all.found(),
            written,
            "{:?}",
            &all.skipped[..all.skipped.len().min(5)]
        );
        assert!(all.skipped.is_empty());
        // Init and a second export on a filled folder keep every file, edited ones included.
        init(root).unwrap();
        let edited = root.join(&all.trees[0].rel);
        fs::write(&edited, b"edited").unwrap();
        assert_eq!(export(root).unwrap(), (0, written));
        assert_eq!(fs::read(&edited).unwrap(), b"edited");
    }

    #[test]
    fn bad_pack_is_skipped_with_warning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let sp = root.join("eur/EUR - Alpine forest (mature)/My_tree");
        fs::create_dir_all(sp.join("weird")).unwrap();
        fs::write(sp.join("broken.schem"), b"not a schematic").unwrap();
        fs::write(sp.join("weird/a.schem"), b"x").unwrap();
        fs::write(root.join("eur/stray.schem"), b"x").unwrap();
        fs::create_dir_all(root.join("eur/No such forest/T")).unwrap();
        fs::write(root.join("eur/No such forest/T/a.schem"), b"x").unwrap();
        fs::write(sp.join("notes.txt"), b"ignored").unwrap();
        let scan = PackDir::scan(root, TreePackMode::Add);
        assert_eq!(scan.found(), 0);
        assert_eq!(scan.skipped.len(), 4, "{:?}", scan.skipped);
        assert!(scan.skipped.iter().any(|l| l.contains("broken.schem")));
        // A missing folder is a warning too, not a failure.
        let gone = PackDir::scan(&root.join("nope"), TreePackMode::Add);
        assert_eq!((gone.found(), gone.skipped.len()), (0, 1));
    }

    #[test]
    fn manifest_adds_or_replaces_per_realm() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let sp = root.join("eur/EUR - Alpine forest (mature)/My_tree/small");
        fs::create_dir_all(&sp).unwrap();
        let bytes = embedded_read("eur/4. europe/Abies_alba1.schem").unwrap();
        fs::write(sp.join("any name.schem"), &bytes[..]).unwrap();
        let base = embedded_read("eur/region.json").unwrap();
        let files = |m: &[u8]| -> Vec<String> {
            let v: Value = serde_json::from_slice(m).unwrap();
            communities(&v)
                .flat_map(|c| c["species"].as_array().unwrap().iter())
                .flat_map(|s| species_files(s).map(str::to_string).collect::<Vec<_>>())
                .collect()
        };
        let n_base = files(&base).len();

        let add = PackDir::scan(root, TreePackMode::Add);
        assert_eq!(add.found(), 1);
        assert!(add.manifest("ena", &base).is_none());
        let added = files(&add.manifest("eur", &base).unwrap());
        assert_eq!(added.len(), n_base + 1);
        let user = "user:eur/EUR - Alpine forest (mature)/My_tree/small/any name.schem";
        assert!(added.iter().any(|f| f == user));
        assert_eq!(tier_of(user), Some(TreeSize::Small));
        assert!(add.read(&user[USER.len()..]).is_some());
        assert!(add.read("../escape.schem").is_none());

        let replace = PackDir::scan(root, TreePackMode::Replace);
        assert_eq!(files(&replace.manifest("eur", &base).unwrap()), vec![user]);
    }
}
