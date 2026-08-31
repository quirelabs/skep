//! Installing the privileged half of local domains: a launchd daemon that owns
//! ports 80 and 443, and the resolver file that sends a whole domain here.
//!
//! Both are the only things skep asks root for, so the code is split in two.
//! Putting files in place is ordinary file work and can be proved in a test
//! against a temporary directory. Handing the job to launchd and flushing the
//! resolver cache is the part that needs the machine to cooperate.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::dns;
use crate::error::{Error, Result};
use crate::platform;
use crate::proxy::{HTTP_PORT, HTTPS_PORT};

/// Bumped whenever the helper's control protocol changes. A helper left behind
/// by an older install answers with its own number rather than pretending.
pub const HELPER_PROTOCOL: u32 = 1;

/// Every path an install touches, in one place so uninstall can undo exactly
/// what install did and a test can point the whole lot somewhere harmless.
#[derive(Clone, Debug)]
pub struct Layout {
    pub label: String,
    pub plist: PathBuf,
    pub helper: PathBuf,
    pub control: PathBuf,
    pub resolver: PathBuf,
    pub backup: PathBuf,
}

impl Layout {
    pub fn system(suffix: &str) -> Self {
        let label = platform::HELPER_LABEL.to_string();
        Self {
            plist: PathBuf::from("/Library/LaunchDaemons").join(format!("{label}.plist")),
            helper: PathBuf::from("/usr/local/libexec/skep-helper"),
            control: PathBuf::from("/var/run/skep-helper.sock"),
            resolver: platform::resolver_file(suffix),
            backup: PathBuf::from("/Library/Application Support/Skep/resolver.backup"),
            label,
        }
    }

    /// The same shape under a directory, so install and uninstall can be
    /// proved without touching the machine.
    pub fn under(root: &Path, suffix: &str) -> Self {
        let label = platform::HELPER_LABEL.to_string();
        Self {
            plist: root.join("LaunchDaemons").join(format!("{label}.plist")),
            helper: root.join("libexec").join("skep-helper"),
            control: root.join("run").join("skep-helper.sock"),
            resolver: root.join("resolver").join(suffix),
            backup: root.join("state").join("resolver.backup"),
            label,
        }
    }
}

/// Who the helper becomes once it has the ports. Never root.
#[derive(Clone, Copy, Debug)]
pub struct Owner {
    pub uid: u32,
    pub gid: u32,
}

/// What a host managed to start for local domains. Reported rather than
/// printed, so the command line and the app can each say it their own way.
#[derive(Debug)]
pub struct Serving {
    pub https: Option<u16>,
    pub http: Option<u16>,
    pub dns: Option<u16>,
    /// The port to put in front of a person. Equal to `https` until a helper
    /// forwards a privileged port here, and 443 once one does.
    pub public_https: u16,
    /// Anything that did not start, in words worth showing a person.
    pub trouble: Vec<String>,
}

impl Default for Serving {
    fn default() -> Self {
        Self {
            https: None,
            http: None,
            dns: None,
            public_https: HTTPS_PORT,
            trouble: Vec::new(),
        }
    }
}

/// The port a browser should be sent to.
///
/// Asked of the helper rather than assumed. The helper was installed to forward
/// something specific, and what it actually forwards is the only honest answer;
/// an install that forwards a different port should not be guessed at. No
/// helper, or a helper that does not reach us, means our own port is the public
/// one, which is what milestone 13.2 shipped and still works.
pub async fn public_https_port(control: &Path) -> u16 {
    match health(control).await {
        Ok(health) => health
            .forwarding
            .iter()
            .find(|forward| forward.to == HTTPS_PORT)
            .map(|forward| forward.from)
            .unwrap_or(HTTPS_PORT),
        Err(_) => HTTPS_PORT,
    }
}

