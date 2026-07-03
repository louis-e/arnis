//! Region-aware tree library: loads a realm pack + vanilla-plus and picks a schematic per cell. Seam-safe.

use std::collections::HashMap;

use serde::Deserialize;

use crate::land_cover::coord_hash;
use crate::trees::schematic::{load_schem, Schematic};
use crate::trees::tree_library::{size_for_height, SizeFilter, TreeSize};
use crate::trees::tree_pack::TreePackSource;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Habitat {
    Conifer,
    Wet,
    Lowland,
    Dry,
    Tropical,
}

impl Habitat {
    fn parse(s: &str) -> Habitat {
        match s {
            "conifer" => Habitat::Conifer,
            "wet" => Habitat::Wet,
            "dry" => Habitat::Dry,
            "tropical" => Habitat::Tropical,
            _ => Habitat::Lowland,
        }
    }
}

// region.json manifest (serde). Variants are split by trunk-width class: w1 thin .. w3 wide.
#[derive(Deserialize)]
struct MSpecies {
    #[serde(default)]
    name: String,
    #[serde(default)]
    w1: Vec<String>,
    #[serde(default)]
    w2: Vec<String>,
    #[serde(default)]
    w3: Vec<String>,
}

/// Palm genera, excluded outside the subtropics (so e.g. New York gets no palms).
fn is_palm(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "palm",
        "cocos",
        "roystonea",
        "sabal",
        "acrocomia",
        "phoenix",
        "washingtonia",
        "borassus",
        "elaeis",
    ]
    .iter()
    .any(|p| n.contains(p))
}

// Cumulative width-class weights (percent): W1=78, then to 96 for W2, rest (4) for W3.
const WIDTH_W1: u64 = 78;
const WIDTH_W2: u64 = 96;
fn default_density() -> u32 {
    20
}
#[derive(Deserialize)]
struct MCommunity {
    name: String,
    habitat: String,
    species: Vec<MSpecies>,
    #[serde(default = "default_density")]
    density: u32,
}
#[derive(Deserialize)]
struct MRegion {
    realm: String,
    default_community: String,
    communities: Vec<MCommunity>,
}

struct Community {
    name: String,
    habitat: Habitat,
    species: Vec<Vec<usize>>,
    density: u32,
}

struct Pack {
    communities: Vec<Community>,
    default_idx: usize,
    by_habitat: HashMap<Habitat, Vec<usize>>,
}

impl Pack {
    fn is_empty(&self) -> bool {
        self.communities.is_empty()
    }
}

pub struct RegionLibrary {
    realm: String,
    entries: Vec<(Schematic, TreeSize, u8)>,
    realm_pack: Pack,
    vanilla_pack: Pack,
    scale: f64,
    ground_level: i32,
    sizes: SizeFilter,
    total_realm: usize,
    total_vanilla: usize,
}

// Metres above the selection's lowest point at which a cell counts as montane.
const MONTANE_METRES: f64 = 450.0;

/// Load a manifest's files into `entries` via `read_file`, returning the built `Pack`.
fn load_pack(
    m: &MRegion,
    read_file: &dyn Fn(&str) -> Option<Vec<u8>>,
    entries: &mut Vec<(Schematic, TreeSize, u8)>,
    exclude_palms: bool,
) -> Pack {
    let mut communities: Vec<Community> = Vec::new();
    for mc in &m.communities {
        let mut species: Vec<Vec<usize>> = Vec::new();
        for sp in &mc.species {
            if exclude_palms && is_palm(&sp.name) {
                continue;
            }
            let mut idxs: Vec<usize> = Vec::new();
            for (rels, wclass) in [(&sp.w1, 1u8), (&sp.w2, 2u8), (&sp.w3, 3u8)] {
                for rel in rels {
                    let Some(bytes) = read_file(rel) else {
                        continue;
                    };
                    if let Ok(schem) = load_schem(&bytes) {
                        if schem.has_leaves() {
                            let size = size_for_height(schem.height);
                            entries.push((schem, size, wclass));
                            idxs.push(entries.len() - 1);
                        }
                    }
                }
            }
            if !idxs.is_empty() {
                species.push(idxs);
            }
        }
        if !species.is_empty() {
            communities.push(Community {
                name: mc.name.clone(),
                habitat: Habitat::parse(&mc.habitat),
                species,
                density: mc.density,
            });
        }
    }
    let default_idx = communities
        .iter()
        .position(|c| c.name == m.default_community)
        .or_else(|| {
            communities
                .iter()
                .position(|c| c.habitat == Habitat::Lowland)
        })
        .unwrap_or(0);
    let mut by_habitat: HashMap<Habitat, Vec<usize>> = HashMap::new();
    for (i, c) in communities.iter().enumerate() {
        by_habitat.entry(c.habitat).or_default().push(i);
    }
    Pack {
        communities,
        default_idx,
        by_habitat,
    }
}

