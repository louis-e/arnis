use log::error;
use reqwest::blocking::Client;
use serde::Serialize;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

/// Telemetry endpoint URL
const TELEMETRY_URL: &str = "https://arnismc.com/telemetry/report_telemetry.php";

/// Global flag to store user's telemetry consent
static TELEMETRY_CONSENT: AtomicBool = AtomicBool::new(false);

/// Every `Client` owns a tokio runtime with its own I/O driver, so building one
/// per message let a burst of warnings exhaust the OS I/O resources. Warnings
/// come from per-tile fetch loops, so the burst is not hypothetical.
static SHARED_CLIENT: OnceLock<Option<Client>> = OnceLock::new();

fn shared_client() -> Option<&'static Client> {
    SHARED_CLIENT
        .get_or_init(|| match build_client(TELEMETRY_TIMEOUT) {
            Ok(client) => Some(client),
            Err(e) => {
                // Runs once, so this cannot spam the log.
                error!("Telemetry disabled: HTTP client build failed: {e}");
                None
            }
        })
        .as_ref()
}

/// Timeout for telemetry requests. Crash reports use a shorter one because the
/// panic hook blocks on them while the process is on its way down.
const TELEMETRY_TIMEOUT: Duration = Duration::from_secs(15);
const CRASH_REPORT_TIMEOUT: Duration = Duration::from_secs(10);

fn build_client(timeout: Duration) -> Result<Client, reqwest::Error> {
    Client::builder().timeout(timeout).build()
}

/// Ceiling on log sends in flight; extra messages are dropped, not queued. Logs
/// are emitted from per-tile loops, so an unbounded burst is one thread and one
/// socket per warning.
const MAX_SENDS_IN_FLIGHT: usize = 4;
static SENDS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

struct SendSlot;

impl SendSlot {
    fn acquire() -> Option<Self> {
        let mut n = SENDS_IN_FLIGHT.load(Ordering::Acquire);
        while n < MAX_SENDS_IN_FLIGHT {
            match SENDS_IN_FLIGHT.compare_exchange_weak(
                n,
                n + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Self),
                Err(now) => n = now,
            }
        }
        None
    }
}

impl Drop for SendSlot {
    fn drop(&mut self) {
        SENDS_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Sets the user's telemetry consent preference
pub fn set_telemetry_consent(consent: bool) {
    TELEMETRY_CONSENT.store(consent, Ordering::Relaxed);
}

/// Gets the user's telemetry consent preference
fn get_telemetry_consent() -> bool {
    TELEMETRY_CONSENT.load(Ordering::Relaxed)
}

/// Determines the current platform as a string
fn get_platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "macos",
        _ => "unknown",
    }
}

/// Gets the application version from Cargo.toml
fn get_app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Crash report payload structure
#[derive(Serialize)]
struct CrashReport<'a> {
    r#type: &'a str,
    error_message: &'a str,
    platform: &'a str,
    app_version: &'a str,
}

/// Generation click payload structure
#[derive(Serialize)]
struct GenerationClick<'a> {
    r#type: &'a str,
}

/// Log entry payload structure
#[derive(Serialize)]
struct LogEntry<'a> {
    r#type: &'a str,
    log_level: &'a str,
    log_message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    app_version: Option<&'a str>,
}

/// Sends a crash report to the telemetry server
fn send_crash_report(error_message: String, platform: &str, app_version: &str) {
    // Wrap in catch_unwind to prevent any panics during crash reporting
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
            // Deliberately not the shared client: a dead HTTP runtime is itself a
            // crash cause, and this runs once per crash. The timeout is required,
            // not optional: the panic hook blocks here, so a stalled connection
            // would hang crash handling indefinitely.
            let client = build_client(CRASH_REPORT_TIMEOUT)?;

            let payload = CrashReport {
                r#type: "crash",
                error_message: &error_message,
                platform,
                app_version,
            };

            let _res = client
                .post(TELEMETRY_URL)
                .header("Content-Type", "application/json")
                .json(&payload)
                .send()?;

            Ok(())
        })();
    }));
}