/// Starts everything local domains need beside a host, and stops it when the
/// host stops. Started whether or not any site is configured yet, because a
/// project running `skep up` adds sites to a host that is already going.
pub async fn serve_alongside(
    host: &crate::host::Host,
    authority: std::sync::Arc<crate::certs::Authority>,
    suffix: &str,
) -> Serving {
    let mut serving = Serving {
        public_https: public_https_port(&Layout::system(suffix).control).await,
        ..Default::default()
    };

    match tokio::net::TcpListener::bind(("127.0.0.1", HTTPS_PORT)).await {
        Ok(listener) => {
            let mut quitting = host.quitting();
            let sites = host.engine().sites();
            tokio::spawn(crate::proxy::serve(
                listener,
                sites,
                authority,
                async move {
                    let _ = quitting.changed().await;
                },
            ));
            serving.https = Some(HTTPS_PORT);
        }
        Err(error) => serving
            .trouble
            .push(format!("port {HTTPS_PORT} is not available: {error}")),
    }

    match tokio::net::TcpListener::bind(("127.0.0.1", HTTP_PORT)).await {
        Ok(listener) => {
            let mut quitting = host.quitting();
            tokio::spawn(crate::proxy::redirect(listener, serving.public_https, async move {
                let _ = quitting.changed().await;
            }));
            serving.http = Some(HTTP_PORT);
        }
        Err(error) => serving
            .trouble
            .push(format!("port {HTTP_PORT} is not available: {error}")),
    }

    match tokio::net::UdpSocket::bind(("127.0.0.1", crate::dns::PORT)).await {
        Ok(socket) => {
            let mut quitting = host.quitting();
            tokio::spawn(crate::dns::serve(socket, suffix.to_string(), async move {
                let _ = quitting.changed().await;
            }));
            serving.dns = Some(crate::dns::PORT);
        }
        Err(error) => serving
            .trouble
            .push(format!("not answering .{suffix} names: {error}")),
    }

    if let crate::dns::Routing::Elsewhere { says } = crate::dns::routing(suffix) {
        serving.trouble.push(format!(
            "something else routes .{suffix} ({says}), so names will not reach skep"
        ));
    }

    serving
}

/// Whether this process could write to the places an install touches.
pub fn is_root() -> bool {
    platform::effective_user() == 0
}

/// Who the helper should become: the person who ran sudo, not root, and not
/// whoever the daemon would otherwise inherit.
pub fn invoking_user() -> Owner {
    let read = |name: &str| {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
    };
    Owner {
        uid: read("SUDO_UID").unwrap_or_else(platform::effective_user),
        gid: read("SUDO_GID").unwrap_or_else(platform::effective_group),
    }
}

/// Gives up root for good. The helper calls this the moment it holds the
/// ports, so the window in which it can do anything else is a few
/// milliseconds at startup.
/// Gives a path to the user this process is about to become, and restricts it
/// to that user.
///
/// Called on the control socket before root is dropped. Binding as root leaves
/// the socket owned by root at the process umask, and since connecting needs
/// write permission the engine gets EACCES and reports no helper at all.
pub fn hand_over(path: &Path, owner: Owner) -> Result<()> {
    platform::give_to(path, owner.uid, owner.gid).map_err(Error::Io)?;
    platform::restrict(path, 0o660).map_err(Error::Io)
}

pub fn become_user(owner: Owner) -> Result<()> {
    platform::drop_privileges(owner.uid, owner.gid).map_err(Error::Io)
}

/// A resolver file that belongs to something else.
#[derive(Clone, Debug)]
pub struct Foreign {
    pub says: String,
    pub likely: Option<String>,
}

/// Whether something other than skep already routes the domain.
pub fn foreign(layout: &Layout) -> Option<Foreign> {
    let text = std::fs::read_to_string(&layout.resolver).ok()?;
    if dns::points_here(&text) {
        return None;
    }
    Some(Foreign {
        says: text.trim().to_string(),
        likely: likely_owner(),
    })
}

/// Naming the probable owner turns a refusal into something a person can act
/// on, the same way a port conflict names what holds the port.
fn likely_owner() -> Option<String> {
    for (marker, name) in [
        ("/Applications/Herd.app", "Laravel Herd"),
        ("/Applications/Valet.app", "Laravel Valet"),
        (
            "/opt/homebrew/sbin/dnsmasq",
            "dnsmasq, probably from Homebrew",
        ),
        ("/usr/local/sbin/dnsmasq", "dnsmasq, probably from Homebrew"),
    ] {
        if Path::new(marker).exists() {
            return Some(name.to_string());
        }
    }
    None
}

/// Puts the files where they belong and reports every path it touched.
/// Refuses rather than trampling a resolver file somebody else wrote, unless
/// asked to take it over, in which case the original is kept.
pub fn place(
    layout: &Layout,
    helper_source: &Path,
    owner: Owner,
    take_over: bool,
) -> Result<Vec<PathBuf>> {
    let mut touched = Vec::new();

    if let Some(found) = foreign(layout) {
        if !take_over {
            return Err(Error::ResolverInUse {
                says: found.says,
                likely: found.likely,
            });
        }
        // Kept rather than overwritten, so the other tool survives a round
        // trip through install and uninstall.
        make_room(&layout.backup)?;
        std::fs::copy(&layout.resolver, &layout.backup).map_err(Error::Io)?;
        touched.push(layout.backup.clone());
    }

    make_room(&layout.helper)?;
    std::fs::copy(helper_source, &layout.helper).map_err(Error::Io)?;
    platform::restrict(&layout.helper, 0o755).map_err(Error::Io)?;
    touched.push(layout.helper.clone());

    let arguments = vec![
        "--control".to_string(),
        layout.control.display().to_string(),
        "--user".to_string(),
        owner.uid.to_string(),
        "--group".to_string(),
        owner.gid.to_string(),
        "--forward".to_string(),
        format!("80:{HTTP_PORT}"),
        "--forward".to_string(),
        format!("443:{HTTPS_PORT}"),
    ];
    make_room(&layout.plist)?;
    std::fs::write(
        &layout.plist,
        platform::daemon_plist(&layout.label, &layout.helper, &arguments),
    )
    .map_err(Error::Io)?;
    touched.push(layout.plist.clone());

    make_room(&layout.resolver)?;
    std::fs::write(&layout.resolver, dns::resolver_text()).map_err(Error::Io)?;
    touched.push(layout.resolver.clone());

    Ok(touched)
}

