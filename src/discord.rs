//! The Discord status while Arnis is open ("Playing Arnis"), sent to the Discord app on the
//! same machine over its local socket. Nothing leaves the machine from here, and only fixed
//! phrases are shown: never a place, a path or a world name.

use serde_json::{json, Value};
use std::io::{self, Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The Discord application that names the status "Arnis" and holds its logo. Empty leaves
/// the status off.
const APPLICATION_ID: &str = "";
/// Key of the logo among the application's Rich Presence art assets.
const LOGO_ASSET: &str = "arnis";
const WEBSITE: &str = "https://arnismc.com";

/// Discord takes five updates per 20 seconds.
const MIN_GAP: Duration = Duration::from_secs(4);
/// A moving percentage only goes out this often.
const PROGRESS_GAP: Duration = Duration::from_secs(15);
const RECONNECT_GAP: Duration = Duration::from_secs(20);
/// How long "generated a world" stays up before the status goes back to the map.
const FINISHED_FOR: Duration = Duration::from_secs(10 * 60);
const TICK: Duration = Duration::from_secs(5);
/// An unchanged status is sent again this often, which also notices a restarted Discord.
const REFRESH: Duration = Duration::from_secs(5 * 60);

enum Event {
    Enabled(bool),
    GenerationStarted,
    Progress(f64, &'static str),
    GenerationDone,
    GenerationEnded,
    Live(bool),
}

/// Whether there is a Discord application to show the status under.
pub fn available() -> bool {
    !APPLICATION_ID.is_empty()
}

fn send(event: Event) {
    static SENDER: OnceLock<Option<Mutex<SyncSender<Event>>>> = OnceLock::new();
    let sender = SENDER.get_or_init(|| {
        if !available() {
            return None;
        }
        // Bounded: a worker stuck on a Discord that stopped answering drops updates instead
        // of piling them up.
        let (tx, rx) = mpsc::sync_channel(256);
        std::thread::Builder::new()
            .name("discord-status".into())
            .spawn(move || Worker::new().run(rx))
            .ok()?;
        Some(Mutex::new(tx))
    });
    if let Some(tx) = sender {
        let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).try_send(event);
    }
}

/// The Settings toggle. Nothing is shown until the GUI has said it is on.
pub fn set_enabled(enabled: bool) {
    send(Event::Enabled(enabled));
}

/// Shows the generation from now until the guard drops, which every way out of a run does.
pub struct Generation(());

impl Generation {
    pub fn begin() -> Self {
        send(Event::GenerationStarted);
        Self(())
    }
}

impl Drop for Generation {
    fn drop(&mut self) {
        send(Event::GenerationEnded);
    }
}

/// A progress update of the running generation, as the GUI gets it. An error ends the
/// status only with the run itself, through [`Generation`]: a refused second start shows
/// one too while the first goes on.
pub fn progress(progress: f64, message: &str) {
    if message.starts_with("Done!") {
        send(Event::GenerationDone);
    } else if !message.starts_with("Error!") {
        send(Event::Progress(progress, stage_of(message)));
    }
}

/// Live mode's server phase.
pub fn live(phase: &str) {
    send(Event::Live(matches!(phase, "starting" | "running")));
}

/// A fixed label for a progress message, or "" to keep the last one.
fn stage_of(message: &str) -> &'static str {
    const STAGES: [(&str, &str); 14] = [
        ("Downloading data", "Downloading map data"),
        ("Loading data from file", "Reading map data"),
        ("Waiting for", "Downloading map data"),
        ("Processing elevation", "Shaping the terrain"),
        ("Processing terrain", "Shaping the terrain"),
        ("Transforming map", "Preparing the map"),
        ("Processing data", "Preparing the map"),
        ("Generating area", "Building the area"),
        ("Generating ground", "Laying the ground"),
        ("Carving caves", "Carving caves"),
        ("Sprinkling ore", "Carving caves"),
        ("Facades", "Adding facades"),
        ("Saving", "Saving the world"),
        ("Finalizing", "Finishing up"),
    ];
    STAGES
        .iter()
        .find(|(prefix, _)| message.starts_with(prefix))
        .map_or("", |&(_, label)| label)
}

