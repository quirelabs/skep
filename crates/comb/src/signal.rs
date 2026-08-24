//! The one place the engine speaks to the operating system about signals.
//! Keeping it to a single function means a Linux port changes nothing here.

use std::io;

/// Ask a process to shut down cleanly. Escalating to SIGKILL once the grace
/// period expires is the caller's job.
#[cfg(unix)]
pub fn terminate(pid: u32) -> io::Result<()> {
    // Safety: kill is safe for any pid; an unknown one just returns ESRCH.
    let sent = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if sent == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
pub fn terminate(_pid: u32) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "graceful shutdown needs unix signals",
    ))
}