/// Sends a generation click event to the telemetry server
pub fn send_generation_click() {
    // Check user consent
    if !get_telemetry_consent() {
        return;
    }

    // Only send in release builds
    if cfg!(debug_assertions) {
        return;
    }

    // Not slot-limited: this fires once per click and the count has to stay exact.
    // Send in background thread to avoid blocking UI. `Builder` over `spawn` because
    // spawn panics when the OS is out of thread resources, which is a state telemetry
    // has to survive rather than escalate.
    // Wrap in catch_unwind to prevent any panics from escaping
    let _ = std::thread::Builder::new().spawn(|| {
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
                let client = shared_client().ok_or("telemetry client unavailable")?;

                let payload = GenerationClick {
                    r#type: "generation_click",
                };

                let _res = client
                    .post(TELEMETRY_URL)
                    .header("Content-Type", "application/json")
                    .json(&payload)
                    .send()?;

                Ok(())
            })();
        }));
    });
}

/// Log levels for telemetry
#[allow(dead_code)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

impl LogLevel {
    fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warning => "warning",
            LogLevel::Error => "error",
        }
    }
}

/// Strip the query string from any URLs embedded in `input`.
///
/// Keeps the scheme/host/path so we can still tell which provider failed,
/// but drops `?key=value&...` where Arnis's Overpass and elevation
/// provider URLs carry bbox coordinates. Non-URL `?` (e.g. in English
/// text) is preserved because we only strip after a `://` scheme.
pub fn redact_url_queries(mut input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    while let Some(scheme_idx) = input.find("://") {
        let (before, after) = input.split_at(scheme_idx);
        out.push_str(before);
        out.push_str("://");
        let after_scheme = &after[3..];
        // Terminators: whitespace and `)` only. `,` is valid inside URL
        // queries (raw bbox coords in tile-service URLs) so stripping on
        // it would leak the tail of the query past the comma.
        let url_end = after_scheme
            .find(|c: char| c.is_whitespace() || c == ')')
            .unwrap_or(after_scheme.len());
        let url = &after_scheme[..url_end];
        let tail = &after_scheme[url_end..];
        // XYZ tile paths encode the user's location, so truncate known
        // tile services to their host; that still identifies the provider.
        const TILE_URL_PREFIXES: [&str; 2] = [
            "tiles.mapterhorn.com",
            "s3.amazonaws.com/elevation-tiles-prod",
        ];
        if let Some(prefix) = TILE_URL_PREFIXES.iter().find(|p| url.starts_with(*p)) {
            out.push_str(prefix);
        } else {
            match url.find('?') {
                Some(q) => out.push_str(&url[..q]),
                None => out.push_str(url),
            }
        }
        input = tail;
    }
    out.push_str(input);
    out
}

