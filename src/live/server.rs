//! The local vanilla server process: its folder, settings and lifetime.

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const EULA_URL: &str = "https://aka.ms/MinecraftEULA";
const LOG_LINES: usize = 200;

/// Where the live server keeps its own files (not the world).
pub fn server_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ARNIS_LIVE_SERVER_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("arnis")
        .join("live-server")
}

pub fn eula_accepted(dir: &Path) -> bool {
    fs::read_to_string(dir.join("eula.txt")).is_ok_and(|t| {
        t.lines()
            .any(|l| l.trim().eq_ignore_ascii_case("eula=true"))
    })
}

pub fn accept_eula(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    fs::write(
        dir.join("eula.txt"),
        format!("# Agreed in Arnis to the Minecraft EULA ({EULA_URL})\neula=true\n"),
    )
    .map_err(|e| e.to_string())
}

/// What the world's level.dat says the server should match.
#[derive(Clone, Debug)]
pub struct WorldRules {
    pub gamemode: &'static str,
    pub difficulty: &'static str,
    pub hardcore: bool,
}

pub struct Launch<'a> {
    pub java: &'a Path,
    pub jar: &'a Path,
    pub saves_dir: &'a Path,
    pub world_name: &'a str,
    pub port: u16,
    pub rcon_port: u16,
    pub rcon_password: &'a str,
    pub view_distance: u32,
    pub rules: &'a WorldRules,
}

pub struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    log: Arc<Mutex<VecDeque<String>>>,
    ready: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
}

pub fn free_port(preferred: Option<u16>) -> u16 {
    if let Some(p) = preferred.filter(|p| port_free(*p)) {
        return p;
    }
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(25565)
}

/// Free on loopback and nothing answering there. Only loopback is probed: listening on
/// every interface, even for a moment, asks for a firewall exception on Windows.
fn port_free(port: u16) -> bool {
    let addr = std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpListener::bind(addr).is_ok()
        && std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err()
}

fn write_properties(dir: &Path, l: &Launch) -> Result<(), String> {
    let r = l.rules;
    let props = [
        ("server-ip", "127.0.0.1".to_string()),
        ("server-port", l.port.to_string()),
        ("level-name", l.world_name.to_string()),
        ("motd", format!("Arnis Live \u{00b7} {}", l.world_name)),
        ("online-mode", "true".to_string()),
        // 26.x servers default to a whitelist; this one only listens on localhost.
        ("white-list", "false".to_string()),
        ("enforce-whitelist", "false".to_string()),
        ("enable-rcon", "true".to_string()),
        ("rcon.port", l.rcon_port.to_string()),
        ("rcon.password", l.rcon_password.to_string()),
        // Ops would otherwise see every RCON command in chat.
        ("broadcast-rcon-to-ops", "false".to_string()),
        ("broadcast-console-to-ops", "false".to_string()),
        ("view-distance", l.view_distance.to_string()),
        ("simulation-distance", l.view_distance.min(10).to_string()),
        ("gamemode", r.gamemode.to_string()),
        ("difficulty", r.difficulty.to_string()),
        ("hardcore", r.hardcore.to_string()),
        ("force-gamemode", "false".to_string()),
        ("allow-flight", "true".to_string()),
        ("spawn-protection", "0".to_string()),
        ("enable-command-block", "true".to_string()),
        ("max-players", "8".to_string()),
        ("pause-when-empty-seconds", "-1".to_string()),
        ("log-ips", "false".to_string()),
        ("sync-chunk-writes", "true".to_string()),
    ];
    let mut text = String::from("# Written by Arnis for live mode on every start.\n");
    for (k, v) in props {
        text.push_str(&format!("{k}={v}\n"));
    }
    let path = dir.join("server.properties");
    crate::world_utils::replace_file_atomically(&path, text.as_bytes())?;
    keep_private(&path);
    Ok(())
}

/// Readable by this user only, for files holding the RCON password.
pub(crate) fn keep_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn heap_mb() -> u64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let total_mb = sys.total_memory() / (1024 * 1024);
    (total_mb / 4).clamp(1024, 4096)
}

