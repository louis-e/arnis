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

/// User agent for OSM-facing requests (Overpass, Nominatim).
///
/// The build hash rides in the comment field so `Arnis/<version>` stays the
/// leading product token, and it tells an official release apart from a local
/// build when a report only carries a version number.
pub const OSM_USER_AGENT: &str = concat!(
    "Arnis/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/louis-e/arnis; build/",
    env!("ARNIS_BUILD_HASH"),
    ")"
);

/// Function to download data using reqwest
fn download_with_reqwest(
    url: &str,
    query: &str,
    timeout_secs: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    let client: Client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent(OSM_USER_AGENT)
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
        .arg("-s") // Add silent mode to suppress output
        .arg("-A")
        .arg(OSM_USER_AGENT)
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
        .arg(format!("--user-agent={OSM_USER_AGENT}"))
        .arg(format!("{url}?data={query}"))
        .output()?;

    if !output.status.success() {
        Err(io::Error::other("Wget command failed"))
    } else {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Whether an Overpass `remark` means the result is cut short.
///
/// Overpass streams its output, so a query that runs out of time or memory
/// *after* printing has started still closes the JSON and appends a `remark`.
/// That response parses like any other, and the elements it does contain are
/// real - the ones it never got to are simply absent. Without this test a
/// half-finished continent looks exactly like a finished one.
fn remark_means_truncated(remark: &str) -> bool {
    let remark = remark.to_ascii_lowercase();
    remark.contains("runtime error")
        || remark.contains("timed out")
        || remark.contains("out of memory")
}

/// Why a downloaded Overpass body could not be used.
#[derive(Debug)]
enum ResponseError {
    /// Not valid Overpass JSON (usually a body cut off mid-token).
    Malformed(String),
    /// Valid JSON, but the server said it stopped early.
    Truncated(String),
}

impl ResponseError {
    fn message(&self) -> &str {
        match self {
            ResponseError::Malformed(m) | ResponseError::Truncated(m) => m,
        }
    }
}

/// Parses an Overpass body and rejects the ones that only look complete.
///
/// The element count is deliberately not consulted. A query that dies before
/// printing anything reports the same remark over an empty list, and that is the
/// worst case rather than a milder one: nothing distinguishes it from a bbox with
/// nothing mapped in it, so it would otherwise pass as "continue without OSM data"
/// and produce a world with no buildings and no roads at all.
fn parse_overpass_response(body: &str) -> Result<OsmData, ResponseError> {
    let mut deserializer = serde_json::Deserializer::from_reader(Cursor::new(body.as_bytes()));
    let data = OsmData::deserialize(&mut deserializer)
        .map_err(|e| ResponseError::Malformed(format!("Malformed response: {e}")))?;

    if let Some(remark) = data.remark.as_deref() {
        if remark_means_truncated(remark) {
            return Err(ResponseError::Truncated(format!(
                "Server stopped early: {remark}"
            )));
        }
    }

    Ok(data)
}

/// File extensions (case-insensitive) that select the raw OSM XML path instead of
/// Arnis's own JSON dump.
const OSM_XML_EXTENSIONS: &[&str] = &["osm", "xml"];

/// Loads OSM data from a local file. `.osm`/`.xml` files are parsed as raw OSM XML (offline,
/// avoiding Overpass size limits) and their `<bounds>` element, if present, is returned so the
/// caller can auto-derive the bounding box. Any other extension keeps the original behavior:
/// deserializing Arnis's own JSON dump (`{"elements":[...],"remark":...}`), which carries no
/// bounds (returned as None).
pub fn fetch_data_from_file(
    file: &str,
) -> Result<(OsmData, Option<LLBBox>), Box<dyn std::error::Error>> {
    println!("{} Loading data from file...", "[1/7]".bold());
    emit_gui_progress_update(1.0, "Loading data from file...");

    let is_xml = std::path::Path::new(file)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            OSM_XML_EXTENSIONS
                .iter()
                .any(|x| ext.eq_ignore_ascii_case(x))
        })
        .unwrap_or(false);

    let f: File = File::open(file)?;
    let reader: BufReader<File> = BufReader::new(f);

    if is_xml {
        crate::osm_parser::parse_osm_xml(reader)
    } else {
        let mut deserializer = serde_json::Deserializer::from_reader(reader);
        let data: OsmData = OsmData::deserialize(&mut deserializer)?;
        Ok((data, None))
    }
}

