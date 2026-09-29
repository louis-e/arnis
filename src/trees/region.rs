//! Region-aware tree library: loads a realm pack + vanilla-plus and picks a schematic per cell. Seam-safe.

use std::collections::HashMap;

use serde::Deserialize;

use crate::ecoregion::{EcoBiome, Ecoregion};
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

/// Palm genera in the packs, gated by `ecoregion::palms_belong`.
const PALM_GENERA: &[&str] = &[
    "Acrocomia",
    "Archontophoenix",
    "Areca",
    "Astrocarym",
    "Attalea",
    "Beccariophoenix",
    "Bismarckia",
    "Borassus",
    "Calyptronoma",
    "Ceroxylon",
    "Cocos",
    "Cyrtostachys",
    "Elaeis",
    "Euterpe",
    "Hyphaene",
    "Jubaea",
    "Livistona",
    "Mauritia",
    "Nypa",
    "Phoenix",
    "Raphia",
    "Rhopalostylis",
    "Roystonea",
    "Sabal",
    "Serenoa",
    "Socratea",
    "Washingtonia",
];

fn is_palm(name: &str) -> bool {
    let genus = name.split('_').next().unwrap_or(name);
    PALM_GENERA.iter().any(|g| g.eq_ignore_ascii_case(genus))
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

#[derive(Clone)]
struct Community {
    name: String,
    habitat: Habitat,
    species: Vec<Vec<usize>>,
    /// Name and genus of each entry in `species`.
    names: Vec<String>,
    genera: Vec<String>,
    density: u32,
}

struct Pack {
    communities: Vec<Community>,
    /// The manifest's own communities; ecoregion mixes append theirs after them.
    own: usize,
    default_idx: usize,
    by_habitat: HashMap<Habitat, Vec<usize>>,
}

impl Pack {
    fn is_empty(&self) -> bool {
        self.communities.is_empty()
    }
}

/// Where in an ecoregion a mix entry grows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Niche {
    Any,
    Montane,
    Wet,
}

const PLAIN: usize = 0;
const MONTANE: usize = 1;
const WET: usize = 2;
const UNTAGGED: usize = 0;
const CONIFER: usize = 1;
const BROADLEAF: usize = 2;

/// A resolved mix entry: community, weight, niche.
type MixItem = (usize, u32, Niche);

/// Weighted communities by [plain, montane, wet] x [untagged, conifer, broadleaf].
type Pools = [[Vec<(usize, u32)>; 3]; 3];

/// One ecoregion's pools, resolved at load so a slot pick allocates nothing.
struct EcoMix {
    pools: Pools,
    vanilla: Pools,
    /// Every community in the mix, for a mapped tree's genus.
    communities: Vec<usize>,
    /// The mix's palms, for trees along a sand beach where palms belong.
    beach: Option<usize>,
    palms: bool,
}

/// What the slot asks of a mix, beyond its ecoregion.
#[derive(Clone, Copy)]
struct Want {
    /// Leaf type or wetland the tags asked for; `None` leaves it to the mix.
    tagged: Option<Habitat>,
    wet: bool,
    montane: bool,
}

impl Want {
    fn pool(self) -> (usize, usize) {
        let place = if self.montane {
            MONTANE
        } else if self.wet {
            WET
        } else {
            PLAIN
        };
        let tag = match self.tagged {
            Some(Habitat::Conifer) => CONIFER,
            Some(Habitat::Lowland) => BROADLEAF,
            _ => UNTAGGED,
        };
        (place, tag)
    }
}

/// Pools from mix entries: niche entries first, then the mix's conifer or wet trees, then all.
fn build_pools(
    entries: &[MixItem],
    communities: &[Community],
    palm: &[bool],
    palms: bool,
) -> Pools {
    type Pool = Vec<(usize, u32)>;
    let habitat = |c: usize| communities[c].habitat;
    let conifer = |c: usize| habitat(c) == Habitat::Conifer;
    let all_palm = |c: usize| communities[c].species.iter().flatten().all(|&i| palm[i]);
    let with = |keep: &dyn Fn(&MixItem) -> bool| -> Pool {
        entries
            .iter()
            .filter(|e| keep(e))
            .map(|&(c, w, _)| (c, w))
            .collect()
    };
    let only = |pool: &Pool, keep: &dyn Fn(usize) -> bool| -> Pool {
        pool.iter().copied().filter(|&(c, _)| keep(c)).collect()
    };
    let or = |a: Pool, b: Pool| if a.is_empty() { b } else { a };
    let plain = or(with(&|e| e.2 == Niche::Any), with(&|_| true));
    let montane = or(
        with(&|e| e.2 == Niche::Montane),
        or(only(&plain, &conifer), plain.clone()),
    );
    let wet = or(
        with(&|e| e.2 == Niche::Wet),
        or(only(&plain, &|c| habitat(c) == Habitat::Wet), plain.clone()),
    );
    // Tags go by species, so a stone pine in a Mediterranean mix answers a conifer tag.
    let grows = |c: usize, conifer: bool| {
        communities[c]
            .genera
            .iter()
            .any(|g| crate::trees::mapped::is_conifer_genus(g) == conifer)
    };
    let all_conifers = or(with(&|e| conifer(e.0)), with(&|e| grows(e.0, true)));
    [plain, montane, wet].map(|pool| {
        let pool = if palms {
            pool
        } else {
            or(only(&pool, &|c| !all_palm(c)), pool)
        };
        let conifers = or(
            or(only(&pool, &conifer), only(&pool, &|c| grows(c, true))),
            all_conifers.clone(),
        );
        let broadleaf = or(
            only(&pool, &|c| !conifer(c) && grows(c, false)),
            or(only(&pool, &|c| grows(c, false)), pool.clone()),
        );
        [pool, conifers, broadleaf]
    })
}

/// What a caller already knows about the slot it is asking for.
#[derive(Clone, Copy, Default)]
pub struct SlotRequest {
    /// Size tier from a measured canopy height, when there is one.
    pub want_size: Option<TreeSize>,
    /// The caller fixed the density itself, so skip the pack's grove noise.
    pub density_decided: bool,
    /// Ecoregion at the slot; its mix picks the community instead of the hint.
    pub eco: Option<Ecoregion>,
    /// The hint comes from tags (leaf type, wetland), not a random species roll.
    pub tagged: bool,
    /// Wetland or mangrove ground, where the mix's swamp and riparian trees grow.
    pub wet_ground: bool,
    /// Beside a sand beach, where the mix's palms line the shore.
    pub beach: bool,
}

