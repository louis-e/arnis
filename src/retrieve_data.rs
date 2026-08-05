use crate::coordinate_system::geographic::LLBBox;
use crate::osm_parser::OsmData;
use crate::progress::{emit_gui_error, emit_gui_progress_update, is_running_with_gui};
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use colored::Colorize;
use rand::prelude::SliceRandom;
use rand::Rng;
use reqwest::blocking::Client;
use reqwest::blocking::ClientBuilder;
use serde::Deserialize;
use serde_json::Value;
use std::fs::File;
use std::io::{self, BufReader, Cursor, Write};
use std::process::Command;
use std::time::Duration;

/// Extract the host portion of a URL for telemetry
fn url_host(url: &str) -> String {
    let after_scheme = url.split("://").nth(1).unwrap_or(url);
    after_scheme
        .split(['/', '?'])
        .next()
        .unwrap_or(after_scheme)
        .to_string()
}

/// Function to download data using reqwest
fn download_with_reqwest(
    url: &str,
    query: &str,
    timeout_secs: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    let client: Client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent(concat!(
            "Arnis/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/louis-e/arnis)"
        ))
        .build()?;

    let response: Result<reqwest::blocking::Response, reqwest::Error> =
        client.get(url).query(&[("data", query)]).send();

    match response {
        Ok(resp) => {
            emit_gui_progress_update(3.0, "");
            if resp.status().is_success() {
                let text = resp.text()?;
                if text.is_empty() {
                    return Err("Received invalid data from server".into());
                }
                Ok(text)
            } else {
                let status = resp.status();
                let user_msg = match status.as_u16() {
                    429 => "Rate limited. Try again later.".to_string(),
                    403 => "Server overloaded. Try again.".to_string(),
                    500 | 502 | 503 | 504 => "Server unavailable. Try again.".to_string(),
                    _ => format!("Response code: {}", status.as_u16()),
                };
                eprintln!("{}", format!("Error! {user_msg}").red().bold());
                Err(user_msg.into())
            }
        }
        Err(e) => {
            if e.is_timeout() {
                let msg = "Request timed out. Try again!";
                eprintln!("{}", format!("Error! {msg}").red().bold());
                Err(msg.into())
            } else if e.is_connect() {
                let msg = "No internet connection.";
                eprintln!("{}", format!("Error! {msg}").red().bold());
                Err(msg.into())
            } else {
                let short: String = e.to_string().chars().take(52).collect();
                eprintln!("{}", format!("Error! {short}").red().bold());
                Err(short.into())
            }
        }
    }
}