/// Main function to fetch data
pub fn fetch_data_from_overpass(
    bbox: LLBBox,
    debug: bool,
    download_method: &str,
    save_file: Option<&str>,
) -> Result<OsmData, Box<dyn std::error::Error>> {
    println!("{} Fetching data...", "[1/7]".bold());
    emit_gui_progress_update(1.0, "Downloading data...");

    // List of Overpass API servers
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

    // Generate Overpass API query for bounding box.
    // Ocean/coastal elements are excluded because ESA WorldCover satellite data
    // handles ocean detection more reliably at 10m resolution (LC_WATER class).
    // Inland water (lakes, rivers, ponds) is still fetched from OSM.
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
        nwr["shop"];
        nwr["office"];
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

    {
        // Fetch data from Overpass API.
        // Strategy:
        // 1) 50% chance: probe one random official server first.
        // 2) If the probe does not succeed, run the normal path: arnis API once,
        //    then shuffled official, then shuffled fallback servers.
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

        let first_fallback_index = request_plan
            .iter()
            .position(|(_, kind)| *kind == ServerKind::Fallback)
            .unwrap_or(request_plan.len());

        let total = request_plan.len();
        let mut last_error: Option<Box<dyn std::error::Error>> = None;
        let mut attempted_hosts: Vec<String> = Vec::new();
        // A truncation is only evidence about the *area* when it is what every
        // answering server did. Counted separately from hosts that never answered,
        // so an unreachable network is not reported as an oversized bbox.
        let mut answered = 0usize;
        let mut truncated = 0usize;
        // Two servers agreeing is enough: a size-driven timeout repeats everywhere,
        // while a merely overloaded instance deserves one second opinion. Without
        // this the user waits out all six timeouts (~30 min) for a foregone answer.
        const TRUNCATIONS_BEFORE_GIVING_UP: usize = 2;
        let (response, data): (String, OsmData) = 'server_loop: {
            for (i, (url, kind)) in request_plan.iter().enumerate() {
                let timeout_secs = if url.contains("private.coffee") {
                    120
                } else {
                    360
                };
                println!("Downloading from {url} with method {download_method}...");
                let result = match download_method {
                    "requests" => download_with_reqwest(url, &query, timeout_secs),
                    "curl" => download_with_curl(url, &query).map_err(|e| e.into()),
                    "wget" => download_with_wget(url, &query).map_err(|e| e.into()),
                    _ => download_with_reqwest(url, &query, timeout_secs), // Default to requests
                };

                // A body that arrived is not yet an answer: parse it here so a
                // truncated result falls through to the next server instead of
                // silently becoming a world with most of its areas missing.
                let result = result.and_then(|body| {
                    answered += 1;
                    match parse_overpass_response(&body) {
                        Ok(data) => Ok((body, data)),
                        Err(e) => {
                            if matches!(e, ResponseError::Truncated(_)) {
                                truncated += 1;
                            }
                            // Keep the rejected body when one was asked for; it is
                            // the only record of what the server actually sent.
                            if let Some(save_file) = save_file {
                                if let Ok(mut file) = File::create(save_file) {
                                    let _ = file.write_all(body.as_bytes());
                                }
                            }
                            eprintln!("{}", format!("Error! {}", e.message()).red().bold());
                            Err(e.message().to_string().into())
                        }
                    }
                });

                match result {
                    Ok(response) => break 'server_loop response,
                    Err(error) => {
                        if download_method != "requests" {
                            eprintln!("Request failed: {error}");
                        }
                        attempted_hosts.push(url_host(url));
                        last_error = Some(error);

                        if truncated >= TRUNCATIONS_BEFORE_GIVING_UP {
                            break;
                        }

                        if i + 1 < total {
                            let delay_secs = if *kind == ServerKind::Fallback { 5 } else { 3 };
                            println!("Retrying in {delay_secs}s (attempt {}/{total})...", i + 1);
                            std::thread::sleep(Duration::from_secs(delay_secs));
                            if i + 1 == first_fallback_index {
                                println!("Primary servers exhausted, trying fallback servers...");
                            }
                        }
                    }
                }
            }
            // All servers exhausted
            #[cfg(feature = "gui")]
            {
                let err_summary = last_error
                    .as_ref()
                    .map(|e| e.to_string().chars().take(120).collect::<String>())
                    .unwrap_or_else(|| "unknown".to_string());
                send_log(
                    LogLevel::Error,
                    &format!(
                        "Overpass fetch failed on all {} providers ({}); last error: {}",
                        attempted_hosts.len(),
                        attempted_hosts.join(", "),
                        err_summary,
                    ),
                );
            }
            // Only blame the bbox when truncation is what every answering server did.
            // If some hosts simply never replied, the size is not the established cause.
            if truncated > 0 && truncated == answered {
                eprintln!(
                    "{}",
                    "Error! The area is too large for the OpenStreetMap API: the servers \
                     that answered all stopped early. Try using a smaller area."
                        .red()
                        .bold()
                );
                emit_gui_error("Try using a smaller area.");
                // Same exit as the out-of-memory case below: the CLI unwraps this
                // Result, so returning Err here would replace the advice with a panic.
                if !is_running_with_gui() {
                    std::process::exit(1);
                }
                return Err("Data fetch failed".into());
            } else if truncated > 0 {
                eprintln!(
                    "{}",
                    format!(
                        "Error! {truncated} of {answered} answering server(s) returned partial \
                         data and the rest could not be reached. Try again, or use a smaller area."
                    )
                    .red()
                    .bold()
                );
            }
            return Err(last_error.unwrap_or_else(|| "All servers failed".into()));
        };

        if let Some(save_file) = save_file {
            let mut file: File = File::create(save_file)?;
            file.write_all(response.as_bytes())?;
            println!("API response saved to: {save_file}");
        }

        if data.is_empty() {
            // Every remark that means the server gave up was already rejected by
            // parse_overpass_response, which failed over to the next host. Reaching
            // here with no elements means the bbox genuinely has nothing mapped in
            // it, which is allowed: Arnis can still generate nature/terrain from
            // elevation + land-cover data, and unmapped natural areas are common.
            if let Some(remark) = data.remark.as_deref() {
                eprintln!(
                    "{}",
                    format!("Warning: API returned: {remark}. Continuing without OSM data.")
                        .yellow()
                        .bold()
                );
            } else {
                eprintln!(
                    "{}",
                    "Warning: OSM API returned no data for this area. Continuing with terrain/nature only."
                        .yellow()
                        .bold()
                );
            }

            if debug {
                println!("Additional debug information: {data:?}");
            }
        }

        emit_gui_progress_update(5.0, "");

        Ok(data)
    }
}