/// Milliseconds, which Discord's timestamps are in.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[derive(Clone, PartialEq, Debug)]
enum Phase {
    Map,
    Generating {
        since: u64,
        percent: u8,
        stage: &'static str,
    },
    Finished(Instant),
    Live(u64),
}

/// What the status says. Two equal ones need no update.
#[derive(Clone, PartialEq, Debug)]
struct Shown {
    details: &'static str,
    state: Option<String>,
    since: u64,
}

impl Shown {
    fn of(phase: &Phase, app_start: u64) -> Self {
        match phase {
            Phase::Map => Shown {
                details: "Picking a place on the map",
                state: None,
                since: app_start,
            },
            Phase::Generating {
                since,
                percent,
                stage,
            } => Shown {
                details: "Generating a Minecraft world",
                state: Some(if stage.is_empty() {
                    format!("{percent}%")
                } else {
                    format!("{stage} · {percent}%")
                }),
                since: *since,
            },
            Phase::Finished(_) => Shown {
                details: "Generated a Minecraft world",
                state: Some("Ready to play".to_string()),
                since: app_start,
            },
            Phase::Live(since) => Shown {
                details: "Playing a live world",
                state: Some("Land appears as players explore".to_string()),
                since: *since,
            },
        }
    }

    fn activity(&self) -> Value {
        let mut activity = json!({
            "details": self.details,
            "timestamps": { "start": self.since },
            "assets": { "large_image": LOGO_ASSET, "large_text": "Arnis" },
            "buttons": [{ "label": "Get Arnis", "url": WEBSITE }],
            "instance": false,
        });
        if let Some(state) = &self.state {
            activity["state"] = json!(state);
        }
        activity
    }
}

struct Worker {
    enabled: bool,
    app_start: u64,
    phase: Phase,
    conn: Option<Connection>,
    shown: Option<Shown>,
    last_send: Option<Instant>,
    last_connect: Option<Instant>,
}

impl Worker {
    fn new() -> Self {
        Self {
            enabled: false,
            app_start: unix_now(),
            phase: Phase::Map,
            conn: None,
            shown: None,
            last_send: None,
            last_connect: None,
        }
    }

    fn run(mut self, rx: Receiver<Event>) {
        loop {
            match rx.recv_timeout(TICK) {
                Ok(event) => self.apply(event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            // A burst of progress settles into one update.
            while let Ok(event) = rx.try_recv() {
                self.apply(event);
            }
            self.tick();
        }
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Enabled(on) => self.enabled = on,
            Event::GenerationStarted => {
                self.phase = Phase::Generating {
                    since: unix_now(),
                    percent: 0,
                    stage: "",
                }
            }
            Event::Progress(p, label) => {
                if let Phase::Generating { percent, stage, .. } = &mut self.phase {
                    if p >= 0.0 {
                        *percent = p.clamp(0.0, 100.0) as u8;
                    }
                    if !label.is_empty() {
                        *stage = label;
                    }
                }
            }
            Event::GenerationDone => {
                if matches!(self.phase, Phase::Generating { .. }) {
                    self.phase = Phase::Finished(Instant::now());
                }
            }
            Event::GenerationEnded => {
                if matches!(self.phase, Phase::Generating { .. }) {
                    self.phase = Phase::Map;
                }
            }
            Event::Live(on) => match (&self.phase, on) {
                (Phase::Live(_), true) | (Phase::Generating { .. }, _) => {}
                (_, true) => self.phase = Phase::Live(unix_now()),
                (Phase::Live(_), false) => self.phase = Phase::Map,
                (_, false) => {}
            },
        }
    }

    fn tick(&mut self) {
        if !self.enabled {
            if let Some(mut conn) = self.conn.take() {
                let _ = conn.set_activity(Value::Null);
            }
            self.shown = None;
            return;
        }
        if matches!(self.phase, Phase::Finished(at) if at.elapsed() > FINISHED_FOR) {
            self.phase = Phase::Map;
        }
        let want = Shown::of(&self.phase, self.app_start);
        let stale = self.last_send.is_none_or(|t| t.elapsed() > REFRESH);
        if self.shown.as_ref() == Some(&want) && !stale {
            return;
        }
        let only_percent = self
            .shown
            .as_ref()
            .is_some_and(|s| s.details == want.details);
        let gap = if only_percent { PROGRESS_GAP } else { MIN_GAP };
        if self.last_send.is_some_and(|t| t.elapsed() < gap) {
            return;
        }
        if self.conn.is_none() {
            if self
                .last_connect
                .is_some_and(|t| t.elapsed() < RECONNECT_GAP)
            {
                return;
            }
            self.last_connect = Some(Instant::now());
            match Connection::open() {
                Ok(conn) => {
                    log::info!("Discord status connected");
                    self.conn = Some(conn);
                }
                Err(_) => return,
            }
        }
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        self.last_send = Some(Instant::now());
        match conn.set_activity(want.activity()) {
            Ok(()) => self.shown = Some(want),
            Err(e) => {
                log::info!("Discord status lost: {e}");
                self.conn = None;
                self.shown = None;
            }
        }
    }
}

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;
/// Replies are a few kilobytes; anything this large is not Discord.
const MAX_FRAME: usize = 1 << 20;

#[cfg(unix)]
type Pipe = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Pipe = std::fs::File;

struct Connection {
    pipe: Pipe,
    nonce: u64,
}

impl Connection {
    fn open() -> io::Result<Self> {
        let mut last = io::Error::new(io::ErrorKind::NotFound, "Discord is not running");
        for path in socket_paths() {
            match open_pipe(&path) {
                Ok(pipe) => {
                    let mut conn = Self { pipe, nonce: 0 };
                    match conn.handshake() {
                        Ok(()) => return Ok(conn),
                        Err(e) => last = e,
                    }
                }
                Err(e) if e.kind() != io::ErrorKind::NotFound => last = e,
                Err(_) => {}
            }
        }
        Err(last)
    }

