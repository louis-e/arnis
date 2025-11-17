use serde::Serialize;
use std::{
    fs::File,
    io,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use sysinfo::{ProcessExt, ProcessesToUpdate, System, SystemExt};

/// Snapshot of system / process metrics captured at a given moment.
#[derive(Debug, Serialize)]
pub struct MetricsSnapshot {
    /// Milliseconds since the UNIX epoch when the snapshot was collected.
    pub captured_at_ms: u128,
    /// Total physical RAM available on the system (bytes).
    pub total_ram_bytes: u64,
    /// Available RAM on the system (bytes).
    pub available_ram_bytes: u64,
    /// Used RAM on the system (bytes).
    pub used_ram_bytes: u64,
    /// Total swap space (bytes).
    pub total_swap_bytes: u64,
    /// Used swap space (bytes).
    pub used_swap_bytes: u64,
    /// Resident set size for the current process (bytes).
    pub process_rss_bytes: Option<u64>,
    /// Virtual memory size for the current process (bytes).
    pub process_virtual_bytes: Option<u64>,
}

/// Lightweight recorder that refreshes sysinfo data on demand.
pub struct MetricsRecorder {
    system: System,
    pid: sysinfo::Pid,
}

impl MetricsRecorder {
    /// Instantiate the recorder and seed the initial data.
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_memory();
        system.refresh_processes(ProcessesToUpdate::All, false);
        let pid = sysinfo::get_current_pid().expect("failed to determine current PID");
        Self { system, pid }
    }

    /// Capture a fresh snapshot of the system and process usage.
    pub fn capture(&mut self) -> MetricsSnapshot {
        self.system.refresh_memory();
        self.system.refresh_processes(ProcessesToUpdate::All, false);
        let process_entry = self.system.process(self.pid);
        let snapshot = MetricsSnapshot {
            captured_at_ms: now_ms(),
            total_ram_bytes: to_bytes(self.system.total_memory()),
            available_ram_bytes: to_bytes(self.system.available_memory()),
            used_ram_bytes: to_bytes(self.system.used_memory()),
            total_swap_bytes: to_bytes(self.system.total_swap()),
            used_swap_bytes: to_bytes(self.system.used_swap()),
            process_rss_bytes: process_entry.map(|process| to_bytes(process.memory())),
            process_virtual_bytes: process_entry.map(|process| to_bytes(process.virtual_memory())),
        };
        snapshot
    }

    /// Serialize the latest snapshot to the given path as pretty JSON.
    pub fn write_to_path<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        let snapshot = self.capture();
        let file = File::create(path)?;
        serde_json::to_writer_pretty(file, &snapshot)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err))
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time moved backwards")
        .as_millis()
}

fn to_bytes(kilobytes: u64) -> u64 {
    kilobytes * 1024
}
