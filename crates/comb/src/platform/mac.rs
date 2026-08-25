//! macOS process control. Linux will get a sibling file rather than cfg blocks
//! threaded through this one.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::ExitStatus;

use std::time::Duration;

use tokio::time::timeout;

use crate::ports::Listener;
use crate::spec::StopSignal;

/// Ask a process to shut down cleanly. Escalating to SIGKILL once the grace
/// period expires is the caller's job.
pub fn terminate(pid: u32, signal: StopSignal) -> io::Result<()> {
    let number = match signal {
        StopSignal::Term => libc::SIGTERM,
        StopSignal::Int => libc::SIGINT,
        StopSignal::Quit => libc::SIGQUIT,
    };
    // Safety: kill is safe for any pid; an unknown one just returns ESRCH.
    let sent = unsafe { libc::kill(pid as libc::pid_t, number) };
    if sent == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Turns a child's exit into the sentence that lands in `Failed { reason }`.
pub fn describe_exit(status: &ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;

    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => "exited for an unknown reason".to_string(),
    }
}

/// Takes an exclusive lock without blocking, returning false when someone else
/// holds it. The kernel drops the lock when the file description closes, which
/// includes the process dying, so a crashed host never leaves it stuck.
pub fn try_lock_exclusive(file: &File) -> io::Result<bool> {
    // Safety: flock only touches the descriptor's lock state.
    let taken = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if taken == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EWOULDBLOCK) => Ok(false),
        _ => Err(error),
    }
}

/// Restricts a path to its owner. Sockets and the directory holding them carry
/// the right to drive every service on the machine.
pub fn restrict(path: &Path, mode: u32) -> io::Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Asks lsof who is listening. A diagnostic, so every failure is silence
/// rather than an error: never let the explanation break the operation.
pub async fn listener_on(port: u16) -> Option<Listener> {
    let lsof = tokio::process::Command::new("lsof")
        .args(["-nP", "+c", "0", "-sTCP:LISTEN", "-F", "pc"])
        .arg(format!("-iTCP:{port}"))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();

    // lsof can stall on a busy machine, and a stalled explanation is worse
    // than none.
    let output = timeout(Duration::from_secs(2), lsof).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&output.stdout);

    let mut pid = None;
    let mut command = None;
    for line in text.lines() {
        match line.split_at_checked(1) {
            Some(("p", rest)) => pid = rest.trim().parse().ok(),
            Some(("c", rest)) => command = Some(rest.trim().to_string()),
            _ => {}
        }
        if pid.is_some() && command.is_some() {
            break;
        }
    }

    let pid = pid?;
    Some(Listener {
        executable: executable_of(pid).await,
        command: command.unwrap_or_else(|| "an unknown process".to_string()),
        pid,
    })
}

async fn executable_of(pid: u32) -> Option<String> {
    let output = tokio::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// Checks the machine can compile before anything is downloaded, so a missing
/// toolchain is a sentence rather than a cryptic make failure minutes in.
pub async fn build_tools_missing() -> Option<String> {
    let ran = |program: &'static str, arg: &'static str| async move {
        tokio::process::Command::new(program)
            .arg(arg)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|status| status.success())
            .unwrap_or(false)
    };

    if !ran("xcode-select", "-p").await || !ran("cc", "--version").await {
        return Some(
            "the Xcode command line tools are not installed. Install them with \
             `xcode-select --install`, then try again"
                .to_string(),
        );
    }
    if !ran("make", "-v").await {
        return Some("make is not available on this machine".to_string());
    }
    None
}