    fn handshake(&mut self) -> io::Result<()> {
        write_frame(
            &mut self.pipe,
            OP_HANDSHAKE,
            &json!({ "v": 1, "client_id": APPLICATION_ID }),
        )?;
        let (op, body) = self.read_reply()?;
        if op == OP_FRAME && body["evt"] == "READY" {
            Ok(())
        } else {
            Err(io::Error::other(format!("handshake refused: {body}")))
        }
    }

    fn set_activity(&mut self, activity: Value) -> io::Result<()> {
        self.nonce += 1;
        let nonce = format!("arnis-{}-{}", std::process::id(), self.nonce);
        write_frame(
            &mut self.pipe,
            OP_FRAME,
            &json!({
                "cmd": "SET_ACTIVITY",
                "args": { "pid": std::process::id(), "activity": activity },
                "nonce": nonce,
            }),
        )?;
        // Read up to its reply, so the socket never fills with unread answers.
        for _ in 0..8 {
            let (_, body) = self.read_reply()?;
            if body["nonce"] == nonce.as_str() {
                if body["evt"] == "ERROR" {
                    // Kept as shown: sending the same thing again would fail the same way.
                    log::warn!("Discord refused the status: {}", body["data"]);
                }
                return Ok(());
            }
        }
        Ok(())
    }

    /// The next frame that is not a ping, answering pings on the way.
    fn read_reply(&mut self) -> io::Result<(u32, Value)> {
        loop {
            let (op, body) = read_frame(&mut self.pipe)?;
            match op {
                OP_PING => write_frame(&mut self.pipe, OP_PONG, &body)?,
                OP_CLOSE => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        format!("Discord closed the connection: {body}"),
                    ))
                }
                _ => return Ok((op, body)),
            }
        }
    }
}

fn write_frame(w: &mut impl Write, op: u32, body: &Value) -> io::Result<()> {
    let body = body.to_string();
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend_from_slice(&op.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body.as_bytes());
    w.write_all(&frame)?;
    w.flush()
}

fn read_frame(r: &mut impl Read) -> io::Result<(u32, Value)> {
    let mut header = [0u8; 8];
    r.read_exact(&mut header)?;
    let op = u32::from_le_bytes(header[..4].try_into().unwrap());
    let len = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized frame",
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let body =
        serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok((op, body))
}

