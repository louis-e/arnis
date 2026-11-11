//! Pieces die with their coordinator. A piece writing after the coordinator
//! is gone would write into a world nobody holds, so this is not left to
//! cleanup code that a killed process never runs.
//!
//! Windows: every piece joins one Job Object with KILL_ON_JOB_CLOSE. Its
//! handle is never closed, so the kernel closes it when the coordinator ends,
//! however it ends, and that kills the pieces.
//! Unix: a piece gets its own process group and a stdin pipe from the
//! coordinator, and exits when that pipe closes (`watch_parent`).

use std::process::{Child, Command};

/// Set up `cmd` so the piece it starts can be tied to this process.
pub fn prepare(cmd: &mut Command) {
    cmd.stdin(std::process::Stdio::piped());
    // A piece of a job started from the window must not open a console of
    // its own (a release build has none to open; a debug build would).
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
}

/// Ties a started piece to this process's lifetime.
#[cfg(windows)]
pub fn adopt(child: &Child) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    // The handle as an integer: it lives as long as the process and is only
    // passed back to the kernel.
    static JOB: OnceLock<Result<isize, String>> = OnceLock::new();
    let job = JOB.get_or_init(|| {
        // SAFETY: plain Win32 calls on a job object this function owns; the
        // limit struct is a live local of the size passed.
        unsafe {
            let job = CreateJobObjectW(None, windows::core::PCWSTR::null())
                .map_err(|e| format!("CreateJobObject: {e}"))?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .map_err(|e| format!("SetInformationJobObject: {e}"))?;
            Ok(job.0 as isize)
        }
    });
    let job = HANDLE(*job.as_ref().map_err(Clone::clone)? as *mut core::ffi::c_void);
    // SAFETY: both handles are open: the job's for the life of the process,
    // the child's for as long as `child` is borrowed.
    unsafe { AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())) }
        .map_err(|e| format!("AssignProcessToJobObject: {e}"))
}

#[cfg(not(windows))]
pub fn adopt(_child: &Child) -> Result<(), String> {
    Ok(())
}

/// In a piece: exit as soon as the coordinator's end of stdin closes.
pub fn watch_parent() {
    #[cfg(unix)]
    std::thread::spawn(|| {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        eprintln!("The job's coordinator is gone; stopping this piece.");
        std::process::exit(1);
    });
}

/// Test hook for a piece that dies mid-build. `ARNIS_TEST_FAIL_PIECE` names a
/// file holding a piece number (from 1) and a delay in seconds: that piece
/// exits as a panic does once the delay is up, and the file is removed first,
/// so the piece's retry builds. Unset, it costs one environment lookup.
pub fn fail_for_test(piece: usize) {
    let Some(path) = std::env::var_os("ARNIS_TEST_FAIL_PIECE") else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let mut values = text.split_whitespace().map(|v| v.parse::<u64>().ok());
    if values.next().flatten() != Some(piece as u64 + 1) || std::fs::remove_file(&path).is_err() {
        return;
    }
    let delay = std::time::Duration::from_secs(values.next().flatten().unwrap_or(0));
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        eprintln!(
            "Error: piece {} stopped by ARNIS_TEST_FAIL_PIECE",
            piece + 1
        );
        // 101 is a panic's code, which the coordinator does not retry.
        std::process::exit(101);
    });
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn a_piece_can_join_the_kill_on_close_job() {
        let mut cmd = std::process::Command::new("cmd");
        // Waits for stdin to close, so it is still running when adopted.
        cmd.args(["/C", "more"]);
        super::prepare(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        super::adopt(&child).unwrap();
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
    }
}
