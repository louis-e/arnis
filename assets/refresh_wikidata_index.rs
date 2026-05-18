//! Pre-bake `assets/wikidata_3d_models.json`: SPARQL P4896 ∩ P625 → Commons API → permissive-only filter.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

const SPARQL_ENDPOINT: &str = "https://query.wikidata.org/sparql";
const COMMONS_API: &str = "https://commons.wikimedia.org/w/api.php";
const USER_AGENT: &str = concat!(
    "Arnis/",
    env!("CARGO_PKG_VERSION"),
    " refresh_wikidata_index (+https://github.com/louis-e/arnis)"
);
const BATCH_SIZE: usize = 50;
const OUTPUT_PATH: &str = "assets/wikidata_3d_models.json";

/// QIDs to skip. OSM tagging produces better output than the Wikimedia model.
const DENY_LIST: &[&str] = &[
    "Q9188", // Empire State Building — wrong height_m (1500), generic block model
];

#[derive(Serialize, Debug, Clone)]
struct ModelEntry {
    label: String,
    url: String,
    license: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    license_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height_m: Option<f64>,
}

#[derive(Serialize, Debug)]
struct Index {
    version: u32,
    generated_at: String,
    source: &'static str,
    license_policy: &'static str,
    models: BTreeMap<String, ModelEntry>,
}

#[derive(Deserialize, Debug)]
struct SparqlResult {
    results: SparqlBindings,
}

#[derive(Deserialize, Debug)]
struct SparqlBindings {
    bindings: Vec<SparqlRow>,
}

#[derive(Deserialize, Debug)]
struct SparqlRow {
    item: SparqlValue,
    #[serde(rename = "itemLabel")]
    item_label: Option<SparqlValue>,
    model: SparqlValue,
    height: Option<SparqlValue>,
}

#[derive(Deserialize, Debug)]
struct SparqlValue {
    value: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(USER_AGENT)
        .build()?;

    eprintln!("[1/3] Querying Wikidata SPARQL...");
    let rows = fetch_sparql_rows(&client)?;
    eprintln!("       got {} candidates", rows.len());

    eprintln!("[2/3] Batching Commons API for license metadata...");
    type Candidate = (String, String, String, Option<f64>, Option<String>);
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut rejected_denylist = 0usize;
    for row in &rows {
        let qid = row.item.value.rsplit('/').next().unwrap_or("").to_string();
        if !qid.starts_with('Q') {
            continue;
        }
        if DENY_LIST.contains(&qid.as_str()) {
            rejected_denylist += 1;
            continue;
        }
        let label = row
            .item_label
            .as_ref()
            .map(|v| v.value.clone())
            .unwrap_or_else(|| qid.clone());
        let url = row.model.value.replace("http://", "https://");
        let height_m = row
            .height
            .as_ref()
            .and_then(|v| v.value.parse::<f64>().ok());
        let filename = filename_from_filepath_url(&url);
        candidates.push((qid, label, url, height_m, filename));
    }
    candidates.retain(|(_, _, _, _, f)| f.is_some());

    let mut models: BTreeMap<String, ModelEntry> = BTreeMap::new();
    let mut kept = 0usize;
    let mut rejected_unknown = 0usize;
    let mut rejected_sharealike = 0usize;
    let mut rejected_restricted = 0usize;
    let mut rejected_missing = 0usize;