#[cfg(unix)]
fn socket_paths() -> Vec<std::path::PathBuf> {
    // The plain client, then Flatpak and Snap installs, which put theirs in a folder of their own.
    const SUBDIRS: [&str; 8] = [
        "",
        "app/com.discordapp.Discord",
        "app/com.discordapp.DiscordCanary",
        "app/dev.vencord.Vesktop",
        ".flatpak/com.discordapp.Discord/xdg-run",
        ".flatpak/dev.vencord.Vesktop/xdg-run",
        "snap.discord",
        "snap.discord-canary",
    ];
    let mut bases: Vec<std::path::PathBuf> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .filter_map(std::env::var_os)
        .map(std::path::PathBuf::from)
        .collect();
    bases.push("/tmp".into());
    bases.dedup();
    let mut out = Vec::new();
    for i in 0..10 {
        for base in &bases {
            for sub in SUBDIRS {
                out.push(base.join(sub).join(format!("discord-ipc-{i}")));
            }
        }
    }
    out
}

#[cfg(windows)]
fn socket_paths() -> Vec<std::path::PathBuf> {
    (0..10)
        .map(|i| format!(r"\\?\pipe\discord-ipc-{i}").into())
        .collect()
}

#[cfg(unix)]
fn open_pipe(path: &std::path::Path) -> io::Result<Pipe> {
    let pipe = std::os::unix::net::UnixStream::connect(path)?;
    pipe.set_read_timeout(Some(Duration::from_secs(5)))?;
    pipe.set_write_timeout(Some(Duration::from_secs(5)))?;
    Ok(pipe)
}

#[cfg(windows)]
fn open_pipe(path: &std::path::Path) -> io::Result<Pipe> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, OP_FRAME, &json!({ "cmd": "X", "n": 3 })).unwrap();
        assert_eq!(&buf[..4], &1u32.to_le_bytes());
        let (op, body) = read_frame(&mut buf.as_slice()).unwrap();
        assert_eq!(op, OP_FRAME);
        assert_eq!(body["n"], 3);

        let mut huge = 1u32.to_le_bytes().to_vec();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_frame(&mut huge.as_slice()).is_err());
        assert!(read_frame(&mut [1u8, 0, 0].as_slice()).is_err());
    }

    #[test]
    fn only_fixed_labels_reach_the_status() {
        assert_eq!(stage_of("Generating ground..."), "Laying the ground");
        assert_eq!(stage_of("Saving Bedrock world..."), "Saving the world");
        assert_eq!(
            stage_of("Facades: downloading photos 3/40..."),
            "Adding facades"
        );
        assert_eq!(stage_of("Berlin, Mitte"), "");
        assert_eq!(stage_of(""), "");
    }

    #[test]
    fn a_generation_moves_through_its_phases() {
        let mut w = Worker::new();
        w.apply(Event::Progress(40.0, "Laying the ground"));
        assert_eq!(
            w.phase,
            Phase::Map,
            "progress outside a generation is a precompute"
        );

        w.apply(Event::GenerationStarted);
        w.apply(Event::Progress(12.6, "Shaping the terrain"));
        w.apply(Event::Progress(-1.0, ""));
        let shown = Shown::of(&w.phase, w.app_start);
        assert_eq!(shown.details, "Generating a Minecraft world");
        assert_eq!(shown.state.as_deref(), Some("Shaping the terrain · 12%"));

        w.apply(Event::Live(true));
        assert!(matches!(w.phase, Phase::Generating { .. }));
        w.apply(Event::GenerationDone);
        assert!(matches!(w.phase, Phase::Finished(_)));
        // The guard dropping after Done! keeps the finished status.
        w.apply(Event::GenerationEnded);
        assert!(matches!(w.phase, Phase::Finished(_)));

        w.apply(Event::GenerationStarted);
        w.apply(Event::GenerationEnded);
        assert_eq!(w.phase, Phase::Map);

        w.apply(Event::Live(true));
        assert!(matches!(w.phase, Phase::Live(_)));
        w.apply(Event::Live(false));
        assert_eq!(w.phase, Phase::Map);
    }

    #[test]
    fn the_activity_carries_logo_button_and_no_empty_state() {
        let a = Shown::of(&Phase::Map, 1_700_000_000_000).activity();
        assert_eq!(a["assets"]["large_image"], LOGO_ASSET);
        assert_eq!(a["buttons"][0]["url"], WEBSITE);
        assert_eq!(a["timestamps"]["start"], 1_700_000_000_000u64);
        assert!(a.get("state").is_none());
    }
}