/// Replace file system paths in `input` with `<path>`. They carry the user name, and a Bedrock
/// world's folder is named after its area.
pub fn redact_paths(input: &str) -> String {
    let b = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let (mut copied, mut i) = (0, 0);
    while i < b.len() {
        if path_starts_at(b, i) {
            out.push_str(&input[copied..i]);
            out.push_str("<path>");
            i = path_end(b, i);
            copied = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&input[copied..]);
    out
}

fn path_starts_at(b: &[u8], i: usize) -> bool {
    let prev = i.checked_sub(1).map(|p| b[p]);
    let next = |k: usize| b.get(i + k).copied();
    let is_slash = |c: Option<u8>| matches!(c, Some(b'/' | b'\\'));
    let after_word = prev.is_some_and(|c| c.is_ascii_alphanumeric());
    let opens = prev.is_none_or(|c| c.is_ascii_whitespace() || b"('\"=`:[,{<;|".contains(&c));
    let file_url = !after_word && b[i..].starts_with(b"file:/");
    let drive =
        b[i].is_ascii_alphabetic() && !after_word && next(1) == Some(b':') && is_slash(next(2));
    let unix = b[i] == b'/' && next(1).is_some_and(|c| c != b'/' && !c.is_ascii_whitespace());
    let unc = b[i] == b'\\' && next(1) == Some(b'\\');
    let home = b[i] == b'~' && is_slash(next(1));
    file_url || drive || (opens && (unix || unc || home))
}

/// A quoted path runs to the last quote of its kind on the line that closes a phrase, so an
/// apostrophe in a name (O'Connor, Ca' Savio) stays inside it; this errs toward hiding more.
/// An unquoted one runs to a colon that ends a phrase, or the line end. Spaces stay inside both.
fn path_end(b: &[u8], start: usize) -> usize {
    let line_end = b[start..]
        .iter()
        .position(|&c| c == b'\n' || c == b'\r')
        .map_or(b.len(), |p| start + p);
    let breaks = |at: usize| {
        b.get(at)
            .is_none_or(|c| c.is_ascii_whitespace() || b":,;)]}".contains(c))
    };
    let closes = |at: usize| {
        breaks(at + 1) || (matches!(b.get(at + 1), Some(b'.' | b'!' | b'?')) && breaks(at + 2))
    };
    let quote = start
        .checked_sub(1)
        .map(|p| b[p])
        .filter(|c| b"'\"`".contains(c));
    let found = match quote {
        Some(q) => (start + 1..line_end)
            .rev()
            .find(|&i| b[i] == q && closes(i)),
        None => (start + 1..line_end)
            .find(|&i| b[i] == b':' && b.get(i + 1).is_none_or(|c| c.is_ascii_whitespace())),
    };
    found.unwrap_or(line_end)
}

/// Sends a log entry to the telemetry server
pub fn send_log(level: LogLevel, message: &str) {
    // Check user consent
    if !get_telemetry_consent() {
        return;
    }

    // Only send in release builds
    if cfg!(debug_assertions) {
        return;
    }

    // Redact URL query strings before anything else so bbox coordinates
    // embedded in reqwest/provider error messages never leave the client.
    let redacted = redact_paths(&redact_url_queries(message));
    let truncated_message = if redacted.chars().count() > 1000 {
        redacted.chars().take(1000).collect::<String>()
    } else {
        redacted
    };

    let platform = get_platform();
    let app_version = get_app_version();

    let Some(slot) = SendSlot::acquire() else {
        return;
    };

    // Send in background thread to avoid blocking. `Builder` over `spawn` because
    // spawn panics when the OS is out of thread resources, which is a state telemetry
    // has to survive rather than escalate.
    // Wrap in catch_unwind to prevent any panics from escaping
    let _ = std::thread::Builder::new().spawn(move || {
        let _slot = slot;
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
                let client = shared_client().ok_or("telemetry client unavailable")?;

                let payload = LogEntry {
                    r#type: "log",
                    log_level: level.as_str(),
                    log_message: &truncated_message,
                    platform: Some(platform),
                    app_version: Some(app_version),
                };

                let _res = client
                    .post(TELEMETRY_URL)
                    .header("Content-Type", "application/json")
                    .json(&payload)
                    .send()?;

                Ok(())
            })();
        }));
    });
}