impl RegionLibrary {
    /// Load the realm pack from `source` (its `region.json`) plus the vanilla-plus sprinkle.
    pub fn load(
        source: &TreePackSource,
        scale: f64,
        ground_level: i32,
        sizes: SizeFilter,
        exclude_palms: bool,
    ) -> Result<RegionLibrary, String> {
        let mbytes = source
            .realm_manifest()
            .ok_or_else(|| "region: missing region.json".to_string())?;
        let m: MRegion = serde_json::from_slice(&mbytes)
            .map_err(|e| format!("region: parse region.json: {e}"))?;

        let mut entries: Vec<(Schematic, TreeSize, u8)> = Vec::new();
        let realm_read = |rel: &str| source.realm_file(rel).map(|c| c.into_owned());
        let realm_pack = load_pack(&m, &realm_read, &mut entries, exclude_palms);
        if realm_pack.is_empty() {
            return Err("region: no usable trees in realm pack".to_string());
        }
        let total_realm = entries.len();

        let mut vanilla_pack = Pack {
            communities: Vec::new(),
            default_idx: 0,
            by_habitat: HashMap::new(),
        };
        if m.realm != "vnplus" {
            if let Some(vbytes) = source.vanilla_manifest() {
                if let Ok(vm) = serde_json::from_slice::<MRegion>(&vbytes) {
                    let vanilla_read = |rel: &str| source.vanilla_file(rel).map(|c| c.into_owned());
                    vanilla_pack = load_pack(&vm, &vanilla_read, &mut entries, exclude_palms);
                }
            }
        }
        let total_vanilla = entries.len() - total_realm;

        Ok(RegionLibrary {
            realm: m.realm,
            entries,
            realm_pack,
            vanilla_pack,
            scale,
            ground_level,
            sizes,
            total_realm,
            total_vanilla,
        })
    }

    fn is_montane(&self, elev_y: i32) -> bool {
        let blocks_above = f64::from(elev_y - self.ground_level);
        blocks_above / self.scale.max(0.001) > MONTANE_METRES
    }

    pub fn schem(&self, idx: usize) -> &Schematic {
        &self.entries[idx].0
    }

    /// The size tier wanted at this cell, by the scale band. Tall rare, Giant only at 1:1.
    fn size_pick(&self, x: i32, z: i32) -> TreeSize {
        let roll = coord_hash(x + 101, z + 233) % 1000;
        if self.scale < 0.3 {
            if roll < 650 {
                TreeSize::Small
            } else if roll < 985 {
                TreeSize::Medium
            } else {
                TreeSize::Big
            }
        } else if self.scale < 0.7 {
            if roll < 380 {
                TreeSize::Small
            } else if roll < 820 {
                TreeSize::Medium
            } else if roll < 985 {
                TreeSize::Big
            } else {
                TreeSize::Tall
            }
        } else if self.scale < 1.0 {
            if roll < 260 {
                TreeSize::Small
            } else if roll < 700 {
                TreeSize::Medium
            } else if roll < 930 {
                TreeSize::Big
            } else {
                TreeSize::Tall
            }
        } else if roll < 200 {
            TreeSize::Small
        } else if roll < 600 {
            TreeSize::Medium
        } else if roll < 880 {
            TreeSize::Big
        } else if roll < 975 {
            TreeSize::Tall
        } else {
            TreeSize::Giant
        }
    }

    /// Whether a size may appear: the UI tier toggle AND a scale gate (Giant only at 1:1).
    fn size_allowed(&self, size: TreeSize) -> bool {
        if !self.sizes.allows(size) {
            return false;
        }
        match size {
            TreeSize::Giant => self.scale >= 1.0,
            _ => true,
        }
    }

