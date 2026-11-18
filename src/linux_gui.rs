//! Starting the GUI on Linux. WebKitGTK aborts the whole process when it cannot set up EGL
//! ("Could not create surfaceless EGL display: EGL_NOT_INITIALIZED. Aborting..."),
//! so the failure never reaches Rust as an error. The GUI therefore runs in a child
//! process, and a child that dies that way while the window comes up is started again
//! with the next renderer that has worked for users: zink (#799, #833) and the GPU's own
//! driver (#1247), then zink on the GPU as #1318 set it. The one that worked is remembered,
//! so later starts don't crash first.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

/// Set on the child, naming the renderer it runs with.
const RENDERER_ENV: &str = "ARNIS_LINUX_RENDERER";

/// Rendering variables a user may set to pick the driver themselves.
const USER_OVERRIDES: [&str; 3] = [
    "MESA_LOADER_DRIVER_OVERRIDE",
    "LIBGL_ALWAYS_SOFTWARE",
    "GALLIUM_DRIVER",
];

/// How long a renderer has to bring the window up: the launcher counts it from the spawn,
/// the GUI's web process watch from the window's setup. A crash of the GUI after it is not
/// the renderer's; the exit codes below are, whenever the launcher sees them.
const STARTUP: Duration = Duration::from_secs(15);

/// What the child exits with when the window failed on EGL with an error, not an abort.
pub const EGL_FAILED_EXIT: i32 = 86;

/// What the child exits with when its web process crashed while the window came up. EGL is
/// one cause but not the only one, so a renderer that has worked gets a second launch.
const WEB_PROCESS_CRASHED_EXIT: i32 = 87;

/// Where to look when the web process crashes, whatever the renderer.
const WEB_PROCESS_HELP: &str = "WebKit's messages above, and the log in \
    ~/.local/share/com.louisdev.arnis/logs, say why; on Ubuntu 24.04 and later, AppArmor \
    stopping WebKit's bubblewrap sandbox is one known cause. The command line works without \
    a window: arnis --help";

/// What a child dies of when WebKit or Mesa gives up natively. A Rust panic exits with 101
/// instead, and a stop the user asked for (SIGINT, SIGTERM) is none of these.
const CRASH_SIGNALS: [i32; 6] = [
    libc::SIGILL,
    libc::SIGTRAP,
    libc::SIGABRT,
    libc::SIGBUS,
    libc::SIGFPE,
    libc::SIGSEGV,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderer {
    /// Mesa's software rasterizer, set in `gui::run_gui`. The default.
    Software,
    /// Mesa's zink driver over Vulkan, on top of the software defaults.
    Zink,
    /// The GPU's own driver, with no software rendering forced.
    Gpu,
    /// Zink with software rendering switched off, so it runs on the GPU's Vulkan driver.
    ZinkGpu,
}

impl Renderer {
    const ORDER: [Renderer; 4] = [
        Renderer::Software,
        Renderer::Zink,
        Renderer::Gpu,
        Renderer::ZinkGpu,
    ];

    fn name(self) -> &'static str {
        match self {
            Renderer::Software => "software",
            Renderer::Zink => "zink",
            Renderer::Gpu => "gpu",
            Renderer::ZinkGpu => "zink-gpu",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|r| r.name() == name.trim())
    }

    fn configure(self, cmd: &mut Command) {
        cmd.env(RENDERER_ENV, self.name());
        match self {
            Renderer::Software | Renderer::Gpu => {}
            Renderer::Zink => {
                cmd.env("MESA_LOADER_DRIVER_OVERRIDE", "zink");
            }
            // Set here, so `gui::run_gui` leaves them as they are.
            Renderer::ZinkGpu => {
                cmd.env("MESA_LOADER_DRIVER_OVERRIDE", "zink")
                    .env("LIBGL_ALWAYS_SOFTWARE", "0")
                    .env("GALLIUM_DRIVER", "zink");
            }
        }
    }
}

/// The renderer this process was started with, if `supervise` started it.
pub fn current() -> Option<Renderer> {
    std::env::var(RENDERER_ENV)
        .ok()
        .and_then(|name| Renderer::from_name(&name))
}

