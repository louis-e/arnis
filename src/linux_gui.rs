//! Starting the GUI on Linux. WebKitGTK aborts the whole process when it cannot set up
//! EGL ("Could not create surfaceless EGL display: EGL_NOT_INITIALIZED. Aborting..."),
//! so the failure never reaches Rust as an error. The GUI therefore runs in a child
//! process, and a child that dies that way is started again with the next renderer
//! that has worked for users: zink (#799, #833) and the GPU's own driver (#1247).
//! The one that worked is remembered, so later starts don't crash first.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Set on the child, naming the renderer it runs with.
const RENDERER_ENV: &str = "ARNIS_LINUX_RENDERER";

/// Rendering variables a user may set to pick the driver themselves.
const USER_OVERRIDES: [&str; 3] = [
    "MESA_LOADER_DRIVER_OVERRIDE",
    "LIBGL_ALWAYS_SOFTWARE",
    "GALLIUM_DRIVER",
];

/// How long a child that said it is aborting gets to exit before it is killed.
const ABORT_GRACE: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderer {
    /// Mesa's software rasterizer, set in `gui::run_gui`. The default.
    Software,
    /// Mesa's zink driver over Vulkan, on top of the software defaults.
    Zink,
    /// The GPU's own driver, with no software rendering forced.
    Gpu,
}

impl Renderer {
    const ORDER: [Renderer; 3] = [Renderer::Software, Renderer::Zink, Renderer::Gpu];

    fn name(self) -> &'static str {
        match self {
            Renderer::Software => "software",
            Renderer::Zink => "zink",
            Renderer::Gpu => "gpu",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ORDER.into_iter().find(|r| r.name() == name.trim())
    }

    fn configure(self, cmd: &mut Command) {
        cmd.env(RENDERER_ENV, self.name());
        if self == Renderer::Zink {
            cmd.env("MESA_LOADER_DRIVER_OVERRIDE", "zink");
        }
    }
}

/// The renderer this process was started with, if `supervise` started it.
pub fn current() -> Option<Renderer> {
    std::env::var(RENDERER_ENV)
        .ok()
        .and_then(|name| Renderer::from_name(&name))
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
    let order = attempt_order(remembered);
    for (i, &renderer) in order.iter().enumerate() {
        let mut cmd = Command::new(&exe);
        renderer.configure(&mut cmd);
        match run_child(cmd) {
            Ok(Outcome::Exited(code)) => {
                if Some(renderer) != remembered {
                    if let Some(path) = memory.as_deref() {
                        remember(path, renderer);
                    }
                }
                return Some(code);
            }
            Ok(Outcome::EglFailed) => {
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
    eprintln!(
        "Error: Arnis could not open its window because no graphics driver would start \
         (EGL). Installing your distribution's Mesa Vulkan drivers can help, or use \
         the command line instead: arnis --help"
    );
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

fn read_remembered(path: &Path) -> Option<Renderer> {
    Renderer::from_name(&std::fs::read_to_string(path).ok()?)
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
    EglFailed,
}

/// WebKitGTK's last words before it aborts on EGL, in all the forms reported so far:
/// "Could not create surfaceless EGL display: EGL_NOT_INITIALIZED. Aborting..." and
/// "Could not create default EGL display: EGL_BAD_PARAMETER. Aborting...".
fn is_egl_abort(line: &str) -> bool {
    line.contains("EGL display") && line.contains("Aborting")
}

/// Runs `cmd` with its stderr passed through, until it exits or WebKit says it is
/// aborting on EGL, whichever comes first.
fn run_child(mut cmd: Command) -> Result<Outcome, String> {
    let mut child = cmd
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start the GUI process: {e}"))?;
    let stderr = child.stderr.take().ok_or("the GUI process has no stderr")?;
    let (tx, rx) = mpsc::channel();
    let forward = std::thread::spawn(move || {
        let mut out = std::io::stderr();
        for line in BufReader::new(stderr).split(b'\n') {
            let Ok(line) = line else { break };
            let _ = out.write_all(&line);
            let _ = out.write_all(b"\n");
            if is_egl_abort(&String::from_utf8_lossy(&line)) {
                let _ = tx.send(());
            }
        }
    });
    // Not joined: WebKit's helper processes share the pipe and may hold it open a
    // little past the child.
    drop(forward);
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(()) => {
                // The abort may be the web process's alone, which leaves a blank window.
                end_within(&mut child, ABORT_GRACE);
                return Ok(Outcome::EglFailed);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                    // An abort line can still be in the pipe when the exit is noticed.
                    if rx.recv_timeout(Duration::from_millis(500)).is_ok() {
                        return Ok(Outcome::EglFailed);
                    }
                    return Ok(Outcome::Exited(status.code().unwrap_or(1)));
                }
            }
            // The pipe closed without an abort line, so the child is done or about to be.
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let status = child.wait().map_err(|e| e.to_string())?;
                return Ok(Outcome::Exited(status.code().unwrap_or(1)));
            }
        }
    }
}

fn end_within(child: &mut Child, grace: Duration) {
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        cmd
    }

    #[test]
    fn recognises_both_reported_abort_lines() {
        assert!(is_egl_abort(
            "Could not create surfaceless EGL display: EGL_NOT_INITIALIZED. Aborting..."
        ));
        assert!(is_egl_abort(
            "Could not create default EGL display: EGL_BAD_PARAMETER. Aborting..."
        ));
        // Mesa's warnings come before the abort, and on their own are not fatal.
        assert!(!is_egl_abort(
            "MESA-EGL: warning: egl: failed to create dri2 screen"
        ));
        assert!(!is_egl_abort(
            "libEGL warning: DRI2: failed to create screen"
        ));
    }

    #[test]
    fn an_egl_abort_is_told_apart_from_an_ordinary_exit() {
        let aborted = sh("echo 'Could not create surfaceless EGL display: \
             EGL_NOT_INITIALIZED. Aborting...' >&2; kill -ABRT $$");
        assert_eq!(run_child(aborted).unwrap(), Outcome::EglFailed);
        assert_eq!(
            run_child(sh("echo fine >&2; exit 3")).unwrap(),
            Outcome::Exited(3)
        );
        assert_eq!(run_child(sh("exit 0")).unwrap(), Outcome::Exited(0));
    }

    #[test]
    fn a_child_left_hanging_after_the_abort_line_is_ended() {
        let hung = sh(
            "echo 'Could not create default EGL display: EGL_BAD_PARAMETER. \
             Aborting...' >&2; sleep 30",
        );
        let started = Instant::now();
        assert_eq!(run_child(hung).unwrap(), Outcome::EglFailed);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn the_remembered_renderer_goes_first() {
        use Renderer::*;
        assert_eq!(attempt_order(None), vec![Software, Zink, Gpu]);
        assert_eq!(attempt_order(Some(Gpu)), vec![Gpu, Software, Zink]);
        assert_eq!(attempt_order(Some(Zink)), vec![Zink, Software, Gpu]);
    }

    #[test]
    fn remembers_only_a_renderer_other_than_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("arnis").join("linux-renderer");
        remember(&path, Renderer::Zink);
        assert_eq!(read_remembered(&path), Some(Renderer::Zink));
        remember(&path, Renderer::Software);
        assert!(!path.exists());
        assert_eq!(Renderer::from_name("gpu\n"), Some(Renderer::Gpu));
        assert_eq!(Renderer::from_name("vulkan"), None);
    }
}