/// Installs a panic hook that logs panics and sends crash reports
pub fn install_panic_hook() {
    panic::set_hook(Box::new(|panic_info| {
        // Log the panic to both stderr and log file
        error!("Application panicked: {:?}", panic_info);

        // Filter out secondary "panic in a function that cannot unwind" panics
        if let Some(location) = panic_info.location() {
            if location.file().contains("panicking.rs") {
                return;
            }
        }

        // Check user consent
        if !get_telemetry_consent() {
            return;
        }

        // Only send crash reports in release builds
        if cfg!(debug_assertions) {
            return;
        }

        // Everything else wrapped in catch_unwind to prevent secondary panics
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            // Extract panic payload
            let payload = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown panic".to_string()
            };

            // Extract location
            let location = panic_info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "unknown location".to_string());

            // Combine payload and location; redact URL queries so a panic
            // message carrying a request URL can't leak bbox coordinates.
            // The location is a source path in the build, not on the user's machine.
            let mut error_message =
                redact_url_queries(&format!("{} @ {}", redact_paths(&payload), location));

            // Truncate to 500 Unicode characters
            if error_message.chars().count() > 500 {
                error_message = error_message.chars().take(500).collect();
            }

            let platform = get_platform();
            let app_version = get_app_version();

            // Send crash report (best-effort, ignore all errors)
            send_crash_report(error_message, platform, app_version);
        }));
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_strips_overpass_query() {
        let input = "error sending request for url (https://overpass-api.de/api/interpreter?data=%5Bbbox%3A50.1%2C13.7%5D): connection timed out";
        let out = redact_url_queries(input);
        assert!(!out.contains("bbox"), "bbox leaked: {out}");
        assert!(!out.contains("?"), "query not stripped: {out}");
        assert!(out.contains("overpass-api.de"), "host lost: {out}");
        assert!(out.ends_with("): connection timed out"));
    }

    #[test]
    fn redact_preserves_non_url_question_marks() {
        let input = "What? That is odd.";
        assert_eq!(redact_url_queries(input), input);
    }

    #[test]
    fn redact_handles_multiple_urls_and_no_query() {
        let input = "first https://a.com/p?x=1 middle https://b.com/q end";
        assert_eq!(
            redact_url_queries(input),
            "first https://a.com/p middle https://b.com/q end"
        );
    }

    #[test]
    fn redact_handles_truncated_tail() {
        let input = "error for url https://host.tld/path?bbox=50.1,13.7";
        assert_eq!(
            redact_url_queries(input),
            "error for url https://host.tld/path"
        );
    }

    #[test]
    fn redact_paths_hides_user_and_world_names() {
        let bedrock = "World generation failed: Failed to save Bedrock world: I/O error while writing Bedrock world: '/home/moosa/Desktop/Arnis Malé' already exists and was not created by Arnis.";
        assert_eq!(
            redact_paths(bedrock),
            "World generation failed: Failed to save Bedrock world: I/O error while writing Bedrock world: '<path>' already exists and was not created by Arnis."
        );
        for (input, want) in [
            (
                r"Failed to create C:\Users\John Doe\Desktop\Arnis World 1: Access is denied. (os error 5)",
                "Failed to create <path>: Access is denied. (os error 5)",
            ),
            (
                r#"Failed to read "C:\\Users\\jo\\AppData\\Local\\arnis\\x.json": denied"#,
                r#"Failed to read "<path>": denied"#,
            ),
            (
                "'/home/O'Connor/Desktop/Arnis Berlin.mcworld.staging' already exists",
                "'<path>' already exists",
            ),
            (
                r"Failed to write 'C:\Users\jo\Desktop\Arnis L'Aquila.mcworld': disk full",
                "Failed to write '<path>': disk full",
            ),
            (
                r#"Failed to read "C:\\Users\\O'Connor\\x.json": denied"#,
                r#"Failed to read "<path>": denied"#,
            ),
            ("'/home/jo/Arnis Roma'. Rename it", "'<path>'. Rename it"),
            (
                r"'C:\Users\jo\Desktop\Arnis Ca' Savio.mcworld.staging' already exists",
                "'<path>' already exists",
            ),
            (
                "failed:/home/moosa/Desktop/Arnis Malé.mcworld.staging",
                "failed:<path>",
            ),
            ("[/home/jo/Arnis Roma]", "[<path>"),
            ("a,/home/jo/x", "a,<path>"),
            ("opened file:///home/jo/Arnis Malé", "opened <path>"),
            ("ends in /home/jo/x:", "ends in <path>:"),
            ("saved to C:/Users/jo/saves/Berlin", "saved to <path>"),
            (r"share \\nas\worlds\Paris (2) missing", "share <path>"),
            ("in ~/worlds/Tokyo", "in <path>"),
        ] {
            assert_eq!(redact_paths(input), want, "for {input:?}");
        }
    }

    #[test]
    fn redact_paths_leaves_urls_and_fractions() {
        for input in [
            "error sending request for url (https://api.arnismc.com/overpass/api/interpreter)",
            "[1/7] tile 13/4359/2842 and/or 3 / 4",
            "Overpass fetch failed on all 1 providers (api.arnismc.com); last error: Response code: 413",
            "io error: Os { code: 112, kind: StorageFull }",
            "range request to http://127.0.0.1:9/v1/archives.json failed [timeout]",
        ] {
            assert_eq!(redact_paths(input), input);
        }
    }

    #[test]
    fn redact_truncates_tile_url_paths() {
        let input = "HTTP 500 from https://tiles.mapterhorn.com/16/34178/23309.webp after retries";
        let out = redact_url_queries(input);
        assert!(!out.contains("34178"), "tile x leaked: {out}");
        assert!(out.contains("tiles.mapterhorn.com"), "host lost: {out}");

        let aws =
            "failed https://s3.amazonaws.com/elevation-tiles-prod/terrarium/15/17436/11365.png now";
        let out = redact_url_queries(aws);
        assert!(!out.contains("17436"), "tile x leaked: {out}");
        assert!(out.ends_with(" now"));
    }
}
