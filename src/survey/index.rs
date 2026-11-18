//! Survey indexes: where each survey (or each of its files) has points, and
//! which to read for a tile where several overlap.
//!
//! Sources that span a continent publish one index of many surveys flown in
//! different years, often over the same ground. A tile is read from one
//! survey only, so two scans years apart never mix: of the surveys covering
//! most of the part of the tile an area needs, the newest, then the densest.

use super::source::{Rect, Sheet, TileUrls};

/// Where one file (or one whole survey) has points.
#[derive(Debug, Clone, PartialEq)]
pub struct Footprint {
    pub url: String,
    /// The survey the file belongs to; a whole survey is its own.
    pub survey: String,
    pub year: u16,
    /// Points a square metre, where the index says; 0 where it does not.
    pub density: f64,
    /// West, south, east, north in degrees.
    pub bbox: (f64, f64, f64, f64),
    /// Rings of (lon, lat), outer and inner alike (even-odd).
    pub rings: Vec<Vec<(f64, f64)>>,
}

impl Footprint {
    /// From GeoJSON Polygon or MultiPolygon coordinates; `None` when empty.
    pub fn new(
        url: &str,
        survey: &str,
        year: u16,
        geometry: &serde_json::Value,
        points: f64,
    ) -> Option<Footprint> {
        let polys: Vec<&serde_json::Value> = match geometry["type"].as_str() {
            Some("MultiPolygon") => geometry["coordinates"]
                .as_array()
                .into_iter()
                .flatten()
                .collect(),
            Some("Polygon") => vec![&geometry["coordinates"]],
            _ => return None,
        };
        let mut rings = Vec::new();
        for poly in polys {
            for ring in poly.as_array().into_iter().flatten() {
                let pts: Vec<(f64, f64)> = ring
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| Some((c[0].as_f64()?, c[1].as_f64()?)))
                    .collect();
                if pts.len() >= 3 {
                    rings.push(pts);
                }
            }
        }
        if rings.is_empty() {
            return None;
        }
        let (mut w, mut s, mut e, mut n) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        // Shoelace area on each ring's own equirectangular scale; holes wind
        // the other way and subtract.
        let mut area = 0.0;
        for r in &rings {
            let k = (r.iter().map(|p| p.1).sum::<f64>() / r.len() as f64)
                .to_radians()
                .cos();
            let mut a = 0.0;
            for i in 0..r.len() {
                let (x0, y0) = r[i];
                let (x1, y1) = r[(i + 1) % r.len()];
                w = w.min(x0);
                e = e.max(x0);
                s = s.min(y0);
                n = n.max(y0);
                a += x0 * y1 - x1 * y0;
            }
            area += a * k / 2.0;
        }
        let m2 = area.abs() * 111_320.0 * 110_574.0;
        Some(Footprint {
            url: url.to_string(),
            survey: survey.to_string(),
            year,
            density: if m2 > 0.0 { points / m2 } else { 0.0 },
            bbox: (w, s, e, n),
            rings,
        })
    }

    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        let (w, s, e, n) = self.bbox;
        if lon < w || lon > e || lat < s || lat > n {
            return false;
        }
        let mut inside = false;
        for r in &self.rings {
            let mut j = r.len() - 1;
            for i in 0..r.len() {
                let ((xi, yi), (xj, yj)) = (r[i], r[j]);
                if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
                    inside = !inside;
                }
                j = i;
            }
        }
        inside
    }
}

/// The year a survey was flown (or funded), from its name: the first run of
/// exactly four digits that reads as a year (`WA_KingCo_1_2021`, `GTA2015`,
/// `USGS_LPC_MI_31Co_Washtenaw_2016_LAS_2019`), else a fiscal-year tag
/// (`MN_CentralMissRiver_4_B22`); 0 when there is none.
pub fn survey_year(name: &str) -> u16 {
    let b = name.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i - start == 4 {
                let y: u16 = name[start..i].parse().unwrap_or(0);
                if (1990..2100).contains(&y) {
                    return y;
                }
            }
        } else {
            i += 1;
        }
    }
    for t in name.split(['_', '-', ' ']).rev() {
        let b = t.as_bytes();
        if b.len() == 3 && b[0].is_ascii_uppercase() && b[1..].iter().all(u8::is_ascii_digit) {
            return 2000 + t[1..].parse::<u16>().unwrap_or(0);
        }
    }
    0
}

/// A survey over one tile: which probes it covers, and its files there.
struct Candidate<'a> {
    covered: Vec<bool>,
    year: u16,
    density: f64,
    files: Vec<&'a Footprint>,
}

