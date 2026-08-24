//! macOS process control. Linux will get a sibling file rather than cfg blocks
//! threaded through this one.

use std::io;
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