/// What OSM says about a mapped tree.
#[derive(Clone, Copy, Default)]
pub struct MappedRequest<'a> {
    pub genus: Option<&'a str>,
    pub conifer: Option<bool>,
    pub want_size: Option<TreeSize>,
    pub eco: Option<Ecoregion>,
    pub beach: bool,
}

pub struct RegionLibrary {
    realm: String,
    /// Pack directory, as ecoregion mixes name it.
    code: String,
    entries: Vec<(Schematic, TreeSize, u8)>,
    /// Whether each entry is a palm.
    palm: Vec<bool>,
    realm_pack: Pack,
    vanilla_pack: Pack,
    scale: f64,
    ground_level: i32,
    blocks_per_meter: f64,
    sizes: SizeFilter,
    total_realm: usize,
    total_vanilla: usize,
    /// Palms were left out at load, so mixes leave them out too.
    palms_stripped: bool,
    /// Palms where no ecoregion says otherwise, by the area's latitude.
    palms_default: bool,
    mixes: HashMap<u16, EcoMix>,
}

// Metres above the selection's lowest point at which a cell counts as montane.
const MONTANE_METRES: f64 = 450.0;
/// Blocks across one grove of an ecoregion's community.
const GROVE_BLOCKS: i32 = 48;
/// Percent of beach-side trees that are palms, where the mix has any.
const BEACH_PALMS: u64 = 70;
const SALT_MIX: u32 = 0x3C0_A11E;
const SALT_VANILLA: u32 = 0x3C0_B22F;

/// One manifest community's trees, minus `excluded` species or genera.
fn build_community(
    mc: &MCommunity,
    read_file: &dyn Fn(&str) -> Option<Vec<u8>>,
    entries: &mut Vec<(Schematic, TreeSize, u8)>,
    exclude_palms: bool,
    excluded: &[&str],
) -> Option<Community> {
    let mut species: Vec<Vec<usize>> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut genera: Vec<String> = Vec::new();
    for sp in &mc.species {
        let genus = sp.name.split('_').next().unwrap_or_default();
        if (exclude_palms && is_palm(&sp.name))
            || excluded.iter().any(|&x| x == sp.name || x == genus)
        {
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
            names.push(sp.name.clone());
            genera.push(genus.to_string());
        }
    }
    (!species.is_empty()).then(|| Community {
        name: mc.name.clone(),
        habitat: Habitat::parse(&mc.habitat),
        species,
        names,
        genera,
        density: mc.density,
    })
}

/// Load a manifest's files into `entries` via `read_file`, returning the built `Pack`.
fn load_pack(
    m: &MRegion,
    read_file: &dyn Fn(&str) -> Option<Vec<u8>>,
    entries: &mut Vec<(Schematic, TreeSize, u8)>,
    exclude_palms: bool,
) -> Pack {
    let communities: Vec<Community> = m
        .communities
        .iter()
        .filter_map(|mc| build_community(mc, read_file, entries, exclude_palms, &[]))
        .collect();
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
        own: communities.len(),
        communities,
        default_idx,
        by_habitat,
    }
}

/// Index lists of the species whose genus passes `keep`.
fn species_where<'a>(
    communities: impl IntoIterator<Item = &'a Community>,
    keep: impl Fn(&str) -> bool,
) -> Vec<Vec<usize>> {
    communities
        .into_iter()
        .flat_map(|c| c.species.iter().zip(&c.genera))
        .filter(|(_, genus)| keep(genus))
        .map(|(sp, _)| sp.clone())
        .collect()
}

/// One entry of an ecoregion's tree mix: `[^~]pack:Community[!Species...]*weight`.
struct MixSpec<'a> {
    niche: Niche,
    pack: &'a str,
    community: &'a str,
    excluded: Vec<&'a str>,
    weight: u32,
    /// Entry without niche and weight; keys the resolved community.
    key: &'a str,
}

fn parse_mix(mix: &str) -> impl Iterator<Item = MixSpec<'_>> {
    mix.split(';').filter_map(|raw| {
        let (niche, rest) = match raw.chars().next()? {
            '^' => (Niche::Montane, &raw[1..]),
            '~' => (Niche::Wet, &raw[1..]),
            _ => (Niche::Any, raw),
        };
        let (key, weight) = rest.rsplit_once('*')?;
        let (pack, body) = key.split_once(':')?;
        let mut parts = body.split('!');
        let community = parts.next()?;
        Some(MixSpec {
            niche,
            pack,
            community,
            excluded: parts.collect(),
            weight: weight.parse().ok().filter(|&w| w > 0)?,
            key,
        })
    })
}

/// Weighted pick of `items` by a roll in `[0, 1]`.
fn weighted<T: Copy>(items: &[(T, u32)], roll: f64) -> Option<T> {
    let total: u32 = items.iter().map(|&(_, w)| w).sum();
    if total == 0 {
        return None;
    }
    let mut r = ((roll.clamp(0.0, 0.999_999) * f64::from(total)) as u32).min(total - 1);
    for &(item, w) in items {
        if r < w {
            return Some(item);
        }
        r -= w;
    }
    None
}