/// Whether an error the GUI returned is EGL failing to start, which another renderer may fix.
pub fn is_egl_error(message: &str) -> bool {
    let lowered = message.to_ascii_lowercase();
    lowered.contains("egl_not_initialized")
        || lowered.contains("surfaceless egl")
        || (lowered.contains("libegl") && lowered.contains("failed"))
}

/// WebKit can also give up on EGL in the page's own process, which leaves a blank window
/// and this process running (#1481), so a crash there while starting ends this one too.
#[cfg(target_os = "linux")]
pub fn watch_web_process(window: &tauri::WebviewWindow) {
    if current().is_none() {
        return;
    }
    let begun = Instant::now();
    let watched = window.with_webview(move |webview| {
        use webkit2gtk::WebViewExt;
        webview
            .inner()
            .connect_web_process_terminated(move |_, reason| {
                if web_process_failed_start(reason, begun.elapsed()) {
                    log::error!("The window's web process crashed while it came up.");
                    log::logger().flush();
                    // Not `exit`: its atexit handlers could hang inside the running main loop.
                    unsafe { libc::_exit(WEB_PROCESS_CRASHED_EXIT) };
                }
            });
    });
    if let Err(e) = watched {
        eprintln!("Warning: could not watch the window's web process ({e}).");
    }
}

#[cfg(target_os = "linux")]
fn web_process_failed_start(
    reason: webkit2gtk::WebProcessTerminationReason,
    since_start: Duration,
) -> bool {
    reason == webkit2gtk::WebProcessTerminationReason::Crashed && since_start < STARTUP
}

/// Runs the GUI in a child process and returns the exit code to leave with, or `None`
/// when this process should open the window itself: it is the child already, or the
/// user picked a driver with one of `USER_OVERRIDES`.
pub fn supervise() -> Option<i32> {
    if current().is_some() || USER_OVERRIDES.iter().any(|v| std::env::var_os(v).is_some()) {
        return None;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("Warning: could not find the Arnis executable ({e}); starting directly.");
            return None;
        }
    };
    let memory = memory_file();
    let remembered = memory.as_deref().and_then(read_remembered);
    let order = attempt_order(remembered.map(|(renderer, _)| renderer));
    let mut only_web_process_crashes = true;
    for (i, &renderer) in order.iter().enumerate() {
        let mut cmd = Command::new(&exe);
        renderer.configure(&mut cmd);
        let started = || {
            if let Some(path) = memory.as_deref() {
                remember(path, renderer);
            }
        };
        match run_child(cmd, STARTUP, started) {
            Ok(Outcome::Exited(code)) => return Some(code),
            // A renderer that has worked may have crashed for a reason of WebKit's own.
            Ok(Outcome::WebProcessCrashed) if remembered == Some((renderer, false)) => {
                if let Some(path) = memory.as_deref() {
                    remember_crashed(path, renderer);
                }
                eprintln!(
                    "Error: the window's web process crashed while it came up. The {} \
                     renderer worked before, so the next start tries it once more before the \
                     others. {WEB_PROCESS_HELP}",
                    renderer.name()
                );
                return Some(1);
            }
            Ok(outcome @ (Outcome::Failed | Outcome::WebProcessCrashed)) => {
                only_web_process_crashes &= outcome == Outcome::WebProcessCrashed;
                if let Some(next) = order.get(i + 1) {
                    eprintln!(
                        "Warning: the window could not start with the {} renderer, trying {}.",
                        renderer.name(),
                        next.name()
                    );
                }
            }
            Err(e) => {
                eprintln!("Warning: {e}; starting directly.");
                return None;
            }
        }
    }
    if let Some(path) = memory.as_deref() {
        let _ = std::fs::remove_file(path);
    }
    if only_web_process_crashes {
        eprintln!(
            "Error: the window's web process crashed with every renderer, so the graphics \
             driver is likely not the cause. {WEB_PROCESS_HELP}"
        );
    } else {
        eprintln!(
            "Error: Arnis could not open its window with any renderer. If the messages above \
             say EGL could not start, installing your distribution's Mesa Vulkan drivers can \
             help; the command line works without a window: arnis --help"
        );
    }
    Some(1)
}