    /// Pick one variant from a community, honoring the size filter (falls back to any size if none fit).
    fn pick_in_community(&self, c: &Community, x: i32, z: i32) -> Option<usize> {
        let allowed_count = |sp: &Vec<usize>| {
            sp.iter()
                .filter(|&&i| self.size_allowed(self.entries[i].1))
                .count()
        };
        let total: usize = c.species.iter().map(&allowed_count).sum();
        if total == 0 {
            let any: usize = c.species.iter().map(Vec::len).sum();
            if any == 0 {
                return None;
            }
            let mut r = (coord_hash(x + 31, z + 57) % any as u64) as usize;
            for sp in &c.species {
                if r < sp.len() {
                    let h = coord_hash(x + 313, z + 727) as usize;
                    return Some(sp[h % sp.len()]);
                }
                r -= sp.len();
            }
            return None;
        }
        let mut r = (coord_hash(x + 31, z + 57) % total as u64) as usize;
        let mut chosen: &Vec<usize> = &c.species[0];
        for sp in &c.species {
            let w = allowed_count(sp);
            if r < w {
                chosen = sp;
                break;
            }
            r -= w;
        }
        let want = self.size_pick(x, z);
        let allowed: Vec<usize> = chosen
            .iter()
            .copied()
            .filter(|&i| self.size_allowed(self.entries[i].1))
            .collect();
        if allowed.is_empty() {
            return None;
        }
        let roll = coord_hash(x + 5, z + 11) % 100;
        let target_wc: u8 = if roll < WIDTH_W1 {
            1
        } else if roll < WIDTH_W2 {
            2
        } else {
            3
        };
        let mut group: Vec<usize> = Vec::new();
        for wc in (1..=target_wc).rev() {
            group = allowed
                .iter()
                .copied()
                .filter(|&i| self.entries[i].2 == wc)
                .collect();
            if !group.is_empty() {
                break;
            }
        }
        if group.is_empty() {
            group = allowed;
        }
        let of_want: Vec<usize> = group
            .iter()
            .copied()
            .filter(|&i| self.entries[i].1 == want)
            .collect();
        let pool: &[usize] = if of_want.is_empty() { &group } else { &of_want };
        if pool.is_empty() {
            return None;
        }
        let h = coord_hash(x + 313, z + 727) as usize;
        Some(pool[h % pool.len()])
    }