impl Server {
    pub fn start(dir: &Path, l: &Launch) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        if !eula_accepted(dir) {
            return Err("The Minecraft EULA has not been accepted.".to_string());
        }
        write_properties(dir, l)?;
        // Every world shares this folder; ops come from the world's cheats setting each session.
        fs::write(dir.join("ops.json"), "[]").map_err(|e| e.to_string())?;
        let mut cmd = Command::new(l.java);
        cmd.current_dir(dir)
            .arg(format!("-Xmx{}M", heap_mb()))
            .arg("-Xms512M")
            .arg("-jar")
            .arg(l.jar)
            .arg("--nogui")
            .arg("--universe")
            .arg(l.saves_dir)
            .arg("--world")
            .arg(l.world_name)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        detach(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Could not start Java for the server: {e}"))?;
        let log = Arc::new(Mutex::new(VecDeque::new()));
        let ready = Arc::new(AtomicBool::new(false));
        let failure = Arc::new(Mutex::new(None));
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        for stream in [
            stdout.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            stderr.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let (log, ready, failure) = (log.clone(), ready.clone(), failure.clone());
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    watch_line(&line, &ready, &failure);
                    let mut log = log.lock().unwrap_or_else(|e| e.into_inner());
                    if log.len() >= LOG_LINES {
                        log.pop_front();
                    }
                    log.push_back(line);
                }
            });
        }
        let stdin = child.stdin.take();
        Ok(Self {
            child,
            stdin,
            log,
            ready,
            failure,
        })
    }

    /// Waits for "Done", the process to end, or a known failure.
    pub fn wait_ready(
        &mut self,
        timeout: Duration,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let start = Instant::now();
        loop {
            if self.ready.load(Ordering::Acquire) {
                return Ok(());
            }
            if let Some(f) = self
                .failure
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
            {
                return Err(f);
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(format!(
                    "The server stopped while starting ({status}). Last lines:\n{}",
                    self.tail(12)
                ));
            }
            if cancel() {
                return Err("Cancelled".to_string());
            }
            if start.elapsed() > timeout {
                return Err("The server took too long to start.".to_string());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn tail(&self, n: usize) -> String {
        let log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        let skip = log.len().saturating_sub(n);
        log.iter()
            .skip(skip)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `stop` through RCON or the console, then a kill if it hangs.
    pub fn stop(&mut self, rcon: Option<&mut super::rcon::Rcon>, timeout: Duration) {
        if !self.running() {
            return;
        }
        let sent = rcon.is_some_and(|r| r.cmd("stop").is_ok());
        if !sent {
            if let Some(stdin) = self.stdin.as_mut() {
                let _ = stdin.write_all(b"stop\n");
                let _ = stdin.flush();
            }
        }
        let start = Instant::now();
        while start.elapsed() < timeout {
            if !self.running() {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop(None, Duration::from_secs(60));
    }
}

fn watch_line(line: &str, ready: &AtomicBool, failure: &Mutex<Option<String>>) {
    if line.contains("Done (") && line.contains("For help") {
        ready.store(true, Ordering::Release);
        return;
    }
    let known = [
        (
            "FAILED TO BIND TO PORT",
            "The server port is taken by another program.",
        ),
        (
            "You need to agree to the EULA",
            "The Minecraft EULA has not been accepted.",
        ),
        (
            "Session lock is already acquired",
            "The world is open in Minecraft. Leave it first.",
        ),
        (
            "session.lock: already locked",
            "The world is open in Minecraft. Leave it first.",
        ),
        (
            "UnsupportedClassVersionError",
            "This Java is too old for the server.",
        ),
        (
            "Failed to start the minecraft server",
            "The server failed to start.",
        ),
    ];
    for (needle, message) in known {
        if line.contains(needle) {
            let mut f = failure.lock().unwrap_or_else(|e| e.into_inner());
            if f.is_none() {
                *f = Some(message.to_string());
            }
        }
    }
}

/// No console window, and out of the terminal's process group: Ctrl+C reaches only Arnis,
/// which then stops the child in order.
pub fn detach(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eula_is_read_and_written() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!eula_accepted(dir.path()));
        accept_eula(dir.path()).unwrap();
        assert!(eula_accepted(dir.path()));
    }

    #[test]
    fn ready_and_failures_are_seen_in_the_log() {
        let (ready, failure) = (AtomicBool::new(false), Mutex::new(None));
        watch_line(
            "[01:58:00] [Server thread/INFO]: Done (1.330s)! For help, type \"help\"",
            &ready,
            &failure,
        );
        assert!(ready.load(Ordering::Acquire));
        watch_line("**** FAILED TO BIND TO PORT!", &ready, &failure);
        assert!(failure.lock().unwrap().as_deref().unwrap().contains("port"));
    }
}