/// Fetches a short area name using Nominatim for the given lat/lon
pub fn fetch_area_name(lat: f64, lon: f64) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(OSM_USER_AGENT)
        .build()?;

    let url = format!("https://nominatim.openstreetmap.org/reverse?format=jsonv2&lat={lat}&lon={lon}&addressdetails=1");

    let resp = client.get(&url).send()?;

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

#[cfg(test)]
mod user_agent_tests {
    use super::*;

    #[test]
    fn osm_user_agent_carries_version_and_build_hash() {
        assert!(OSM_USER_AGENT.starts_with(concat!("Arnis/", env!("CARGO_PKG_VERSION"))));
        assert!(OSM_USER_AGENT.contains("build/"));

        let hash = env!("ARNIS_BUILD_HASH");
        assert!(!hash.is_empty());
        // Only ASCII, and short enough that no server's header limit is in play.
        assert!(OSM_USER_AGENT.is_ascii());
        assert!(OSM_USER_AGENT.len() < 200);
    }
}

#[cfg(test)]
mod partial_response_tests {
    use super::*;

    // Overpass streams its output, so a query that dies mid-print still closes the
    // JSON and appends a remark. The elements that made it are valid, which is what
    // makes this dangerous: a 21,000 km² request that lost most of its nodes parses
    // fine and generates a world with the areas silently missing (issue #1257).
    #[test]
    fn a_timed_out_response_with_elements_is_rejected() {
        let body = r#"{
            "version": 0.6,
            "elements": [{"type":"node","id":1,"lat":1.0,"lon":2.0}],
            "remark": "runtime error: Query timed out in \"print\" at line 5 after 360 seconds."
        }"#;
        let err = parse_overpass_response(body).expect_err("partial data must not be accepted");
        assert!(matches!(err, ResponseError::Truncated(_)));
    }

    #[test]
    fn an_out_of_memory_response_with_elements_is_rejected() {
        let body = r#"{
            "elements": [{"type":"way","id":7,"nodes":[1,2,1]}],
            "remark": "runtime error: Query run out of memory in \"query\" at line 3."
        }"#;
        assert!(matches!(
            parse_overpass_response(body),
            Err(ResponseError::Truncated(_))
        ));
    }

    // A body cut off mid-token is not JSON at all; it must fail over to the next
    // server rather than take the whole run down.
    #[test]
    fn a_body_cut_mid_token_is_malformed() {
        let body = r#"{"elements":[{"type":"node","id":1,"lat":1.0,"lon":-8"#;
        assert!(matches!(
            parse_overpass_response(body),
            Err(ResponseError::Malformed(_))
        ));
    }

    // The worst case, not a milder one: a query that dies before printing anything
    // reports the same remark over an empty list. Accepting it looks exactly like a
    // bbox with nothing mapped in it, and the caller would carry on and build a world
    // with no buildings AND no roads.
    #[test]
    fn an_empty_response_that_stopped_early_is_rejected() {
        // The inner quotes stay JSON-escaped so these parse as real Overpass bodies;
        // an unescaped one would be rejected as malformed and prove nothing.
        for remark in [
            r#"runtime error: Query timed out in \"query\" at line 3 after 360 seconds."#,
            r#"runtime error: Query run out of memory in \"query\" at line 3."#,
        ] {
            let body = format!(r#"{{"elements":[],"remark":"{remark}"}}"#);
            assert!(
                matches!(
                    parse_overpass_response(&body),
                    Err(ResponseError::Truncated(_))
                ),
                "empty + {remark} must not pass as an empty area"
            );
        }
    }

    // The one empty case that is genuinely fine: no remark at all. Arnis can still
    // build terrain and nature, so this must keep working.
    #[test]
    fn an_empty_response_with_no_remark_is_accepted() {
        let data = parse_overpass_response(r#"{"elements":[]}"#)
            .expect("an unmapped bbox is not a failure");
        assert!(data.is_empty());
    }

    // A remark that is not an error must not be mistaken for one, whether or not
    // the response carried elements.
    #[test]
    fn a_benign_remark_is_accepted_either_way() {
        let with = r#"{"elements":[{"type":"node","id":1,"lat":1.0,"lon":2.0}],"remark":"Please note the data is from OpenStreetMap"}"#;
        let without = r#"{"elements":[],"remark":"Please note the data is from OpenStreetMap"}"#;
        assert!(parse_overpass_response(with).is_ok());
        assert!(parse_overpass_response(without).is_ok());
    }

    #[test]
    fn a_complete_response_is_accepted() {
        let body = r#"{"elements":[{"type":"node","id":1,"lat":1.0,"lon":2.0}]}"#;
        let data = parse_overpass_response(body).expect("a clean response must be accepted");
        assert!(!data.is_empty());
    }

    #[test]
    fn only_error_remarks_count_as_truncation() {
        assert!(remark_means_truncated(
            "runtime error: Query timed out in \"print\""
        ));
        assert!(remark_means_truncated("Query run out of memory"));
        assert!(!remark_means_truncated(
            "Please note the data is from OpenStreetMap"
        ));
    }
}

#[cfg(test)]
mod fetch_from_file_tests {
    use super::*;

    // A `.osm` file must route to the raw-XML parser, which surfaces the document's <bounds>
    // element; the JSON path returns None, so a Some result here proves the extension routing.
    #[test]
    fn osm_extension_routes_to_xml_parser_and_returns_bounds() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.osm");
        let (data, bounds) = fetch_data_from_file(path).expect("fixture .osm should load");

        // Exactly the fixture's <bounds minlat=.. minlon=.. maxlat=.. maxlon=..> element.
        assert_eq!(
            bounds,
            Some(LLBBox::new(54.63, 9.93, 54.632, 9.933).unwrap())
        );

        // The fixture's 8 nodes + 2 ways were parsed. Element internals are private to
        // osm_parser (exact-count / tag assertions live in its osm_xml_tests), so the parse
        // is asserted here through the public surface.
        assert!(!data.is_empty());

        // The returned box is the explicit <bounds>, not the node-coordinate extent: the 8
        // nodes span a strictly smaller box, so this confirms <bounds> was surfaced instead of
        // the OsmData::bounds() fallback.
        assert_eq!(
            data.bounds(),
            Some(LLBBox::new(54.6302, 9.9302, 54.6318, 9.9325).unwrap())
        );
    }

    // Any non-.osm/.xml extension keeps the original Arnis JSON-dump path, which carries no
    // <bounds> (None). Paired with the .osm case above, this proves routing keys off the extension.
    #[test]
    fn json_extension_keeps_json_path_and_has_no_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dump.json");
        std::fs::write(
            &path,
            r#"{"elements":[{"type":"node","id":1,"lat":1.0,"lon":2.0}],"remark":null}"#,
        )
        .unwrap();

        let (data, bounds) =
            fetch_data_from_file(path.to_str().unwrap()).expect("JSON dump should load");
        assert!(bounds.is_none());
        assert!(!data.is_empty());
    }
}
