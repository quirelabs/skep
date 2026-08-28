//! macOS process control. Linux will get a sibling file rather than cfg blocks
//! threaded through this one.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, ExitStatus};

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

/// Whether a copy-on-write clone can apply between these two places. It needs
/// one APFS volume, so this is checked rather than attempted: the difference
/// between instant and "copying the whole thing" is worth saying out loud
/// before it starts, not after.
pub fn can_clone(from: &Path, into: &Path) -> bool {
    match (volume(from), volume(into)) {
        (Some(source), Some(target)) => source == target && source.1 == "apfs",
        _ => false,
    }
}

/// Identity of the filesystem a path sits on, and what kind it is. The device
/// id answers "same volume"; statfs answers "which kind".
fn volume(path: &Path) -> Option<(u64, String)> {
    use std::os::unix::fs::MetadataExt;

    let device = std::fs::metadata(path).ok()?.dev();
    let name = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut info: libc::statfs = unsafe { std::mem::zeroed() };
    // Safety: statfs writes into a buffer we own, for a path we own.
    if unsafe { libc::statfs(name.as_ptr(), &mut info) } != 0 {
        return None;
    }
    // Safety: f_fstypename is a NUL terminated name filled in by the kernel.
    let kind = unsafe { CStr::from_ptr(info.f_fstypename.as_ptr()) }
        .to_string_lossy()
        .to_string();
    Some((device, kind))
}

