//! Trees OSM maps one by one: `natural=tree` nodes and the trees of a `natural=tree_row`.

use std::collections::HashMap;

use rand::prelude::IndexedRandom;

use crate::deterministic_rng::element_rng;
use crate::element_processing::tree::TreeType;
use crate::osm_parser::{ProcessedElement, ProcessedNode};

const ROW_SPACING_M: f64 = 8.0;
const CROWN_RADIUS_M: f64 = 5.0;

pub struct MappedTree {
    pub kind: TreeType,
    pub genus: Option<String>,
    pub conifer: Option<bool>,
    pub height_m: Option<f64>,
}

impl MappedTree {
    pub fn from_tags(tags: &HashMap<String, String>, id: u64) -> Self {
        use TreeType::*;
        let genus = genus_from_tags(tags);
        let leaf_type = tags.get("leaf_type").map(String::as_str);
        let known = genus.as_deref().and_then(genus_pool);
        let pool: &[TreeType] = match (known, leaf_type) {
            (Some(pool), _) => pool,
            (None, Some("broadleaved")) => &[Oak, Birch, TallOak],
            (None, Some("needleleaved")) => &[Spruce, Pine],
            // Any named genus not known as a conifer is a broadleaf.
            (None, _) if genus.is_some() => &[Oak, TallOak],
            (None, Some(_)) => &[Oak, Spruce, Birch, TallOak, Pine],
            (None, None) => &[Oak, Spruce, Birch, TallOak],
        };
        let conifer = match (genus.as_deref(), leaf_type) {
            (_, Some("needleleaved")) => Some(true),
            (_, Some("broadleaved")) => Some(false),
            (Some(g), _) => Some(is_conifer_genus(g)),
            (None, _) => None,
        };
        let kind = *pool.choose(&mut element_rng(id)).unwrap_or(&Oak);
        let height_m = tags
            .get("height")
            .and_then(|h| crate::mapillary::geometry::parse_length_m(h))
            .filter(|h| (1.0..=120.0).contains(h));
        MappedTree {
            kind,
            genus,
            conifer,
            height_m,
        }
    }
}

fn genus_from_tags(tags: &HashMap<String, String>) -> Option<String> {
    for key in ["genus", "species", "taxon"] {
        let word = tags.get(key).and_then(|v| {
            v.split(|c: char| !c.is_alphabetic())
                .find(|w| !w.is_empty())
        });
        if let Some(word) = word {
            let mut chars = word.chars();
            let first = chars.next()?.to_uppercase();
            return Some(first.chain(chars.flat_map(char::to_lowercase)).collect());
        }
    }
    let genus = match tags.get("genus:wikidata").map(String::as_str) {
        Some("Q12004") => "Betula",
        Some("Q26782") => "Quercus",
        Some("Q25243") => "Picea",
        _ => return None,
    };
    Some(genus.to_string())
}

pub fn is_conifer_genus(genus: &str) -> bool {
    matches!(
        genus,
        "Abies"
            | "Agathis"
            | "Araucaria"
            | "Callitris"
            | "Calocedrus"
            | "Cedrus"
            | "Cephalotaxus"
            | "Chamaecyparis"
            | "Cryptomeria"
            | "Cunninghamia"
            | "Cupressocyparis"
            | "Cupressus"
            | "Glyptostrobus"
            | "Juniperus"
            | "Keteleeria"
            | "Larix"
            | "Metasequoia"
            | "Picea"
            | "Pinus"
            | "Platycladus"
            | "Podocarpus"
            | "Pseudolarix"
            | "Pseudotsuga"
            | "Sciadopitys"
            | "Sequoia"
            | "Sequoiadendron"
            | "Taxodium"
            | "Taxus"
            | "Tetraclinis"
            | "Thuja"
            | "Thujopsis"
            | "Torreya"
            | "Tsuga"
            | "Wollemia"
            | "Xanthocyparis"
    )
}

fn genus_pool(genus: &str) -> Option<&'static [TreeType]> {
    use TreeType::*;
    Some(match genus {
        "Betula" => &[Birch],
        "Quercus" => &[Oak],
        "Salix" => &[Willow],
        "Pinus" | "Larix" | "Cedrus" => &[Pine],
        "Prunus" | "Malus" | "Pyrus" | "Magnolia" | "Cercis" | "Crataegus" | "Sorbus"
        | "Amelanchier" | "Jacaranda" | "Lagerstroemia" => &[FloweringOak],
        "Acacia" | "Vachellia" | "Senegalia" | "Albizia" | "Prosopis" | "Parkinsonia"
        | "Delonix" => &[Acacia],
        "Phoenix" | "Washingtonia" | "Cocos" | "Trachycarpus" | "Sabal" | "Roystonea"
        | "Syagrus" | "Butia" | "Livistona" | "Chamaerops" | "Elaeis" | "Archontophoenix" => {
            &[Jungle]
        }
        "Rhizophora" | "Avicennia" | "Laguncularia" | "Bruguiera" | "Sonneratia" => &[Mangrove],
        g if is_conifer_genus(g) => &[Spruce],
        _ => return None,
    })
}

