//! macOS process control. Linux will get a sibling file rather than cfg blocks
//! threaded through this one.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::ExitStatus;

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