/// Undoes `place`. A resolver file that was somebody else's comes back rather
/// than being deleted.
pub fn remove(layout: &Layout) -> Result<Vec<PathBuf>> {
    let mut touched = Vec::new();

    if layout.backup.is_file() {
        std::fs::copy(&layout.backup, &layout.resolver).map_err(Error::Io)?;
        std::fs::remove_file(&layout.backup).map_err(Error::Io)?;
        touched.push(layout.backup.clone());
    } else if layout.resolver.exists() {
        std::fs::remove_file(&layout.resolver).map_err(Error::Io)?;
    }
    touched.push(layout.resolver.clone());

    for path in [&layout.plist, &layout.helper, &layout.control] {
        if path.exists() {
            std::fs::remove_file(path).map_err(Error::Io)?;
        }
        touched.push(path.clone());
    }

    Ok(touched)
}

/// Hands the job to launchd, then proves names actually resolve. A written
/// file is not proof: mDNSResponder caches, so the check has to go out through
/// the system resolver and come back.
pub fn activate(layout: &Layout, suffix: &str) -> Result<String> {
    // launchd refuses to bootstrap a label it already holds, so installing over
    // a running helper failed with "Bootstrap failed: 5" and left the old
    // process serving the new files. Boot the old job out first; a failure here
    // only means there was nothing to remove, which is the ordinary first
    // install.
    let _ = platform::unload_daemon(&layout.label);
    platform::load_daemon(&layout.plist).map_err(Error::Io)?;
    platform::flush_dns().map_err(Error::Io)?;

    let probe = format!("skep-check.{suffix}");
    let found = platform::resolves_to(&probe);
    if found.iter().any(|address| address == "127.0.0.1") {
        Ok(format!(
            "{probe} resolves to 127.0.0.1 through the system resolver"
        ))
    } else if found.is_empty() {
        Err(Error::Domains(format!(
            "the resolver file is written, but {probe} still does not resolve on this machine"
        )))
    } else {
        Err(Error::Domains(format!(
            "{probe} resolves to {} rather than this machine",
            found.join(", ")
        )))
    }
}

pub fn deactivate(layout: &Layout) -> Result<()> {
    platform::unload_daemon(&layout.label).map_err(Error::Io)?;
    platform::flush_dns().map_err(Error::Io)
}

/// What the helper says when asked who it is.
#[derive(Debug, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Health {
    pub protocol: u32,
    pub pid: u32,
    pub forwarding: Vec<Forward>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forward {
    pub from: u16,
    pub to: u16,
}

/// Asks the installed helper what it is. The version comes first, exactly as
/// it does on the engine's socket, so an older helper is a sentence rather
/// than a confusing failure further in.
pub async fn health(control: &Path) -> Result<Health> {
    let stream = UnixStream::connect(control)
        .await
        .map_err(|_| Error::Domains("no helper is running".to_string()))?;
    let mut stream = BufReader::new(stream);

    let hello = serde_json::to_string(&Hello {
        protocol: HELPER_PROTOCOL,
    })
    .map_err(|error| Error::Domains(error.to_string()))?;
    stream
        .get_mut()
        .write_all(format!("{hello}\n").as_bytes())
        .await
        .map_err(Error::Io)?;

    let mut line = String::new();
    stream.read_line(&mut line).await.map_err(Error::Io)?;
    let health: Health = serde_json::from_str(line.trim())
        .map_err(|_| Error::Domains(format!("the helper said something unexpected: {line}")))?;

    if health.protocol != HELPER_PROTOCOL {
        return Err(Error::Domains(format!(
            "the installed helper speaks version {} and this skep speaks {HELPER_PROTOCOL}, so run skep domains install again",
            health.protocol
        )));
    }
    Ok(health)
}

fn make_room(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }
    Ok(())
}