/// Mirrors the dispatch in `process_element`.
pub fn is_tree_row(tags: &HashMap<String, String>) -> bool {
    tags.get("natural").map(String::as_str) == Some("tree_row")
        && !["building", "building:part", "highway", "landuse"]
            .iter()
            .any(|k| tags.contains_key(*k))
}

/// A row is drawn from its first tree to its last, so both ends get one.
pub fn tree_row_positions(nodes: &[ProcessedNode], scale: f64) -> Vec<(i32, i32)> {
    let pts: Vec<(f64, f64)> = nodes.iter().map(|n| (n.x as f64, n.z as f64)).collect();
    let Some(&first) = pts.first() else {
        return Vec::new();
    };
    let dist = |a: (f64, f64), b: (f64, f64)| (b.0 - a.0).hypot(b.1 - a.1);
    let total: f64 = pts.windows(2).map(|w| dist(w[0], w[1])).sum();
    if total < 1.0 {
        return vec![(first.0.round() as i32, first.1.round() as i32)];
    }
    let spacing = (ROW_SPACING_M * scale).max(1.0);
    let intervals = (total / spacing).round().max(1.0) as usize;
    let step = total / intervals as f64;
    let closed =
        pts.len() > 2 && nodes.first().map(|n| (n.x, n.z)) == nodes.last().map(|n| (n.x, n.z));
    let count = if closed { intervals } else { intervals + 1 };

    let mut out: Vec<(i32, i32)> = Vec::with_capacity(count);
    let (mut seg, mut seg_start) = (0usize, 0.0f64);
    for k in 0..count {
        let target = k as f64 * step;
        while seg + 2 < pts.len() && seg_start + dist(pts[seg], pts[seg + 1]) < target {
            seg_start += dist(pts[seg], pts[seg + 1]);
            seg += 1;
        }
        let (a, b) = (pts[seg], pts[seg + 1]);
        let len = dist(a, b);
        let t = if len > 0.0 {
            ((target - seg_start) / len).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let cell = (
            (a.0 + (b.0 - a.0) * t).round() as i32,
            (a.1 + (b.1 - a.1) * t).round() as i32,
        );
        if out.last() != Some(&cell) {
            out.push(cell);
        }
    }
    out
}

/// Mapped trunks, so canopy data does not plant their crowns a second time.
pub struct MappedTrunks {
    /// (cell x, cell z, x, z), sorted by cell.
    trunks: Vec<(i32, i32, i32, i32)>,
    radius: i32,
}

impl MappedTrunks {
    pub fn collect(elements: &[ProcessedElement], scale: f64) -> Self {
        let radius = ((CROWN_RADIUS_M * scale).round() as i32).max(1);
        let mut trunks = Vec::new();
        let mut add = |x: i32, z: i32| {
            trunks.push((x.div_euclid(radius), z.div_euclid(radius), x, z));
        };
        for element in elements {
            match element {
                ProcessedElement::Node(node)
                    if node.tags.get("natural").map(String::as_str) == Some("tree") =>
                {
                    add(node.x, node.z);
                }
                ProcessedElement::Way(way) if is_tree_row(&way.tags) => {
                    for (x, z) in tree_row_positions(&way.nodes, scale) {
                        add(x, z);
                    }
                }
                _ => {}
            }
        }
        trunks.sort_unstable();
        trunks.shrink_to_fit();
        MappedTrunks { trunks, radius }
    }

    pub fn under_crown(&self, x: i32, z: i32) -> bool {
        if self.trunks.is_empty() {
            return false;
        }
        let r = self.radius;
        let (cx, cz) = (x.div_euclid(r), z.div_euclid(r));
        for gx in cx - 1..=cx + 1 {
            let start = self.trunks.partition_point(|t| (t.0, t.1) < (gx, cz - 1));
            for &(tgx, tgz, tx, tz) in &self.trunks[start..] {
                if (tgx, tgz) > (gx, cz + 1) {
                    break;
                }
                if (tx - x).pow(2) + (tz - z).pow(2) <= r * r {
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn node(x: i32, z: i32) -> ProcessedNode {
        ProcessedNode {
            id: 0,
            tags: HashMap::new(),
            x,
            z,
        }
    }

    #[test]
    fn an_unlisted_broadleaf_species_never_becomes_a_conifer() {
        for id in 0..200 {
            let t = MappedTree::from_tags(&tags(&[("species", "Robinia pseudoacacia")]), id);
            assert!(matches!(t.kind, TreeType::Oak | TreeType::TallOak));
            assert_eq!(t.genus.as_deref(), Some("Robinia"));
            assert_eq!(t.conifer, Some(false));
        }
    }

    #[test]
    fn genus_comes_from_species_genus_or_wikidata() {
        let g = |pairs: &[(&str, &str)]| MappedTree::from_tags(&tags(pairs), 1).genus;
        assert_eq!(g(&[("genus", "Tilia")]).as_deref(), Some("Tilia"));
        assert_eq!(
            g(&[("species", "tilia cordata 'Greenspire'")]).as_deref(),
            Some("Tilia")
        );
        assert_eq!(
            g(&[("genus:wikidata", "Q12004")]).as_deref(),
            Some("Betula")
        );
        assert_eq!(g(&[("leaf_type", "broadleaved")]), None);
    }

    #[test]
    fn conifers_and_leaf_type_are_recognised() {
        let t = MappedTree::from_tags(&tags(&[("species", "Taxus baccata")]), 1);
        assert!(matches!(t.kind, TreeType::Spruce));
        assert_eq!(t.conifer, Some(true));
        let t = MappedTree::from_tags(&tags(&[("species", "Pinus nigra")]), 1);
        assert!(matches!(t.kind, TreeType::Pine));
        // An explicit leaf type outranks the guess for an unknown genus.
        let t = MappedTree::from_tags(
            &tags(&[("species", "Foo bar"), ("leaf_type", "needleleaved")]),
            1,
        );
        assert!(matches!(t.kind, TreeType::Spruce | TreeType::Pine));
        assert_eq!(t.conifer, Some(true));
        assert_eq!(MappedTree::from_tags(&tags(&[]), 1).conifer, None);
    }

    #[test]
    fn height_is_read_in_metres() {
        let h = |v: &str| MappedTree::from_tags(&tags(&[("height", v)]), 1).height_m;
        assert_eq!(h("12"), Some(12.0));
        assert_eq!(h("12.5 m"), Some(12.5));
        assert_eq!(h("0"), None);
        assert_eq!(h("tall"), None);
    }

    #[test]
    fn a_tree_row_gets_a_tree_at_each_end_and_evenly_between() {
        let row = [node(0, 0), node(40, 0)];
        let trunks = tree_row_positions(&row, 1.0);
        assert_eq!(
            trunks,
            vec![(0, 0), (8, 0), (16, 0), (24, 0), (32, 0), (40, 0)]
        );

        // Around a corner the spacing follows the line, not the chord.
        let bent = [node(0, 0), node(12, 0), node(12, 12)];
        let trunks = tree_row_positions(&bent, 1.0);
        assert_eq!(trunks.first(), Some(&(0, 0)));
        assert_eq!(trunks.last(), Some(&(12, 12)));
        assert_eq!(trunks.len(), 4);

        // Half the scale, half the blocks between trees.
        assert_eq!(tree_row_positions(&row, 0.5).len(), 11);
        assert_eq!(tree_row_positions(&[node(3, 4)], 1.0), vec![(3, 4)]);
    }

    #[test]
    fn crowns_cover_the_ground_around_mapped_trunks_only() {
        let tree = ProcessedElement::Node(ProcessedNode {
            id: 1,
            tags: tags(&[("natural", "tree")]),
            x: 100,
            z: 100,
        });
        let row = ProcessedElement::Way(crate::osm_parser::ProcessedWay {
            id: 2,
            tags: tags(&[("natural", "tree_row")]),
            nodes: vec![node(-50, 0), node(-10, 0)],
        });
        let trunks = MappedTrunks::collect(&[tree, row], 1.0);
        assert!(trunks.under_crown(100, 100));
        assert!(trunks.under_crown(104, 102));
        assert!(!trunks.under_crown(107, 100));
        assert!(trunks.under_crown(-30, 3));
        assert!(!trunks.under_crown(-30, 9));
        assert!(!trunks.under_crown(0, 100));
    }
}