    /// Choose a community for `habitat_hint`; on a montane cell lowland/wet swap to conifer.
    fn pick_community<'a>(
        &self,
        pack: &'a Pack,
        hint: Habitat,
        x: i32,
        z: i32,
        montane: bool,
    ) -> &'a Community {
        let eff_hint = if montane && matches!(hint, Habitat::Lowland | Habitat::Wet) {
            Habitat::Conifer
        } else {
            hint
        };
        let cand = pack
            .by_habitat
            .get(&eff_hint)
            .filter(|v| !v.is_empty())
            .or_else(|| pack.by_habitat.get(&hint).filter(|v| !v.is_empty()));
        let idx = match cand {
            Some(v) => {
                let n = crate::ground_generation::value_noise_01(x, z, 160);
                let k = ((n * v.len() as f64) as usize).min(v.len() - 1);
                v[k]
            }
            None => pack.default_idx,
        };
        &pack.communities[idx]
    }

    fn base_spacing(&self) -> i32 {
        // Wider schem-pack canopies need more spacing to avoid overcrowded forests.
        if self.scale < 0.3 {
            7
        } else if self.scale < 0.7 {
            6
        } else {
            5
        }
    }

    /// WorldPainter community density (~10..150) -> fraction of slots that keep a tree.
    fn keep_prob(density: u32) -> f64 {
        (0.34 + density as f64 / 90.0).clamp(0.30, 1.0)
    }

    const GROVE_PERIOD: i32 = 22;

    /// Pick the trunk slot + schematic for a candidate cell, or `None` for a clearing.
    pub fn pick_slot(
        &self,
        x: i32,
        z: i32,
        hint: Habitat,
        elev_y: i32,
    ) -> Option<(i32, i32, usize, u8)> {
        let s = self.base_spacing();
        let (sx, sz) = crate::trees::schematic::trunk_slot_s(x, z, s);
        let montane =
            self.is_montane(elev_y) && crate::ground_generation::value_noise_01(sx, sz, 64) < 0.6;
        let blend = coord_hash(sx + 7, sz + 13) % 100;
        let (community, idx): (&Community, Option<usize>) = if blend >= 97 {
            let n = self.realm_pack.communities.len();
            if n == 0 {
                return None;
            }
            let ci = (coord_hash(sx + 5, sz + 9) % n as u64) as usize;
            let c = &self.realm_pack.communities[ci];
            (c, self.pick_in_community(c, sx, sz))
        } else if blend >= 67 && !self.vanilla_pack.is_empty() {
            let c = self.pick_community(&self.vanilla_pack, hint, sx, sz, montane);
            (c, self.pick_in_community(c, sx, sz))
        } else {
            let c = self.pick_community(&self.realm_pack, hint, sx, sz, montane);
            (c, self.pick_in_community(c, sx, sz))
        };
        let grove = crate::ground_generation::value_noise_01(sx, sz, Self::GROVE_PERIOD);
        let jitter = (coord_hash(sx ^ 0x71c3, sz ^ 0x2d9b) % 1000) as f64 / 1000.0;
        if grove * 0.82 + jitter * 0.18 >= Self::keep_prob(community.density) {
            return None;
        }
        let idx = idx?;
        let rot = (coord_hash(sx ^ 0x5bd1, sz ^ 0x9e37) % 4) as u8;
        Some((sx, sz, idx, rot))
    }

    pub fn report(&self) {
        let (mut s, mut m, mut b, mut t, mut g) = (0u32, 0u32, 0u32, 0u32, 0u32);
        for (_, size, _) in &self.entries {
            match size {
                TreeSize::Small => s += 1,
                TreeSize::Medium => m += 1,
                TreeSize::Big => b += 1,
                TreeSize::Tall => t += 1,
                TreeSize::Giant => g += 1,
            }
        }
        let on = |v: bool| if v { "on" } else { "off" };
        println!(
            "Region tree pack loaded: realm {} - {} regional trees ({} communities) + {} vanilla sprinkle trees ({} communities)",
            self.realm,
            self.total_realm,
            self.realm_pack.communities.len(),
            self.total_vanilla,
            self.vanilla_pack.communities.len(),
        );
        println!(
            "  size tiers [schems]: small {} [{}], medium {} [{}], big {} [{}], tall {} [{}], giant {} [{}]",
            s, on(self.sizes.small),
            m, on(self.sizes.medium),
            b, on(self.sizes.big),
            t, on(self.sizes.tall),
            g, on(self.sizes.giant),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn habitat_parse() {
        assert_eq!(Habitat::parse("conifer"), Habitat::Conifer);
        assert_eq!(Habitat::parse("anything"), Habitat::Lowland);
    }

    #[test]
    fn palm_detection() {
        assert!(is_palm("Cocos_nucifera"));
        assert!(is_palm("Roystonea_regia"));
        assert!(!is_palm("Quercus_alba"));
    }

    #[test]
    fn ena_palm_gate_removes_trees() {
        let src = TreePackSource::embedded("ena");
        let incl = RegionLibrary::load(&src, 1.0, -62, SizeFilter::default(), false).unwrap();
        let excl = RegionLibrary::load(&src, 1.0, -62, SizeFilter::default(), true).unwrap();
        assert!(
            excl.total_realm < incl.total_realm,
            "palm gate should drop ena palm trees ({} vs {})",
            excl.total_realm,
            incl.total_realm
        );
    }

    #[test]
    fn embedded_eur_loads_and_picks() {
        let src = TreePackSource::embedded("eur");
        let lib =
            RegionLibrary::load(&src, 1.0, -62, SizeFilter::default(), false).expect("load eur");
        assert!(lib.total_realm > 0);
        for k in 0..200 {
            if let Some((_, _, idx, _)) = lib.pick_slot(k * 3, k * 7, Habitat::Lowland, 0) {
                assert!(idx < lib.entries.len());
            }
        }
    }
}