/// Vanilla-plus trees sprinkled into each biome, by community name.
fn vanilla_sprinkle(biome: EcoBiome) -> &'static [(&'static str, u32)] {
    use EcoBiome::*;
    match biome {
        MoistTropical => &[("VN+ Jungle Tall", 3), ("VN+ Jungle Sparse", 2)],
        DryTropical => &[("VN+ Jungle Sparse", 2), ("VN+ Acacia", 2)],
        TropicalConifer => &[("VN+ Pine", 2), ("VN+ Oaks", 1), ("VN+ Jungle Sparse", 1)],
        TemperateBroadleaf => &[
            ("VN+ Oaks", 4),
            ("VN+ Birches", 2),
            ("VN+ Dark Oaks", 1),
            ("VN+ Old Growth Birches", 1),
            ("VN+ Swamp Oaks", 1),
        ],
        TemperateConifer => &[
            ("VN+ Spruce", 2),
            ("VN+ Pine", 2),
            ("VN+ Old Growth Spruces", 1),
            ("VN+ Old Growth Pines", 1),
            ("VN+ Oaks", 1),
        ],
        Boreal => &[
            ("VN+ Spruce", 3),
            ("VN+ Pine", 2),
            ("VN+ Birches", 2),
            ("VN+ Old Growth Spruces", 1),
        ],
        TropicalGrassland => &[("VN+ Acacia", 4), ("VN+ Jungle Sparse", 1)],
        TemperateGrassland => &[("VN+ Oaks", 3), ("VN+ Birches", 1), ("VN+ Swamp Oaks", 1)],
        Flooded => &[("VN+ Swamp Oaks", 2), ("VN+ Oaks", 1)],
        MontaneGrassland => &[
            ("VN+ Spruce", 1),
            ("VN+ Pine", 1),
            ("VN+ Birches", 1),
            ("VN+ Oaks", 1),
        ],
        Tundra => &[("VN+ Spruce", 2), ("VN+ Birches", 2)],
        Mediterranean => &[("VN+ Oaks", 3), ("VN+ Pine", 2), ("VN+ Acacia", 1)],
        Desert => &[("VN+ Acacia", 3), ("VN+ Oaks", 1)],
        Mangroves => &[("VN+ Swamp Oaks", 1), ("VN+ Jungle Sparse", 1)],
    }
}

impl RegionLibrary {
    /// Load the realm pack from `source` (its `region.json`) plus the vanilla-plus sprinkle.
    pub fn load(
        source: &TreePackSource,
        scale: f64,
        ground_level: i32,
        blocks_per_meter: f64,
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
            own: 0,
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

        let mut lib = RegionLibrary {
            realm: m.realm,
            code: source.code().to_string(),
            entries,
            palm: Vec::new(),
            realm_pack,
            vanilla_pack,
            scale,
            ground_level,
            blocks_per_meter,
            sizes,
            total_realm,
            total_vanilla,
            palms_stripped: exclude_palms,
            palms_default: true,
            mixes: HashMap::new(),
        };
        lib.mark_palms();
        Ok(lib)
    }

    fn mark_palms(&mut self) {
        let mut palm = vec![false; self.entries.len()];
        for c in self
            .realm_pack
            .communities
            .iter()
            .chain(&self.vanilla_pack.communities)
        {
            for (sp, name) in c.species.iter().zip(&c.names) {
                if is_palm(name) {
                    sp.iter().for_each(|&i| palm[i] = true);
                }
            }
        }
        self.palm = palm;
    }

    /// Resolves the area's ecoregion mixes, loading communities from other packs as needed.
    pub fn attach_ecoregions(&mut self, ids: &[u16], abs_lat: f64) {
        self.palms_default = abs_lat <= 35.0;
        let mut manifests: HashMap<String, Option<MRegion>> = HashMap::new();
        let mut resolved: HashMap<String, Option<usize>> = HashMap::new();
        let mut found: Vec<(u16, Ecoregion, Vec<MixItem>)> = Vec::new();
        for &id in ids {
            let (Some(eco), Some((_, mix))) =
                (crate::ecoregion::lookup(id), crate::ecoregion::tree_mix(id))
            else {
                continue;
            };
            let mut entries = Vec::new();
            for spec in parse_mix(mix) {
                let community = match resolved.get(spec.key) {
                    Some(&c) => c,
                    None => {
                        let c = self.resolve_community(&spec, &mut manifests);
                        resolved.insert(spec.key.to_string(), c);
                        c
                    }
                };
                if let Some(c) = community {
                    entries.push((c, spec.weight, spec.niche));
                }
            }
            if !entries.is_empty() {
                found.push((id, eco, entries));
            }
        }
        self.mark_palms();
        let mut typed: HashMap<(usize, bool), usize> = HashMap::new();
        for (id, eco, entries) in found {
            let palms = crate::ecoregion::palms_belong(eco, abs_lat);
            let mut pools = build_pools(&entries, &self.realm_pack.communities, &self.palm, palms);
            // Tagged forests get the species of their leaf type where a community mixes both.
            for place in &mut pools {
                for (tag, conifer) in [(CONIFER, true), (BROADLEAF, false)] {
                    for item in &mut place[tag] {
                        item.0 = *typed
                            .entry((item.0, conifer))
                            .or_insert_with(|| self.leaf_type_variant(item.0, conifer));
                    }
                }
            }
            let sprinkle: Vec<MixItem> = vanilla_sprinkle(eco.biome)
                .iter()
                .filter_map(|&(name, w)| {
                    let pos = self
                        .vanilla_pack
                        .communities
                        .iter()
                        .position(|c| c.name == name);
                    pos.map(|i| (i, w, Niche::Any))
                })
                .collect();
            let vanilla = build_pools(&sprinkle, &self.vanilla_pack.communities, &self.palm, palms);
            let mut communities: Vec<usize> = entries.iter().map(|e| e.0).collect();
            communities.sort_unstable();
            communities.dedup();
            let beach = palms.then(|| self.palm_grove(&communities)).flatten();
            self.mixes.insert(
                id,
                EcoMix {
                    pools,
                    vanilla,
                    communities,
                    beach,
                    palms,
                },
            );
        }
    }

    /// The palm species of `communities` as one appended community, if there are any.
    fn palm_grove(&mut self, communities: &[usize]) -> Option<usize> {
        let mut grove = Community {
            name: String::new(),
            habitat: Habitat::Wet,
            species: Vec::new(),
            names: Vec::new(),
            genera: Vec::new(),
            density: default_density(),
        };
        for &c in communities {
            let c = &self.realm_pack.communities[c];
            for ((sp, name), genus) in c.species.iter().zip(&c.names).zip(&c.genera) {
                if is_palm(name) && !grove.names.contains(name) {
                    grove.species.push(sp.clone());
                    grove.names.push(name.clone());
                    grove.genera.push(genus.clone());
                }
            }
        }
        if grove.species.is_empty() {
            return None;
        }
        self.realm_pack.communities.push(grove);
        Some(self.realm_pack.communities.len() - 1)
    }