/// For each tile of the rectangle, the files to read: those of the one survey
/// that, of the surveys covering at least three quarters as much of the part
/// of the tile the rectangle needs as the best, is newest, then densest.
pub fn choose(foot: &[Footprint], sheet: &Sheet, rect: Rect) -> TileUrls {
    use std::collections::BTreeMap;
    let (e0, n0, e1, n1) = rect;
    let mut out = Vec::new();
    for (te, tn) in sheet.tiles(rect) {
        let (x0, y0) = (f64::from(te) * 1000.0, f64::from(tn) * 1000.0);
        let (a0, b0) = (x0.max(e0), y0.max(n0));
        let (a1, b1) = ((x0 + 1000.0).min(e1), (y0 + 1000.0).min(n1));
        let probes: Vec<(f64, f64)> = (0..5)
            .flat_map(|i| (0..5).map(move |j| (i, j)))
            .map(|(i, j)| {
                sheet.crs.inverse(
                    a0 + (a1 - a0) * (0.1 + 0.2 * f64::from(i)),
                    b0 + (b1 - b0) * (0.1 + 0.2 * f64::from(j)),
                )
            })
            .collect();
        // The part's own extent in degrees, to pick a survey's files by.
        let corners =
            [(a0, b0), (a0, b1), (a1, b0), (a1, b1)].map(|(x, y)| sheet.crs.inverse(x, y));
        let (mut s, mut w, mut n, mut e) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (lat, lon) in corners {
            s = s.min(lat);
            n = n.max(lat);
            w = w.min(lon);
            e = e.max(lon);
        }
        let meets = |f: &Footprint| f.bbox.0 < e && f.bbox.2 > w && f.bbox.1 < n && f.bbox.3 > s;
        let mut surveys: BTreeMap<&str, Candidate> = BTreeMap::new();
        for f in foot.iter().filter(|f| meets(f)) {
            let entry = surveys
                .entry(f.survey.as_str())
                .or_insert_with(|| Candidate {
                    covered: vec![false; probes.len()],
                    year: f.year,
                    density: f.density,
                    files: Vec::new(),
                });
            for (k, &(lat, lon)) in probes.iter().enumerate() {
                entry.covered[k] |= f.contains(lat, lon);
            }
            entry.files.push(f);
        }
        let covered = |c: &Candidate| c.covered.iter().filter(|&&b| b).count();
        let most = surveys.values().map(covered).max().unwrap_or(0);
        if most == 0 {
            continue;
        }
        let best = surveys
            .values()
            .filter(|c| 4 * covered(c) >= 3 * most)
            .max_by(|a, b| {
                (a.year, a.density)
                    .partial_cmp(&(b.year, b.density))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some(c) = best {
            for f in &c.files {
                out.push(((te, tn), f.url.clone()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years_come_from_the_names() {
        assert_eq!(survey_year("WA_KingCo_1_2021"), 2021);
        assert_eq!(
            survey_year("USGS_LPC_MI_31Co_Washtenaw_2016_LAS_2019"),
            2016
        );
        assert_eq!(survey_year("MN_CentralMissRiver_4_B22"), 2022);
        assert_eq!(survey_year("AK_EMRI_3_D24"), 2024);
        assert_eq!(survey_year("MA_NE_CMGP_Sandy_Z19_A1_2015"), 2015);
        assert_eq!(survey_year("USGS_LPC_MS_Tupelo_UTM16_2015_LAS_2016"), 2015);
        assert_eq!(survey_year("ARRA-NRCS_Illinois_2010"), 2010);
        assert_eq!(survey_year("GTA2015"), 2015);
        assert_eq!(survey_year("600023_52_CMM_2023"), 2023);
        assert_eq!(survey_year("Hamilton_Niagara_2021_2"), 2021);
        assert_eq!(survey_year("Edmonton"), 0);
    }

    #[test]
    fn footprints_hold_their_points() {
        let g: serde_json::Value = serde_json::from_str(
            r#"{"type":"Polygon","coordinates":[[[-83.80,42.25],[-83.70,42.25],[-83.70,42.30],[-83.80,42.30],[-83.80,42.25]]]}"#,
        )
        .unwrap();
        let f = Footprint::new("u", "s", 2010, &g, 3e9).unwrap();
        // 0.1 by 0.05 degrees at 42.3 is about 8.2 x 5.5 km.
        assert!((f.density - 3e9 / 45.4e6).abs() < 3.0, "{}", f.density);
        assert!(f.contains(42.27, -83.75));
        assert!(!f.contains(42.27, -83.65));
        assert!(Footprint::new("u", "s", 0, &serde_json::Value::Null, 0.0).is_none());
    }
}
