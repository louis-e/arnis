use log::error;
use reqwest::blocking::Client;
use serde::Serialize;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Crash report payload structure
#[derive(Serialize)]
struct CrashReport<'a> {
    error_message: &'a str,
    platform: &'a str,
    app_version: &'a str,
}

/// Sends a crash report to the telemetry server
fn send_crash_report(error_message: String, platform: &str, app_version: &str) {
    let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
        let client = Client::new();
        let url = "https://arnismc.com/report_telemetry.php";

        let payload = CrashReport {
            error_message: &error_message,
            platform,
            app_version,
        };

        let _res = client
            .post(url)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()?;

        Ok(())
    })();
}

/// Installs a panic hook that logs panics and sends crash reports
pub fn install_panic_hook() {
    panic::set_hook(Box::new(|panic_info| {
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

            // Determine platform
            let platform = match std::env::consts::OS {
                "windows" => "windows",
                "linux" => "linux",
                "macos" => "macos",
                _ => "unknown",
            };

            // Get app version
            let app_version = env!("CARGO_PKG_VERSION");

            // Send crash report (best-effort, ignore all errors)
            send_crash_report(error_message, platform, app_version);
        }));
    }));
}