    /// The mix's palms for a beach-side slot, most of the time.
    fn beach_palms(&self, mix: &EcoMix, x: i32, z: i32) -> Option<&Community> {
        let idx = mix.beach?;
        (coord_hash(x ^ 0x0BEA, z ^ 0x0C11) % 100 < BEACH_PALMS)
            .then(|| &self.realm_pack.communities[idx])
    }

    /// `community` with only its conifers (or broadleaves), appended when it mixes both.
    fn leaf_type_variant(&mut self, community: usize, conifer: bool) -> usize {
        let base = &self.realm_pack.communities[community];
        let keep: Vec<usize> = (0..base.genera.len())
            .filter(|&s| crate::trees::mapped::is_conifer_genus(&base.genera[s]) == conifer)
            .collect();
        if keep.is_empty() || keep.len() == base.genera.len() {
            return community;
        }
        let variant = Community {
            species: keep.iter().map(|&s| base.species[s].clone()).collect(),
            names: keep.iter().map(|&s| base.names[s].clone()).collect(),
            genera: keep.iter().map(|&s| base.genera[s].clone()).collect(),
            ..base.clone()
        };
        self.realm_pack.communities.push(variant);
        self.realm_pack.communities.len() - 1
    }

    fn resolve_community(
        &mut self,
        spec: &MixSpec,
        manifests: &mut HashMap<String, Option<MRegion>>,
    ) -> Option<usize> {
        let own = &self.realm_pack.communities[..self.realm_pack.own];
        if spec.pack == self.code {
            let i = own.iter().position(|c| c.name == spec.community)?;
            if spec.excluded.is_empty() {
                return Some(i);
            }
            let base = &own[i];
            let keep: Vec<usize> = (0..base.species.len())
                .filter(|&s| {
                    !spec
                        .excluded
                        .iter()
                        .any(|&x| x == base.names[s] || x == base.genera[s])
                })
                .collect();
            if keep.is_empty() {
                return None;
            }
            let derived = Community {
                species: keep.iter().map(|&s| base.species[s].clone()).collect(),
                names: keep.iter().map(|&s| base.names[s].clone()).collect(),
                genera: keep.iter().map(|&s| base.genera[s].clone()).collect(),
                ..base.clone()
            };
            self.realm_pack.communities.push(derived);
            return Some(self.realm_pack.communities.len() - 1);
        }
        let source = TreePackSource::embedded(spec.pack);
        let manifest = manifests.entry(spec.pack.to_string()).or_insert_with(|| {
            source
                .realm_manifest()
                .and_then(|b| serde_json::from_slice::<MRegion>(&b).ok())
        });
        let mc = manifest
            .as_ref()?
            .communities
            .iter()
            .find(|c| c.name == spec.community)?;
        let read = |rel: &str| source.realm_file(rel).map(|c| c.into_owned());
        let community = build_community(
            mc,
            &read,
            &mut self.entries,
            self.palms_stripped,
            &spec.excluded,
        )?;
        self.realm_pack.communities.push(community);
        Some(self.realm_pack.communities.len() - 1)
    }

