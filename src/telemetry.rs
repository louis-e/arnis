use log::error;
use reqwest::blocking::Client;
use serde::Serialize;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};

/// Telemetry endpoint URL
const TELEMETRY_URL: &str = "https://arnismc.com/telemetry/report_telemetry.php";

/// Global flag to store user's telemetry consent
static TELEMETRY_CONSENT: AtomicBool = AtomicBool::new(false);

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
            let client = Client::new();

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

    // Send in background thread to avoid blocking UI
    // Wrap in catch_unwind to prevent any panics from escaping
    let _ = std::thread::spawn(|| {
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
                let client = Client::new();

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

    // Truncate message to 1000 characters
    let truncated_message = if message.chars().count() > 1000 {
        message.chars().take(1000).collect::<String>()
    } else {
        message.to_string()
    };

    let platform = get_platform();
    let app_version = get_app_version();

    // Send in background thread to avoid blocking
    // Wrap in catch_unwind to prevent any panics from escaping
    let _ = std::thread::spawn(move || {
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
                let client = Client::new();

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

            // Combine payload and location
            let mut error_message = format!("{} @ {}", payload, location);

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
