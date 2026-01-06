use crate::coordinate_system::geographic::LLBBox;
use crate::osm_parser::OsmData;
use crate::progress::{emit_gui_error, emit_gui_progress_update, is_running_with_gui};
use colored::Colorize;
use rand::seq::SliceRandom;
use reqwest::blocking::Client;
use reqwest::blocking::ClientBuilder;
use serde::Deserialize;
use serde_json::Value;
use std::fs::File;
use std::io::{self, BufReader, Cursor, Write};
use std::process::Command;
use std::time::Duration;

/// Function to download data using reqwest
fn download_with_reqwest(url: &str, query: &str) -> Result<String, Box<dyn std::error::Error>> {
    let client: Client = ClientBuilder::new()
        .timeout(Duration::from_secs(360))
        .build()?;

    let response: Result<reqwest::blocking::Response, reqwest::Error> =
        client.get(url).query(&[("data", query)]).send();

    match response {
        Ok(resp) => {
            emit_gui_progress_update(3.0, "Downloading data...");
            if resp.status().is_success() {
                let text = resp.text()?;
                if text.is_empty() {
                    return Err("Error! Received invalid from server".into());
                }
                Ok(text)
            } else {
                Err(format!("Error! Received response code: {}", resp.status()).into())
            }
        }
        Err(e) => {
            if e.is_timeout() {
                eprintln!(
                    "{}",
                    "Error! Request timed out. Try selecting a smaller area."
                        .red()
                        .bold()
                );
                emit_gui_error("Request timed out. Try selecting a smaller area.");
            } else {
                eprintln!("{}", format!("Error! {e:.52}").red().bold());
                emit_gui_error(&format!("{:.52}", e.to_string()));
            }
            // Always propagate errors
            Err(e.into())
        }
    }
}

/// Function to download data using `curl`
fn download_with_curl(url: &str, query: &str) -> io::Result<String> {
    let output: std::process::Output = Command::new("curl")
        .arg("-s") // Add silent mode to suppress output
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
        .arg("-qO-") // Use `-qO-` to output the result directly to stdout
        .arg(format!("{url}?data={query}"))
        .output()?;

    if !output.status.success() {
        Err(io::Error::other("Wget command failed"))
    } else {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

pub fn fetch_data_from_file(file: &str) -> Result<OsmData, Box<dyn std::error::Error>> {
    println!("{} Loading data from file...", "[1/7]".bold());
    emit_gui_progress_update(1.0, "Loading data from file...");

    let file: File = File::open(file)?;
    let reader: BufReader<File> = BufReader::new(file);
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let data: OsmData = OsmData::deserialize(&mut deserializer)?;
    Ok(data)
}

/// Main function to fetch data
pub fn fetch_data_from_overpass(
    bbox: LLBBox,
    debug: bool,
    download_method: &str,
    save_file: Option<&str>,
) -> Result<OsmData, Box<dyn std::error::Error>> {
    println!("{} Fetching data...", "[1/7]".bold());
    emit_gui_progress_update(1.0, "Fetching data...");

    // List of Overpass API servers
    let api_servers: Vec<&str> = vec![
        "https://overpass-api.de/api/interpreter",
        "https://lz4.overpass-api.de/api/interpreter",
        "https://z.overpass-api.de/api/interpreter",
        //"https://overpass.kumi.systems/api/interpreter", // This server is not reliable anymore
        //"https://overpass.private.coffee/api/interpreter", // This server is not reliable anymore
    ];
    let fallback_api_servers: Vec<&str> =
        vec!["https://maps.mail.ru/osm/tools/overpass/api/interpreter"];
    let mut url: &&str = api_servers.choose(&mut rand::thread_rng()).unwrap();

    // Generate Overpass API query for bounding box
    let query: String = format!(
        r#"[out:json][timeout:360][bbox:{},{},{},{}];
    (
        nwr["building"];
        nwr["highway"];
        nwr["landuse"];
        nwr["natural"];
        nwr["leisure"];
        nwr["water"];
        nwr["waterway"];
        nwr["amenity"];
        nwr["tourism"];
        nwr["bridge"];
        nwr["railway"];
        nwr["barrier"];
        nwr["entrance"];
        nwr["door"];
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

    {
        // Fetch data from Overpass API
        let mut attempt = 0;
        let max_attempts = 1;
        let response: String = loop {
            println!("Downloading from {url} with method {download_method}...");
            let result = match download_method {
                "requests" => download_with_reqwest(url, &query),
                "curl" => download_with_curl(url, &query).map_err(|e| e.into()),
                "wget" => download_with_wget(url, &query).map_err(|e| e.into()),
                _ => download_with_reqwest(url, &query), // Default to requests
            };

            match result {
                Ok(response) => break response,
                Err(error) => {
                    if attempt >= max_attempts {
                        return Err(error);
                    }

                    println!("Request failed. Switching to fallback url...");
                    url = fallback_api_servers
                        .choose(&mut rand::thread_rng())
                        .unwrap();
                    attempt += 1;
                }
            }
        };

        if let Some(save_file) = save_file {
            let mut file: File = File::create(save_file)?;
            file.write_all(response.as_bytes())?;
            println!("API response saved to: {save_file}");
        }

        let mut deserializer =
            serde_json::Deserializer::from_reader(Cursor::new(response.as_bytes()));
        let data: OsmData = OsmData::deserialize(&mut deserializer)?;

        if data.elements.is_empty() {
            if let Some(remark) = data.remark.as_deref() {
                // Check if the remark mentions memory or other runtime errors
                if remark.contains("runtime error") && remark.contains("out of memory") {
                    eprintln!("{}", "Error! The query ran out of memory on the Overpass API server. Try using a smaller area.".red().bold());
                    emit_gui_error("Try using a smaller area.");
                } else {
                    // Handle other Overpass API errors if present in the remark field
                    eprintln!("{}", format!("Error! API returned: {remark}").red().bold());
                    emit_gui_error(&format!("API returned: {remark}"));
                }
            } else {
                // General case for when there are no elements and no specific remark
                eprintln!(
                    "{}",
                    "Error! API returned no data. Please try again!"
                        .red()
                        .bold()
                );
                emit_gui_error("API returned no data. Please try again!");
            }

            if debug {
                println!("Additional debug information: {data:?}");
            }

            if !is_running_with_gui() {
                std::process::exit(1);
            } else {
                return Err("Data fetch failed".into());
            }
        }

        emit_gui_progress_update(5.0, "");

        Ok(data)
    }
}

/// Fetches a short area name using Nominatim for the given lat/lon
pub fn fetch_area_name(lat: f64, lon: f64) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let client = Client::builder().timeout(Duration::from_secs(20)).build()?;

    let url = format!("https://nominatim.openstreetmap.org/reverse?format=jsonv2&lat={lat}&lon={lon}&addressdetails=1");

    let resp = client.get(&url).header("User-Agent", "arnis-rust").send()?;

    if !resp.status().is_success() {
        return Ok(None);
    }

    let json: Value = resp.json()?;

    if let Some(address) = json.get("address") {
        let fields = ["city", "town", "village", "county", "borough", "suburb"];
        for field in fields.iter() {
            if let Some(name) = address.get(*field).and_then(|v| v.as_str()) {
                let mut name_str = name.to_string();

                // Remove "City of " prefix
                if name_str.to_lowercase().starts_with("city of ") {
                    name_str = name_str[name_str.find(" of ").unwrap() + 4..].to_string();
                }

                return Ok(Some(name_str));
            }
        }
    }

    Ok(None)
}