    fn is_montane(&self, elev_y: i32) -> bool {
        // Inverting the metre->Y affine needs the vertical blocks per metre, which compression
        // pulls well below the horizontal scale; fall back to the scale only if it is missing.
        let per_metre = if self.blocks_per_meter > 0.0 {
            self.blocks_per_meter
        } else {
            self.scale
        };
        f64::from(elev_y - self.ground_level) / per_metre.max(0.001) > MONTANE_METRES
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

    /// Whether palms may grow in a cell with this mix.
    fn palms_at(&self, mix: Option<&EcoMix>) -> bool {
        mix.map_or(self.palms_default, |m| m.palms)
    }

    /// Pick one variant from a community, honoring the size filter (falls back to any size if none fit).
    /// `want` overrides the scale-band size roll, so a measured canopy height can ask for its own tier.
    fn pick_in_community(
        &self,
        c: &Community,
        x: i32,
        z: i32,
        want: Option<TreeSize>,
        no_palms: bool,
    ) -> Option<usize> {
        let usable = |i: usize| !(no_palms && self.palm[i]);
        // Tier a hint resolves to here, stepping down when the cap or the 1:1
        // giant gate rules the wanted one out. Species without it drop out of
        // the pick below, else the hint loses to the species roll.
        let tier: Option<TreeSize> = want.and_then(|w| {
            let sizes = || {
                c.species
                    .iter()
                    .flatten()
                    .copied()
                    .filter(|&i| usable(i) && self.size_allowed(self.entries[i].1))
                    .map(|i| self.entries[i].1)
            };
            sizes().filter(|&s| s <= w).max().or_else(|| sizes().min())
        });
        let in_tier = |i: usize| match tier {
            Some(t) => self.entries[i].1 == t,
            None => true,
        };
        let allowed_count = |sp: &Vec<usize>| {
            sp.iter()
                .filter(|&&i| usable(i) && self.size_allowed(self.entries[i].1) && in_tier(i))
                .count()
        };
        let total: usize = c.species.iter().map(&allowed_count).sum();
        if total == 0 {
            let species: Vec<&Vec<usize>> = c
                .species
                .iter()
                .filter(|sp| sp.iter().any(|&i| usable(i)))
                .collect();
            let any: usize = species.iter().map(|sp| sp.len()).sum();
            if any == 0 {
                return None;
            }
            let mut r = (coord_hash(x + 31, z + 57) % any as u64) as usize;
            for sp in species {
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
        let want = tier.unwrap_or_else(|| self.size_pick(x, z));
        // The tier filters before the width walk, since a measured height beats
        // a look. Without a hint `in_tier` passes everything.
        let allowed: Vec<usize> = chosen
            .iter()
            .copied()
            .filter(|&i| usable(i) && self.size_allowed(self.entries[i].1) && in_tier(i))
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

    /// Weighted-pick roll: groves of one community, a fifth of the slots mixed at random.
    fn grove_roll(x: i32, z: i32, salt: u32) -> f64 {
        let h = coord_hash(x ^ salt as i32, z ^ 0x5A17);
        if h.is_multiple_of(5) {
            ((h >> 8) % 10_000) as f64 / 10_000.0
        } else {
            crate::ground_generation::patch_noise(x, z, GROVE_BLOCKS, salt)
        }
    }

    /// Community from an ecoregion's mix; `None` for a tagged conifer the mix lacks.
    fn pick_from_mix(&self, mix: &EcoMix, want: Want, x: i32, z: i32) -> Option<&Community> {
        let (place, tag) = want.pool();
        let idx = weighted(&mix.pools[place][tag], Self::grove_roll(x, z, SALT_MIX))?;
        Some(&self.realm_pack.communities[idx])
    }

    /// Vanilla sprinkle for the mix's biome; the pack's own conifers where it has none.
    fn pick_vanilla(&self, mix: &EcoMix, want: Want, x: i32, z: i32) -> &Community {
        let (place, tag) = want.pool();
        match weighted(
            &mix.vanilla[place][tag],
            Self::grove_roll(x, z, SALT_VANILLA),
        ) {
            Some(i) => &self.vanilla_pack.communities[i],
            None => self.pick_community(&self.vanilla_pack, Habitat::Conifer, x, z, want.montane),
        }
    }

    /// Side of the lattice cell that holds at most one trunk.
    pub fn base_spacing(&self) -> i32 {
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
        req: SlotRequest,
    ) -> Option<(i32, i32, usize, u8)> {
        let s = self.base_spacing();
        let (sx, sz) = crate::trees::schematic::trunk_slot_s(x, z, s);
        let montane =
            self.is_montane(elev_y) && crate::ground_generation::value_noise_01(sx, sz, 64) < 0.6;
        let blend = coord_hash(sx + 7, sz + 13) % 100;
        let mix = req.eco.and_then(|e| self.mixes.get(&e.id));
        let no_palms = !self.palms_at(mix);
        let from_mix = mix.and_then(|mix| {
            let want = Want {
                tagged: req.tagged.then_some(hint),
                wet: req.wet_ground || (req.tagged && hint == Habitat::Wet),
                montane,
            };
            let vanilla = !self.vanilla_pack.is_empty();
            // Palms line a dry shore; conifer tags and wet ground keep their own trees.
            let shore = req.beach && !want.wet && want.tagged != Some(Habitat::Conifer);
            if let Some(c) = shore.then(|| self.beach_palms(mix, sx, sz)).flatten() {
                Some(c)
            } else if vanilla && (67..97).contains(&blend) {
                Some(self.pick_vanilla(mix, want, sx, sz))
            } else {
                self.pick_from_mix(mix, want, sx, sz)
                    .or_else(|| vanilla.then(|| self.pick_vanilla(mix, want, sx, sz)))
            }
        });
        let (community, idx): (&Community, Option<usize>) = if let Some(c) = from_mix {
            (
                c,
                self.pick_in_community(c, sx, sz, req.want_size, no_palms),
            )
        } else if blend >= 97 {
            let n = self.realm_pack.own;
            if n == 0 {
                return None;
            }
            let ci = (coord_hash(sx + 5, sz + 9) % n as u64) as usize;
            let c = &self.realm_pack.communities[ci];
            (
                c,
                self.pick_in_community(c, sx, sz, req.want_size, no_palms),
            )
        } else if blend >= 67 && !self.vanilla_pack.is_empty() {
            let c = self.pick_community(&self.vanilla_pack, hint, sx, sz, montane);
            (
                c,
                self.pick_in_community(c, sx, sz, req.want_size, no_palms),
            )
        } else {
            let c = self.pick_community(&self.realm_pack, hint, sx, sz, montane);
            (
                c,
                self.pick_in_community(c, sx, sz, req.want_size, no_palms),
            )
        };
        // The grove noise invents clearings, which would thin the same trees
        // twice when the caller already measured the density.
        if !req.density_decided {
            let grove = crate::ground_generation::value_noise_01(sx, sz, Self::GROVE_PERIOD);
            let jitter = (coord_hash(sx ^ 0x71c3, sz ^ 0x2d9b) % 1000) as f64 / 1000.0;
            if grove * 0.82 + jitter * 0.18 >= Self::keep_prob(community.density) {
                return None;
            }
        }
        let idx = idx?;
        let rot = (coord_hash(sx ^ 0x5bd1, sz ^ 0x9e37) % 4) as u8;
        Some((sx, sz, idx, rot))
    }

    /// A mapped tree keeps its position and is never thinned. Its genus comes first,
    /// then the community's species of its leaf type.
    pub fn pick_mapped(
        &self,
        x: i32,
        z: i32,
        hint: Habitat,
        elev_y: i32,
        req: MappedRequest,
    ) -> Option<(i32, i32, usize, u8)> {
        let rot = (coord_hash(x ^ 0x5bd1, z ^ 0x9e37) % 4) as u8;
        let mix = req.eco.and_then(|e| self.mixes.get(&e.id));
        let no_palms = !self.palms_at(mix);
        let pick = |species: Vec<Vec<usize>>, no_palms: bool| {
            if species.is_empty() {
                return None;
            }
            let pool = Community {
                name: String::new(),
                habitat: hint,
                species,
                names: Vec::new(),
                genera: Vec::new(),
                density: 0,
            };
            self.pick_in_community(&pool, x, z, req.want_size, no_palms)
        };
        let mix_communities = || {
            mix.into_iter()
                .flat_map(|m| &m.communities)
                .map(|&c| &self.realm_pack.communities[c])
        };
        // A mapped genus is what stands there, planted palm or not.
        if let Some(genus) = req.genus {
            let same = |g: &str| g.eq_ignore_ascii_case(genus);
            if let Some(idx) = pick(species_where(mix_communities(), same), false) {
                return Some((x, z, idx, rot));
            }
            for pack in [&self.realm_pack, &self.vanilla_pack] {
                if let Some(idx) = pick(species_where(&pack.communities, same), false) {
                    return Some((x, z, idx, rot));
                }
            }
        }

        let montane =
            self.is_montane(elev_y) && crate::ground_generation::value_noise_01(x, z, 64) < 0.6;
        let want = Want {
            tagged: req.conifer.map(|c| {
                if c {
                    Habitat::Conifer
                } else {
                    Habitat::Lowland
                }
            }),
            wet: hint == Habitat::Wet,
            montane,
        };
        let beach = mix
            .filter(|_| req.beach && !want.wet && req.conifer != Some(true))
            .and_then(|m| self.beach_palms(m, x, z));
        let community = beach
            .or_else(|| mix.and_then(|m| self.pick_from_mix(m, want, x, z)))
            .unwrap_or_else(|| self.pick_community(&self.realm_pack, hint, x, z, montane));
        let idx = match req.conifer {
            Some(conifer) => {
                let of_type = |g: &str| crate::trees::mapped::is_conifer_genus(g) == conifer;
                pick(species_where([community], of_type), no_palms)
                    .or_else(|| pick(species_where(mix_communities(), of_type), no_palms))
                    .or_else(|| {
                        pick(
                            species_where(&self.realm_pack.communities, of_type),
                            no_palms,
                        )
                    })
                    .or_else(|| self.pick_in_community(community, x, z, req.want_size, no_palms))
            }
            None => self.pick_in_community(community, x, z, req.want_size, no_palms),
        }
        // A mapped tree always stands, even if only a palm is left to stand for it.
        .or_else(|| self.pick_in_community(community, x, z, req.want_size, false))?;
        Some((x, z, idx, rot))
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
            self.realm_pack.own,
            self.total_vanilla,
            self.vanilla_pack.communities.len(),
        );
        let borrowed = self.realm_pack.communities.len() - self.realm_pack.own;
        if !self.mixes.is_empty() {
            println!(
                "  ecoregions: {} mapped, {} extra communities ({} trees)",
                self.mixes.len(),
                borrowed,
                self.entries.len() - self.total_realm - self.total_vanilla,
            );
        }
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
        assert!(is_palm("Areca_catechu"));
        assert!(!is_palm("Quercus_alba"));
        // A Japanese maple is no palm.
        assert!(!is_palm("Acer_palmatum"));
    }

    #[test]
    fn montane_starts_at_the_same_real_height_whatever_the_compression() {
        let src = TreePackSource::embedded("eur");
        let base = -62;
        let lib = |bpm: f64| {
            RegionLibrary::load(&src, 1.0, base, bpm, SizeFilter::default(), false)
                .expect("load eur")
        };

        let uncompressed = lib(1.0);
        assert!(!uncompressed.is_montane(base + 450));
        assert!(uncompressed.is_montane(base + 451));

        // Alps at scale 1 with the vanilla ceiling: 4441 m squeezed into 366 blocks,
        // so 450 m above the base lands 37.1 blocks up.
        let compressed = lib(366.0 / 4441.0);
        assert!(!compressed.is_montane(base + 37));
        assert!(compressed.is_montane(base + 38));
    }

    #[test]
    fn ena_palm_gate_removes_trees() {
        let src = TreePackSource::embedded("ena");
        let incl = RegionLibrary::load(&src, 1.0, -62, 1.0, SizeFilter::default(), false).unwrap();
        let excl = RegionLibrary::load(&src, 1.0, -62, 1.0, SizeFilter::default(), true).unwrap();
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
        let lib = RegionLibrary::load(&src, 1.0, -62, 1.0, SizeFilter::default(), false)
            .expect("load eur");
        assert!(lib.total_realm > 0);
        for k in 0..200 {
            if let Some((_, _, idx, _)) =
                lib.pick_slot(k * 3, k * 7, Habitat::Lowland, 0, SlotRequest::default())
            {
                assert!(idx < lib.entries.len());
            }
        }
    }

    /// Entry indices of every pack species whose genus passes `keep`.
    fn entries_where(lib: &RegionLibrary, keep: impl Fn(&str) -> bool) -> Vec<usize> {
        [&lib.realm_pack, &lib.vanilla_pack]
            .iter()
            .flat_map(|p| &p.communities)
            .flat_map(|c| c.species.iter().zip(&c.genera))
            .filter(|(_, g)| keep(g))
            .flat_map(|(sp, _)| sp.iter().copied())
            .collect()
    }

    // A mapped tree is a tree for sure: it is never thinned out and stays where it was mapped.
    #[test]
    fn mapped_trees_are_never_dropped_or_moved() {
        let src = TreePackSource::embedded("eur");
        let lib = RegionLibrary::load(&src, 1.0, -62, 1.0, SizeFilter::default(), false).unwrap();
        for k in 0..2000 {
            let (x, z) = (k * 37 % 3001 - 1500, k * 91 % 2999 - 1500);
            let picked = lib.pick_mapped(x, z, Habitat::Lowland, 0, MappedRequest::default());
            let (sx, sz, idx, _) = picked.expect("a mapped tree always gets a model");
            assert_eq!((sx, sz), (x, z));
            assert!(idx < lib.entries.len());
        }
    }

    #[test]
    fn mapped_trees_stand_in_every_mix() {
        for (pack, eco, lat) in [("eur", 795, 41.9), ("ena", 330, 35.8), ("sam", 576, 34.6)] {
            let lib = with_ecoregions(pack, &[eco], lat);
            for conifer in [None, Some(true), Some(false)] {
                for k in 0..500 {
                    let (x, z) = (k * 37 % 3001 - 1500, k * 91 % 2999 - 1500);
                    let req = MappedRequest {
                        conifer,
                        eco: crate::ecoregion::lookup(eco),
                        ..Default::default()
                    };
                    let picked = lib.pick_mapped(x, z, Habitat::Lowland, 0, req);
                    assert!(picked.is_some(), "{pack} {eco} {conifer:?}");
                }
            }
        }
    }

    #[test]
    fn mapped_genus_and_leaf_type_steer_the_model() {
        let src = TreePackSource::embedded("eur");
        let lib = RegionLibrary::load(&src, 1.0, -62, 1.0, SizeFilter::default(), false).unwrap();
        let tilia = entries_where(&lib, |g| g == "Tilia");
        let conifers = entries_where(&lib, crate::trees::mapped::is_conifer_genus);
        assert!(!tilia.is_empty() && !conifers.is_empty());
        for k in 0..500 {
            let (x, z) = (k * 13, k * 29);
            let lime = MappedRequest {
                genus: Some("Tilia"),
                conifer: Some(false),
                ..Default::default()
            };
            let (_, _, idx, _) = lib.pick_mapped(x, z, Habitat::Lowland, 0, lime).unwrap();
            assert!(tilia.contains(&idx), "a mapped lime gets a lime");

            // No Robinia in the pack: any broadleaf, but never a conifer.
            let robinia = MappedRequest {
                genus: Some("Robinia"),
                conifer: Some(false),
                ..Default::default()
            };
            let (_, _, idx, _) = lib.pick_mapped(x, z, Habitat::Lowland, 0, robinia).unwrap();
            assert!(
                !conifers.contains(&idx),
                "a broadleaf never becomes a conifer"
            );
        }
    }

    // A canopy height hint should steer the pick toward that tier, and must never
    // exceed it: a 5 m crown cannot become a 30 m tree.
    #[test]
    fn canopy_size_hint_steers_the_pick() {
        let src = TreePackSource::embedded("eur");
        let lib = RegionLibrary::load(&src, 1.0, -62, 1.0, SizeFilter::default(), false)
            .expect("load eur");
        let tally = |hint: Option<TreeSize>| {
            let req = SlotRequest {
                want_size: hint,
                ..Default::default()
            };
            let mut counts = [0u32; 5];
            for k in 0..2000 {
                if let Some((_, _, idx, _)) = lib.pick_slot(k * 3, k * 7, Habitat::Lowland, 0, req)
                {
                    counts[lib.entries[idx].1 as usize] += 1;
                }
            }
            counts
        };
        let small = tally(Some(TreeSize::Small));
        let medium = tally(Some(TreeSize::Medium));
        let tall = tally(Some(TreeSize::Tall));
        assert!(small.iter().sum::<u32>() > 100, "some slots must fill");

        // A hint never overshoots except where a community has nothing that
        // small, in which case its own smallest stands in.
        assert_eq!((small[3], small[4]), (0, 0), "{small:?}");
        assert!(small[2] * 100 < small.iter().sum::<u32>(), "{small:?}");
        assert_eq!((medium[0], medium[3], medium[4]), (0, 0, 0), "{medium:?}");
        assert_eq!((tall[0], tall[4]), (0, 0), "{tall:?}");

        // And it binds: the wanted tier dominates what the pack can offer.
        assert!(
            medium[1] * 10 > medium.iter().sum::<u32>() * 9,
            "{medium:?}"
        );
        assert!(tall[3] * 2 > tall.iter().sum::<u32>(), "{tall:?}");
        let mean = |c: &[u32; 5]| -> f64 {
            let n: u32 = c.iter().sum();
            c.iter()
                .enumerate()
                .map(|(i, &v)| (i * v as usize) as f64)
                .sum::<f64>()
                / f64::from(n)
        };
        assert!(mean(&small) < mean(&medium), "{small:?} {medium:?}");
        assert!(mean(&medium) < mean(&tall), "{medium:?} {tall:?}");

        // The slot count is the canopy fraction's job, not the height's, so the
        // hint must not change how many trees stand.
        let plain: u32 = tally(None).iter().sum();
        assert_eq!(plain, small.iter().sum::<u32>());
        assert_eq!(plain, tall.iter().sum::<u32>());
    }

    // The user's cap outranks the canopy: asking for a 30 m crown under a Small
    // cap must not grow a single tree past what the cap alone would have placed.
    // Communities with nothing that small still fall back to their own smallest,
    // which is the pack's long-standing "a tree beats a hole" rule.
    #[test]
    fn max_tree_size_clamps_the_canopy_hint() {
        let src = TreePackSource::embedded("eur");
        let lib = RegionLibrary::load(
            &src,
            1.0,
            -62,
            1.0,
            SizeFilter::up_to(TreeSize::Small),
            false,
        )
        .expect("load eur");
        let tally = |want_size| {
            let req = SlotRequest {
                want_size,
                density_decided: true,
                ..Default::default()
            };
            let mut counts = [0u32; 5];
            for k in 0..2000 {
                if let Some((_, _, idx, _)) = lib.pick_slot(k * 3, k * 7, Habitat::Lowland, 0, req)
                {
                    counts[lib.entries[idx].1 as usize] += 1;
                }
            }
            counts
        };
        let capped = tally(None);
        let hinted = tally(Some(TreeSize::Giant));
        assert!(
            capped.iter().sum::<u32>() > 100,
            "the cap must not empty the pack"
        );
        assert_eq!(hinted, capped, "a Giant hint must not beat a Small cap");
        assert!(
            hinted[0] > hinted[1..].iter().copied().max().unwrap_or(0),
            "the cap should still dominate ({hinted:?})"
        );
    }

    /// A library for `pack` with the given ecoregions attached.
    fn with_ecoregions(pack: &str, ids: &[u16], abs_lat: f64) -> RegionLibrary {
        let src = TreePackSource::embedded(pack);
        let mut lib = RegionLibrary::load(&src, 1.0, -62, 1.0, SizeFilter::default(), false)
            .expect("load pack");
        lib.attach_ecoregions(ids, abs_lat);
        lib
    }

    /// Names of the species a run of slots in `eco` plants, with how often.
    fn planted(
        lib: &RegionLibrary,
        eco: u16,
        req: SlotRequest,
        hint: Habitat,
        elev: i32,
    ) -> HashMap<String, u32> {
        let names: HashMap<usize, String> = lib
            .realm_pack
            .communities
            .iter()
            .chain(&lib.vanilla_pack.communities)
            .flat_map(|c| c.species.iter().zip(&c.names))
            .flat_map(|(sp, n)| sp.iter().map(move |&i| (i, n.clone())))
            .collect();
        let req = SlotRequest {
            eco: crate::ecoregion::lookup(eco),
            density_decided: true,
            ..req
        };
        let mut out: HashMap<String, u32> = HashMap::new();
        for k in 0..3000 {
            if let Some((_, _, idx, _)) =
                lib.pick_slot(k * 11 % 997 * 5, k * 7 % 991 * 5, hint, elev, req)
            {
                *out.entry(names[&idx].clone()).or_default() += 1;
            }
        }
        out
    }

    #[test]
    fn every_ecoregion_mix_resolves() {
        let mut by_pack: HashMap<&str, Vec<u16>> = HashMap::new();
        for id in 0..900u16 {
            if let Some((pack, _)) = crate::ecoregion::tree_mix(id) {
                by_pack.entry(pack).or_default().push(id);
            }
        }
        assert!(by_pack.values().map(Vec::len).sum::<usize>() > 800);
        for (pack, ids) in by_pack {
            let mut lib = with_ecoregions(pack, &ids, 30.0);
            let mut manifests = HashMap::new();
            for &id in &ids {
                let mix = lib
                    .mixes
                    .get(&id)
                    .unwrap_or_else(|| panic!("ecoregion {id} in {pack}"));
                assert!(!mix.vanilla[PLAIN][UNTAGGED].is_empty(), "vanilla for {id}");
                let (_, mix) = crate::ecoregion::tree_mix(id).unwrap();
                for spec in parse_mix(mix) {
                    let resolved = lib.resolve_community(&spec, &mut manifests);
                    assert!(resolved.is_some(), "ecoregion {id}: {}", spec.key);
                }
            }
        }
    }

    #[test]
    fn humid_pampas_grows_pampas_trees() {
        let lib = with_ecoregions("sam", &[576], 34.6);
        let got = planted(&lib, 576, SlotRequest::default(), Habitat::Conifer, 0);
        assert!(got.values().sum::<u32>() > 1000);
        for rainforest in [
            "Hevea_brasiliensis",
            "Nothofagus_betuloides",
            "Araucaria_araucana",
        ] {
            assert!(
                !got.contains_key(rainforest),
                "{rainforest} in the Pampas: {got:?}"
            );
        }
        assert!(got.contains_key("Phytolacca_dioica"), "{got:?}");
        // The eucalyptus windbreaks come from the Australian pack.
        assert!(got.keys().any(|n| n.starts_with("Eucalyptus")), "{got:?}");
    }

    #[test]
    fn montane_and_wet_ground_take_their_niche() {
        let lib = with_ecoregions("eur", &[689], 46.0);
        let vanilla: Vec<&String> = lib
            .vanilla_pack
            .communities
            .iter()
            .flat_map(|c| &c.names)
            .collect();
        let regional = |got: &HashMap<String, u32>| -> u32 {
            got.iter()
                .filter(|(n, _)| !vanilla.contains(n))
                .map(|(_, &c)| c)
                .sum()
        };
        let wet_req = SlotRequest {
            wet_ground: true,
            ..Default::default()
        };
        let wet = planted(&lib, 689, wet_req, Habitat::Lowland, 0);
        let riparian: u32 = ["Alnus_glutinosa", "Populus_nigra", "Salix_alba"]
            .iter()
            .filter_map(|n| wet.get(*n))
            .sum();
        assert!(
            riparian > 0 && riparian == regional(&wet),
            "wet ground: {wet:?}"
        );

        // Up the mountain the beech thins out and the alpine conifers take over.
        let low = planted(&lib, 689, SlotRequest::default(), Habitat::Lowland, 0);
        let high = planted(
            &lib,
            689,
            SlotRequest::default(),
            Habitat::Lowland,
            -62 + 2000,
        );
        let beech = |got: &HashMap<String, u32>| got.get("Fagus_sylvatica").copied().unwrap_or(0);
        assert!(beech(&high) * 2 < beech(&low), "low {low:?} high {high:?}");
        assert!(high.contains_key("Pinus_cembra"), "{high:?}");
    }

    #[test]
    fn palms_follow_the_ecoregion() {
        // Rome plants Canary palms; the Piedmont above 35 degrees keeps its palmetto out.
        let rome = with_ecoregions("eur", &[795], 41.9);
        let got = planted(&rome, 795, SlotRequest::default(), Habitat::Lowland, 0);
        assert!(got.contains_key("Phoenix_canariensis"), "{got:?}");
        assert!(got.contains_key("Pinus_pinea"), "{got:?}");

        let piedmont = with_ecoregions("ena", &[330], 35.8);
        let got = planted(&piedmont, 330, SlotRequest::default(), Habitat::Lowland, 0);
        assert!(got.contains_key("Liquidambar_styraciflua"), "{got:?}");
        assert!(!got.keys().any(|n| is_palm(n)), "{got:?}");
    }

    #[test]
    fn palms_line_the_beach_where_they_belong() {
        let beach = SlotRequest {
            beach: true,
            ..Default::default()
        };
        // Miami: the Everglades mix grows coconut, royal and sabal palms on the shore.
        let miami = with_ecoregions("fl", &[581], 25.8);
        let got = planted(&miami, 581, beach, Habitat::Lowland, 0);
        let palms: u32 = got
            .iter()
            .filter(|(n, _)| is_palm(n))
            .map(|(_, &c)| c)
            .sum();
        assert!(palms * 2 > got.values().sum::<u32>(), "{got:?}");
        let inland = planted(&miami, 581, SlotRequest::default(), Habitat::Lowland, 0);
        let inland_palms: u32 = inland
            .iter()
            .filter(|(n, _)| is_palm(n))
            .map(|(_, &c)| c)
            .sum();
        assert!(inland_palms * 3 < palms, "{inland:?}");

        // A Baltic beach has no palms to line it.
        let baltic = with_ecoregions("eur", &[647], 54.5);
        let got = planted(&baltic, 647, beach, Habitat::Lowland, 0);
        assert!(
            !got.is_empty() && !got.keys().any(|n| is_palm(n)),
            "{got:?}"
        );
    }

    #[test]
    fn a_needleleaved_forest_in_rome_is_stone_pine_and_cypress() {
        let lib = with_ecoregions("eur", &[795], 41.9);
        let req = SlotRequest {
            tagged: true,
            ..Default::default()
        };
        let got = planted(&lib, 795, req, Habitat::Conifer, 0);
        assert!(got.contains_key("Pinus_pinea"), "{got:?}");
        assert!(got.contains_key("Cupressus_sempervirens"), "{got:?}");
        assert!(!got.contains_key("Picea_generica"), "{got:?}");
    }

    #[test]
    fn tags_still_decide_the_leaf_type() {
        let lib = with_ecoregions("eur", &[654], 52.5);
        let req = SlotRequest {
            tagged: true,
            ..Default::default()
        };
        let got = planted(&lib, 654, req, Habitat::Conifer, 0);
        assert!(!got.is_empty());
        for name in got.keys() {
            let genus = name.split('_').next().unwrap();
            assert!(
                crate::trees::mapped::is_conifer_genus(genus),
                "{name} in {got:?}"
            );
        }
    }
}