/// The remembered renderer first, then the rest in their usual order.
fn attempt_order(remembered: Option<Renderer>) -> Vec<Renderer> {
    let mut order: Vec<Renderer> = remembered.into_iter().collect();
    order.extend(
        Renderer::ORDER
            .into_iter()
            .filter(|r| Some(*r) != remembered),
    );
    order
}

fn memory_file() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("arnis").join("linux-renderer"))
}

/// The remembered renderer, and whether its web process has crashed once since it worked.
fn read_remembered(path: &Path) -> Option<(Renderer, bool)> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    match text.strip_suffix('?') {
        Some(name) => Some((Renderer::from_name(name)?, true)),
        None => Some((Renderer::from_name(text)?, false)),
    }
}

fn remember_crashed(path: &Path, renderer: Renderer) {
    let _ = std::fs::write(path, format!("{}?", renderer.name()));
}

fn remember(path: &Path, renderer: Renderer) {
    if renderer == Renderer::Software {
        let _ = std::fs::remove_file(path);
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, renderer.name());
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Exited(i32),
    /// The window never came up: the child crashed within `STARTUP`, or reported EGL failing.
    Failed,
    /// The window's web process crashed while it came up.
    WebProcessCrashed,
}

/// Runs `cmd` until it exits, its output going straight to this process's. `started` runs
/// once the child has outlived `startup`, or exits cleanly before it. Errs only when the
/// child could not be started at all; once it runs, this process must not open a window too.
fn run_child(
    mut cmd: Command,
    startup: Duration,
    started: impl FnOnce(),
) -> Result<Outcome, String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start the GUI process: {e}"))?;
    let begun = Instant::now();
    let mut started = Some(started);
    let lost = |e: std::io::Error| {
        eprintln!("Warning: lost track of the GUI process: {e}");
        Ok(Outcome::Exited(1))
    };
    loop {
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(e) => return lost(e),
        };
        if let Some(status) = status {
            let early = begun.elapsed() < startup;
            if let Some(failed) = renderer_failure(status, early) {
                return Ok(failed);
            }
            if status.success() || !early {
                if let Some(started) = started.take() {
                    started();
                }
            }
            return Ok(Outcome::Exited(exit_code(status)));
        }
        if begun.elapsed() >= startup {
            if let Some(started) = started.take() {
                started();
            }
            return match child.wait() {
                Ok(status) => Ok(renderer_failure(status, false)
                    .unwrap_or_else(|| Outcome::Exited(exit_code(status)))),
                Err(e) => lost(e),
            };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// How the child's end says its renderer failed, if it does. A crash counts only `early`.
fn renderer_failure(status: ExitStatus, early: bool) -> Option<Outcome> {
    let crashed = || {
        status
            .signal()
            .is_some_and(|signal| CRASH_SIGNALS.contains(&signal))
    };
    match status.code() {
        Some(EGL_FAILED_EXIT) => Some(Outcome::Failed),
        Some(WEB_PROCESS_CRASHED_EXIT) => Some(Outcome::WebProcessCrashed),
        _ if early && crashed() => Some(Outcome::Failed),
        _ => None,
    }
}

/// The child's exit code, or the shell's 128 plus the signal that ended it.
fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        cmd
    }

    /// The outcome of `script` given `startup`, and whether it counted as started.
    fn run(script: &str, startup: Duration) -> (Outcome, bool) {
        let started = Cell::new(false);
        let outcome = run_child(sh(script), startup, || started.set(true)).unwrap();
        (outcome, started.get())
    }

    #[test]
    fn a_crash_or_an_egl_error_while_starting_is_a_failed_renderer() {
        let startup = Duration::from_secs(10);
        assert_eq!(run("kill -ABRT $$", startup), (Outcome::Failed, false));
        assert_eq!(run("kill -SEGV $$", startup), (Outcome::Failed, false));
        assert_eq!(run("exit 86", startup), (Outcome::Failed, false));
    }

    #[test]
    fn an_ordinary_exit_or_a_stop_is_passed_on_and_not_retried() {
        let startup = Duration::from_secs(10);
        assert_eq!(run("exit 0", startup), (Outcome::Exited(0), true));
        // An error that is not EGL's says nothing about the renderer.
        assert_eq!(run("exit 3", startup), (Outcome::Exited(3), false));
        assert_eq!(run("kill -TERM $$", startup), (Outcome::Exited(143), false));
    }

    #[test]
    fn a_crash_after_the_window_is_up_is_not_the_renderers() {
        let started = Cell::new(false);
        let outcome = run_child(
            sh("sleep 1; kill -ABRT $$"),
            Duration::from_millis(200),
            || started.set(true),
        )
        .unwrap();
        assert_eq!(outcome, Outcome::Exited(128 + libc::SIGABRT));
        assert!(started.get(), "remembered once it outlived the start");
    }

    #[test]
    fn an_egl_failure_the_child_reports_late_is_still_the_renderers() {
        // A slow start can put the child's own window past the launcher's.
        let (outcome, started) = run("sleep 1; exit 86", Duration::from_millis(200));
        assert_eq!(outcome, Outcome::Failed);
        assert!(started);
    }

    #[test]
    fn a_web_process_crash_is_told_apart_whenever_it_comes() {
        let startup = Duration::from_secs(10);
        assert_eq!(run("exit 87", startup), (Outcome::WebProcessCrashed, false));
        let late = run("sleep 1; exit 87", Duration::from_millis(200));
        assert_eq!(late, (Outcome::WebProcessCrashed, true));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn only_a_web_process_crash_while_starting_fails_the_renderer() {
        use webkit2gtk::WebProcessTerminationReason as Reason;
        let early = Duration::from_secs(3);
        assert!(web_process_failed_start(Reason::Crashed, early));
        assert!(!web_process_failed_start(Reason::Crashed, STARTUP));
        assert!(!web_process_failed_start(
            Reason::ExceededMemoryLimit,
            early
        ));
        assert!(!web_process_failed_start(Reason::TerminatedByApi, early));
    }

    #[test]
    fn egl_errors_are_told_from_other_errors() {
        assert!(is_egl_error(
            "Error while starting the application UI (Tauri): EGL_NOT_INITIALIZED"
        ));
        assert!(is_egl_error("Could not create surfaceless EGL display"));
        assert!(is_egl_error("libEGL: eglInitialize failed"));
        assert!(!is_egl_error("Failed to get main window"));
    }

    #[test]
    fn the_remembered_renderer_goes_first() {
        use Renderer::*;
        assert_eq!(attempt_order(None), vec![Software, Zink, Gpu, ZinkGpu]);
        assert_eq!(attempt_order(Some(Gpu)), vec![Gpu, Software, Zink, ZinkGpu]);
        assert_eq!(
            attempt_order(Some(ZinkGpu)),
            vec![ZinkGpu, Software, Zink, Gpu]
        );
    }

    #[test]
    fn remembers_only_a_renderer_other_than_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("arnis").join("linux-renderer");
        remember(&path, Renderer::Zink);
        assert_eq!(read_remembered(&path), Some((Renderer::Zink, false)));
        remember(&path, Renderer::Software);
        assert!(!path.exists());
        assert_eq!(Renderer::from_name("gpu\n"), Some(Renderer::Gpu));
        assert_eq!(Renderer::from_name("zink-gpu"), Some(Renderer::ZinkGpu));
        assert_eq!(Renderer::from_name("vulkan"), None);
    }

    #[test]
    fn a_web_process_crash_is_remembered_until_the_renderer_works_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("linux-renderer");
        remember(&path, Renderer::Gpu);
        remember_crashed(&path, Renderer::Gpu);
        assert_eq!(read_remembered(&path), Some((Renderer::Gpu, true)));
        remember(&path, Renderer::Gpu);
        assert_eq!(read_remembered(&path), Some((Renderer::Gpu, false)));
    }
}