/// Function to download data using `curl`
fn download_with_curl(url: &str, query: &str) -> io::Result<String> {
    let output: std::process::Output = Command::new("curl")
        .arg("-s") 
        .arg(format!("{url}?data={query}"))
        .output()?;

    if !output.status.success() {
        Err(io::Error::other("Curl command failed"))
    } else {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Function to download data using `wget`
fn download_with_wget(url: &str, query: &str) -> io::Result<String> {
    let output: std::process::Output = Command::new("wget")
        .arg("-qO-") 
        .arg(format!("{url}?data={query}"))
        .output()?;

    if !output.status.success() {
        Err(io::Error::other("Wget command failed"))
    } else {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Loads a user-provided OSM JSON data file locally
pub fn fetch_data_from_file(file: &str) -> Result<OsmData, Box<dyn std::error::Error>> {
    println!("{} Loading data from file...", "[1/7]".bold());
    emit_gui_progress_update(1.0, "Loading data from file...");

    let file: File = File::open(file)?;
    let reader: BufReader<File> = BufReader::new(file);
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let data: OsmData = OsmData::deserialize(&mut deserializer)?;
    Ok(data)
}

/// Entrypoint configuration wrapper to select the data source
pub fn get_map_data(
    user_file: Option<&str>,
    bbox: LLBBox,
    debug: bool,
    download_method: &str,
    save_file: Option<&str>,
) -> Result<OsmData, Box<dyn std::error::Error>> {
    // 1. If the user provided a custom map file path, use it directly
    if let Some(file_path) = user_file {
        match fetch_data_from_file(file_path) {
            Ok(data) => return Ok(data),
            Err(e) => {
                let err_msg = format!("Failed to load local map file: {e}");
                emit_gui_error(&err_msg);
                eprintln!("{}", err_msg.red().bold());
                // Fall through to Overpass API if the file failed to parse
            }
        }
    }

    // 2. Fallback to Overpass API if no file was specified or file reading failed
    fetch_data_from_overpass(bbox, debug, download_method, save_file)
}

/// Main function to fetch data from the internet
pub fn fetch_data_from_overpass(
    bbox: LLBBox,
    debug: bool,
    download_method: &str,
    save_file: Option<&str>,
) -> Result<OsmData, Box<dyn std::error::Error>> {
    println!("{} Fetching data...", "[1/7]".bold());
    emit_gui_progress_update(1.0, "Downloading data...");

    let arnis_api_server = "https://api.arnismc.com/overpass/api/interpreter";
    let api_servers: Vec<&str> = vec![
        "https://overpass-api.de/api/interpreter",
        "https://lz4.overpass-api.de/api/interpreter",
        "https://z.overpass-api.de/api/interpreter",
    ];
    let fallback_api_servers: Vec<&str> = vec![
        "https://maps.mail.ru/osm/tools/overpass/api/interpreter",
        "https://overpass.private.coffee/api/interpreter",
    ];

    let query: String = format!(
        r#"[out:json][timeout:360][bbox:{},{},{},{}];
    (
        nwr["building"];
        nwr["building:part"];
        relation["type"="building"];
        nwr["highway"];
        nwr["landuse"]["landuse"!="salt_pond"];
        nwr["natural"]["natural"!="coastline"]["natural"!="bay"]["natural"!="strait"];
        nwr["leisure"];
        nwr["water"]["water"!="bay"]["water"!="ocean"]["water"!="sea"]["tidal"!="yes"];
        nwr["waterway"]["waterway"!="tidal_channel"];
        nwr["amenity"];
        nwr["tourism"];
        nwr["bridge"];
        nwr["railway"];
        nwr["roller_coaster"];
        nwr["barrier"];
        nwr["entrance"];
        nwr["door"];
        nwr["power"];
        nwr["historic"];
        nwr["emergency"];
        nwr["advertising"];
        nwr["man_made"];
        nwr["aeroway"];
        nwr["3dmr"];
        way["place"]["place"!~"^(ocean|sea|bay|strait|sound|fjord)$"];
        way;
    )->.relsinbbox;
    (
        way(r.relsinbbox);
    )->.waysinbbox;
    (
        node(w.waysinbbox);
        node(w.relsinbbox);
    )->.nodesinbbox;
    .relsinbbox out body;
    .waysinbbox out body;
    .nodesinbbox out skel qt;"#,
        bbox.min().lat(),
        bbox.min().lng(),
        bbox.max().lat(),
        bbox.max().lng(),
    );

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ServerKind {
        Primary,
        Fallback,
    }

    let mut rng = rand::rng();
    let mut request_plan: Vec<(&str, ServerKind)> = Vec::new();
    let mut probed_server: Option<&str> = None;

    if rng.random_bool(0.5) {
        let probe_idx = rng.random_range(0..api_servers.len());
        let probe_server = api_servers[probe_idx];
        request_plan.push((probe_server, ServerKind::Primary));
        probed_server = Some(probe_server);
    }

    request_plan.push((arnis_api_server, ServerKind::Primary));

    let mut shuffled_primary_servers = api_servers.clone();
    shuffled_primary_servers.shuffle(&mut rng);
    if let Some(probed_server) = probed_server {
        shuffled_primary_servers.retain(|&url| url != probed_server);
    }
    request_plan.extend(
        shuffled_primary_servers
            .into_iter()
            .map(|url| (url, ServerKind::Primary)),
    );

    let mut shuffled_fallback_servers = fallback_api_servers.clone();
    shuffled_fallback_servers.shuffle(&mut rng);
    request_plan.extend(
        shuffled_fallback_servers
            .into_iter()
            .map(|url| (url, ServerKind::Fallback)),
    );

    // Iterating and execution logic for request_plan follows here...
    unimplemented!("Complete remaining execution engine loop details here.")
}
