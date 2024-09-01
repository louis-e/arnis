use reqwest::blocking::Client;
use serde_json::Value;
use std::fs::File;
use std::io::{self, BufReader, Write};
use std::process::Command;
use rand::seq::SliceRandom;

/// Function to download data using the `reqwest` crate
fn download_with_reqwest(url: &str, query: &str) -> Result<String, Box<dyn std::error::Error>> {
    let client: Client = Client::new();
    let response: String = client.get(url).query(&[("data", query)]).send()?.text()?;
    Ok(response)
}

/// Function to download data using `curl`
fn download_with_curl(url: &str, query: &str) -> io::Result<String> {
    let output: std::process::Output = Command::new("curl")
        .arg("-s")  // Add silent mode to suppress output
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
        .arg("-qO-")  // Use `-qO-` to output the result directly to stdout
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
    println!("Fetching data...");

    // List of Overpass API servers
    let api_servers: Vec<&str> = vec![
        "https://overpass-api.de/api/interpreter",
        "https://lz4.overpass-api.de/api/interpreter",
        "https://z.overpass-api.de/api/interpreter",
        "https://overpass.kumi.systems/api/interpreter",
        "https://overpass.private.coffee/api/interpreter",
    ];
    let url: &&str = api_servers.choose(&mut rand::thread_rng()).unwrap();

    // Generate Overpass API query for bounding box
    let query: String = format!(
        r#"[out:json][bbox:{},{},{},{}];
    (
        nwr["building"];
        nwr["highway"];
        nwr["landuse"];
        nwr["natural"];
        nwr["leisure"];
        nwr["waterway"];
        nwr["amenity"];
        nwr["bridge"];
        nwr["railway"];
        nwr["barrier"];
        nwr["entrance"];
        nwr["door"];
    )->.waysinbbox;
    (
        node(w.waysinbbox);
    )->.nodesinbbox;
    .waysinbbox out body;
    .nodesinbbox out skel qt;"#,
        bbox.1, bbox.0, bbox.3, bbox.2
    );

    if debug {
        println!("OSM Query: {}", query);
    }

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

        if data["elements"].as_array().map_or(0, |elements: &Vec<Value>| elements.len()) == 0 {
            println!("Error! No data available");
            std::process::exit(1);
        }

        // If debug is enabled, write data to file
        if debug {
            let mut file: File = File::create("export.json")?;
            file.write_all(response.as_bytes())?;
        }

        Ok(data)
    }
}
