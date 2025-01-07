use crate::progress::{emit_gui_error, emit_gui_progress_update, is_running_with_gui};
use colored::Colorize;
use rand::seq::SliceRandom;
use reqwest::blocking::Client;
use reqwest::blocking::ClientBuilder;
use serde_json::Value;
use std::fs::File;
use std::io::{self, BufReader, Write};
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
                eprintln!("{}", format!("Error! {:.52}", e).red().bold());
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
        .arg(format!("{}?data={}", url, query))
        .output()?;

    if !output.status.success() {
        Err(io::Error::new(io::ErrorKind::Other, "Curl command failed"))
    } else {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Function to download data using `wget`
fn download_with_wget(url: &str, query: &str) -> io::Result<String> {
    let output: std::process::Output = Command::new("wget")
        .arg("-qO-") // Use `-qO-` to output the result directly to stdout
        .arg(format!("{}?data={}", url, query))
        .output()?;

    if !output.status.success() {
        Err(io::Error::new(io::ErrorKind::Other, "Wget command failed"))
    } else {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Main function to fetch data
pub fn fetch_data(
    bbox: (f64, f64, f64, f64),
    file: Option<&str>,
    debug: bool,
    download_method: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    println!("{} Fetching data...", "[1/5]".bold());
    emit_gui_progress_update(1.0, "Fetching data...");

    // List of Overpass API servers
    let api_servers: Vec<&str> = vec![
        "https://overpass-api.de/api/interpreter",
        "https://lz4.overpass-api.de/api/interpreter",
        "https://z.overpass-api.de/api/interpreter",
        //"https://overpass.kumi.systems/api/interpreter", // This server is not reliable anymore
        //"https://overpass.private.coffee/api/interpreter", // This server is not reliable anymore
        "https://overpass.osm.ch/api/interpreter",
    ];
    let url: &&str = api_servers.choose(&mut rand::thread_rng()).unwrap();

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
        bbox.1, bbox.0, bbox.3, bbox.2
    );

    if let Some(file) = file {
        // Load data from file
        let file: File = File::open(file)?;
        let reader: BufReader<File> = BufReader::new(file);
        let data: Value = serde_json::from_reader(reader)?;
        Ok(data)
    } else {
        // Fetch data from Overpass API
        let response: String = match download_method {
            "requests" => download_with_reqwest(url, &query)?,
            "curl" => download_with_curl(url, &query)?,
            "wget" => download_with_wget(url, &query)?,
            _ => download_with_reqwest(url, &query)?, // Default to requests
        };

        let data: Value = serde_json::from_str(&response)?;

        if data["elements"]
            .as_array()
            .map_or(0, |elements: &Vec<Value>| elements.len())
            == 0
        {
            if let Some(remark) = data["remark"].as_str() {
                // Check if the remark mentions memory or other runtime errors
                if remark.contains("runtime error") && remark.contains("out of memory") {
                    eprintln!("{}", "Error! The query ran out of memory on the Overpass API server. Try using a smaller area.".red().bold());
                    emit_gui_error("Try using a smaller area.");
                } else {
                    // Handle other Overpass API errors if present in the remark field
                    eprintln!(
                        "{}",
                        format!("Error! API returned: {}", remark).red().bold()
                    );
                    emit_gui_error(&format!("API returned: {}", remark));
                }
            } else {
                // General case for when there are no elements and no specific remark
                eprintln!(
                    "{}",
                    "Error! No data available in this region.".red().bold()
                );
                emit_gui_error("No data available in this region.");
            }

            if debug {
                println!("Additional debug information: {}", data);
            }

            if !is_running_with_gui() {
                std::process::exit(1);
            } else {
                return Err("Data fetch failed".into());
            }
        }

        // If debug is enabled, write data to file
        if debug {
            let mut file: File = File::create("export.json")?;
            file.write_all(response.as_bytes())?;
        }

        emit_gui_progress_update(5.0, "");

        Ok(data)
    }
}