    let chunks: Vec<_> = candidates.chunks(BATCH_SIZE).collect();
    for (i, batch) in chunks.iter().enumerate() {
        eprintln!("       batch {}/{}", i + 1, chunks.len());
        let titles: Vec<String> = batch
            .iter()
            .map(|(_, _, _, _, f)| format!("File:{}", f.as_ref().unwrap()))
            .collect();
        let info_map = fetch_commons_metadata(&client, &titles)?;

        for (qid, label, url, height_m, filename) in *batch {
            let title = format!("File:{}", filename.as_ref().unwrap());
            let Some(meta) = info_map.get(&title) else {
                rejected_missing += 1;
                continue;
            };
            let license = meta.license.clone().unwrap_or_default();
            let class = classify(&license);
            match class {
                LicenseClass::Permissive => {
                    kept += 1;
                    models.insert(
                        qid.clone(),
                        ModelEntry {
                            label: label.clone(),
                            url: url.clone(),
                            license,
                            license_url: meta.license_url.clone(),
                            artist: meta.artist.clone().map(strip_html),
                            height_m: *height_m,
                        },
                    );
                }
                LicenseClass::ShareAlike => rejected_sharealike += 1,
                LicenseClass::Restricted => rejected_restricted += 1,
                LicenseClass::Unknown => rejected_unknown += 1,
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let index = Index {
        version: 1,
        generated_at: chrono_now(),
        source:
            "https://query.wikidata.org/sparql (P4896 ∩ P625) + commons.wikimedia.org imageinfo",
        license_policy: "permissive-only: CC0, Public Domain, CC BY (any version)",
        models,
    };

    eprintln!("[3/3] Writing {OUTPUT_PATH}");
    eprintln!(
        "       kept: {kept}    sharealike-rejected: {rejected_sharealike}    restricted-rejected: {rejected_restricted}    unknown-rejected: {rejected_unknown}    metadata-missing: {rejected_missing}    denylist-rejected: {rejected_denylist}"
    );

    std::fs::create_dir_all("assets")?;
    let json = serde_json::to_string_pretty(&index)?;
    std::fs::write(OUTPUT_PATH, json)?;
    eprintln!("Done.");
    Ok(())
}

fn fetch_sparql_rows(
    client: &reqwest::blocking::Client,
) -> Result<Vec<SparqlRow>, Box<dyn std::error::Error>> {
    // `psn:P2048` is the SI-normalized statement value (metres); `wdt:` would give the raw unit.
    let query = r#"
SELECT ?item ?itemLabel ?model ?height WHERE {
  ?item wdt:P4896 ?model .
  ?item wdt:P625 ?coord .
  FILTER(STRENDS(STR(?model), ".stl"))
  OPTIONAL { ?item p:P2048/psn:P2048/wikibase:quantityAmount ?height . }
  SERVICE wikibase:label { bd:serviceParam wikibase:language "en" . }
}"#;
    let resp = client
        .get(SPARQL_ENDPOINT)
        .query(&[("query", query), ("format", "json")])
        .header("Accept", "application/sparql-results+json")
        .send()?;
    if !resp.status().is_success() {
        return Err(format!("SPARQL HTTP {}", resp.status()).into());
    }
    let parsed: SparqlResult = resp.json()?;
    Ok(parsed.results.bindings)
}

#[derive(Debug, Clone, Default)]
struct CommonsMeta {
    license: Option<String>,
    license_url: Option<String>,
    artist: Option<String>,
}

fn fetch_commons_metadata(
    client: &reqwest::blocking::Client,
    titles: &[String],
) -> Result<BTreeMap<String, CommonsMeta>, Box<dyn std::error::Error>> {
    let titles_param = titles.join("|");
    let resp = client
        .get(COMMONS_API)
        .query(&[
            ("action", "query"),
            ("format", "json"),
            ("titles", &titles_param),
            ("prop", "imageinfo"),
            ("iiprop", "extmetadata|mime"),
            ("iiextmetadatafilter", "LicenseShortName|LicenseUrl|Artist"),
        ])
        .send()?;
    let v: serde_json::Value = resp.json()?;
    let mut out = BTreeMap::new();
    if let Some(pages) = v.pointer("/query/pages").and_then(|p| p.as_object()) {
        for page in pages.values() {
            let title = page.get("title").and_then(|t| t.as_str()).unwrap_or("");
            if title.is_empty() {
                continue;
            }
            let Some(info) = page.pointer("/imageinfo/0") else {
                continue;
            };
            let ext = info.pointer("/extmetadata");
            let license = ext
                .and_then(|e| e.pointer("/LicenseShortName/value"))
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let license_url = ext
                .and_then(|e| e.pointer("/LicenseUrl/value"))
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let artist = ext
                .and_then(|e| e.pointer("/Artist/value"))
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            out.insert(
                title.to_string(),
                CommonsMeta {
                    license,
                    license_url,
                    artist,
                },
            );
        }
    }
    Ok(out)
}

enum LicenseClass {
    Permissive,
    ShareAlike,
    Restricted,
    Unknown,
}

fn classify(s: &str) -> LicenseClass {
    let l = s.to_ascii_lowercase();
    if l.contains("share") || l.contains("by-sa") || l.contains("by sa") || l.contains("gfdl") {
        return LicenseClass::ShareAlike;
    }
    if l.contains("by-nc")
        || l.contains("by nc")
        || l.contains("by-nd")
        || l.contains("by nd")
        || l.contains("noncommercial")
        || l.contains("non-commercial")
        || l.contains("noderivatives")
        || l.contains("no-derivatives")
        || l.contains("no derivatives")
    {
        return LicenseClass::Restricted;
    }
    if l.contains("cc0")
        || l.contains("public domain")
        || l == "pd"
        || l.contains("cc by")
        || l.contains("attribution")
    {
        return LicenseClass::Permissive;
    }
    LicenseClass::Unknown
}

fn filename_from_filepath_url(url: &str) -> Option<String> {
    let tail = url.rsplit('/').next()?;
    let decoded = percent_decode(tail);
    Some(decoded.replace('_', " "))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn strip_html(input: String) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    decode_html_entities(&collapsed)
}

fn decode_html_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        if let Some(semi) = after.find(';') {
            let entity = &after[..semi];
            let decoded = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16)
                    .ok()
                    .and_then(char::from_u32),
                e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(c) = decoded {
                out.push(c);
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn chrono_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    let hh = (secs % 86400) / 3600;
    let mm = (secs % 3600) / 60;
    let ss = secs % 60;
    format!("{year:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}