/// A copy-on-write clone. The destination must not exist yet.
pub fn clone_directory(from: &Path, into: &Path) -> io::Result<()> {
    let source = CString::new(from.as_os_str().as_bytes())?;
    let target = CString::new(into.as_os_str().as_bytes())?;
    // Safety: both paths are NUL terminated and owned for the call.
    if unsafe { libc::clonefile(source.as_ptr(), target.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Who this process is really running as.
pub fn effective_user() -> u32 {
    // Safety: geteuid cannot fail and touches nothing.
    unsafe { libc::geteuid() }
}

pub fn effective_group() -> u32 {
    // Safety: getegid cannot fail and touches nothing.
    unsafe { libc::getegid() }
}

/// The launchd job that owns the privileged ports.
pub const HELPER_LABEL: &str = "com.quirelabs.skep.helper";

/// A launchd daemon description. Started at boot and restarted if it dies,
/// because a forwarder that stays down makes every local domain fail with no
/// hint as to why.
pub fn daemon_plist(label: &str, program: &Path, args: &[String]) -> String {
    let mut arguments = String::new();
    for value in std::iter::once(program.display().to_string()).chain(args.iter().cloned()) {
        arguments.push_str(&format!("    <string>{}</string>\n", escaped(&value)));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20 <key>Label</key>\n\
         \x20 <string>{label}</string>\n\
         \x20 <key>ProgramArguments</key>\n\
         \x20 <array>\n{arguments}\x20 </array>\n\
         \x20 <key>RunAtLoad</key>\n\
         \x20 <true/>\n\
         \x20 <key>KeepAlive</key>\n\
         \x20 <true/>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// A path with an ampersand in it would otherwise produce a plist launchd
/// refuses to read.
fn escaped(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn load_daemon(plist: &Path) -> io::Result<()> {
    launchctl(&["bootstrap", "system"], Some(plist))
}

pub fn unload_daemon(label: &str) -> io::Result<()> {
    launchctl(&["bootout", &format!("system/{label}")], None)
}

fn launchctl(args: &[&str], path: Option<&Path>) -> io::Result<()> {
    let mut command = Command::new("launchctl");
    command.args(args);
    if let Some(path) = path {
        command.arg(path);
    }
    let output = command.output()?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(io::Error::other(if said.is_empty() {
        format!("launchctl {} failed", args.join(" "))
    } else {
        said
    }))
}

/// macOS caches resolution, so a file that has been written is not proof that
/// anything resolves through it yet.
pub fn flush_dns() -> io::Result<()> {
    let _ = Command::new("dscacheutil").arg("-flushcache").output()?;
    let _ = Command::new("killall")
        .args(["-HUP", "mDNSResponder"])
        .output()?;
    Ok(())
}

/// Asks the system resolver, not our own server, so the answer proves the
/// whole path works rather than that we can talk to ourselves.
pub fn resolves_to(name: &str) -> Vec<String> {
    let Ok(output) = Command::new("dscacheutil")
        .args(["-q", "host", "-a", "name", name])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("ip_address: "))
        .map(|address| address.trim().to_string())
        .collect()
}

/// Gives up root for good. The group goes first, because after the user id
/// changes there is no privilege left to change it with.
pub fn drop_privileges(uid: u32, gid: u32) -> io::Result<()> {
    if uid == 0 {
        return Err(io::Error::other("refusing to drop privileges to root"));
    }
    // Safety: both calls only change this process's credentials.
    unsafe {
        if libc::setgid(gid) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::setuid(uid) != 0 {
            return Err(io::Error::last_os_error());
        }
        // If root can be picked back up, it was never really given away.
        if libc::setuid(0) == 0 {
            return Err(io::Error::other(
                "root was still available after dropping it",
            ));
        }
    }
    Ok(())
}

/// Where macOS looks when it sends a whole domain somewhere other than the
/// usual resolvers. Writing here needs root, which is why it is the last thing
/// local domains need and the first thing that asks for a password.
pub fn resolver_file(suffix: &str) -> std::path::PathBuf {
    std::path::PathBuf::from("/etc/resolver").join(suffix)
}

/// The system trust store. Writing to it is what needs an administrator, and
/// why trusting the root is a step a person takes on purpose.
const SYSTEM_KEYCHAIN: &str = "/Library/Keychains/System.keychain";

/// Writes a file only its owner can read. The mode is set as the file is
/// created, so a secret is never briefly readable by anyone else.
pub fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // A file that already existed keeps its old mode, so say it again.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(contents.as_bytes())
}

pub fn trust_root(certificate: &Path) -> io::Result<()> {
    security(
        &[
            "add-trusted-cert",
            "-d",
            "-r",
            "trustRoot",
            "-k",
            SYSTEM_KEYCHAIN,
        ],
        certificate,
    )
}

pub fn untrust_root(certificate: &Path) -> io::Result<()> {
    security(&["remove-trusted-cert", "-d"], certificate)
}

/// Whether this machine would accept what the root signs.
pub fn root_is_trusted(certificate: &Path) -> bool {
    Command::new("security")
        .args(["verify-cert", "-c"])
        .arg(certificate)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Blocking on purpose: these prompt for a password, and the caller is a person
/// waiting for that prompt rather than the event loop.
fn security(args: &[&str], certificate: &Path) -> io::Result<()> {
    let output = Command::new("security")
        .args(args)
        .arg(certificate)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(io::Error::other(if reason.is_empty() {
        "the keychain refused the certificate".to_string()
    } else {
        reason
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_is_possible_within_one_volume() {
        let root = std::env::temp_dir();
        assert!(
            can_clone(&root, &root),
            "the temp directory should be APFS on any machine this runs on"
        );
    }

    #[test]
    fn a_path_that_is_not_there_cannot_be_cloned() {
        assert!(!can_clone(
            Path::new("/nowhere/at/all"),
            &std::env::temp_dir()
        ));
    }

    #[test]
    fn cloning_reproduces_a_tree() {
        let root = std::env::temp_dir().join(format!("skep-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("from").join("nested")).unwrap();
        std::fs::write(root.join("from").join("nested").join("file"), b"data").unwrap();

        clone_directory(&root.join("from"), &root.join("into")).unwrap();

        assert_eq!(
            std::fs::read(root.join("into").join("nested").join("file")).unwrap(),
            b"data"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
