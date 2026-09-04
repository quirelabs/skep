//! Built-in service adapters. An adapter describes a service and how to check
//! it; the engine owns spawning and supervision and never calls back in here.

mod cloudflared;
pub mod mail;
mod mailpit;
mod mongodb;
mod mysql;
mod postgres;
pub mod project;
mod valkey;

use std::collections::BTreeMap;
use std::path::Path;

use comb::{
    BinarySpec, Build, Client, Error, HealthCheck, InstanceId, Label, Paths, Platform, Port, Probe,
    Release, Request as Wire, Response, Result, ServiceSpec, ServiceStatus, Tag, Version,
};

pub use cloudflared::{Cloudflared, PUBLIC_SUFFIX as TUNNEL_SUFFIX};
pub use mailpit::Mailpit;
pub use mongodb::Mongodb;
pub use mysql::Mysql;
pub use postgres::Postgres;
pub use valkey::Valkey;

/// One pinned artifact in a catalog, keyed by version and platform. Generated
/// by `scripts/pin-release.sh`; the hash is what every download is checked
/// against, so these rows are the trust root.
#[derive(Clone, Copy, Debug)]
pub struct Pin {
    pub version: &'static str,
    pub platform: Platform,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
    pub strip_components: u8,
}

pub trait ServiceAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    /// Where the executable sits inside the unpacked release.
    fn program(&self) -> &'static str;
    fn pins(&self) -> &'static [Pin];
    fn default_version(&self) -> &'static str;
    /// Port names this service listens on, with their defaults.
    fn default_ports(&self) -> &'static [(&'static str, u16)];
    /// Set when the pinned artifact is source that has to be compiled. Part of
    /// the release, so installing and describing a service cannot disagree
    /// about whether a build is needed.
    fn build(&self) -> Option<Build> {
        None
    }
    /// Everything the engine needs to supervise one instance of this service.
    fn spec(&self, request: &Request, paths: &Paths) -> Result<ServiceSpec>;
}

/// What a caller asked for. Anything left out falls back to the adapter.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub version: Option<Version>,
    pub tag: Option<Tag>,
    pub ports: BTreeMap<String, u16>,
    /// What a targeted instance points at: the url a tunnel exposes.
    pub origin: Option<Origin>,
}

/// Where a tunnel sends what arrives. A site is reached on loopback with its
/// name carried in the request, so the proxy routes it and picks its
/// certificate, and nothing depends on the resolver being installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub url: String,
    pub host: Option<String>,
}

impl Origin {
    pub fn service(port: u16) -> Self {
        Self {
            url: format!("http://127.0.0.1:{port}"),
            host: None,
        }
    }

    pub fn site(host: &str, https_port: u16) -> Self {
        Self {
            url: format!("https://127.0.0.1:{https_port}"),
            host: Some(host.to_string()),
        }
    }
}

impl Request {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_version(mut self, version: Version) -> Self {
        self.version = Some(version);
        self
    }

    pub fn with_tag(mut self, tag: Tag) -> Self {
        self.tag = Some(tag);
        self
    }

    pub fn with_origin(mut self, origin: Origin) -> Self {
        self.origin = Some(origin);
        self
    }

    pub fn with_port(mut self, name: impl Into<String>, port: u16) -> Self {
        self.ports.insert(name.into(), port);
        self
    }

    /// The version to use, checked against what the adapter actually has.
    pub fn resolve_version(&self, adapter: &dyn ServiceAdapter) -> Result<Version> {
        let version = match &self.version {
            Some(version) => version.clone(),
            None => Version::new(adapter.default_version())?,
        };
        if versions(adapter).contains(&version) {
            Ok(version)
        } else {
            Err(Error::UnknownVersion {
                service: adapter.name().to_string(),
                requested: version.to_string(),
                known: versions(adapter)
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            })
        }
    }

    pub fn port(&self, adapter: &dyn ServiceAdapter, name: &str) -> Result<u16> {
        if let Some(port) = self.ports.get(name) {
            return Ok(*port);
        }
        adapter
            .default_ports()
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, port)| *port)
            .ok_or_else(|| Error::InvalidId(format!("{} has no port named {name}", adapter.name())))
    }

    pub fn instance(&self, adapter: &dyn ServiceAdapter, version: &Version) -> Result<InstanceId> {
        Ok(InstanceId {
            service: adapter.name().parse()?,
            version: version.clone(),
            tag: self.tag.clone(),
        })
    }
}

/// Resolves a version the way a project file writes one: "17" finds the newest
/// pinned 17.x, while an exact version is taken as given. Matching is on dot
/// boundaries, so "1" never matches "15.14.0".
pub fn resolve(adapter: &dyn ServiceAdapter, requested: &str) -> Result<Version> {
    let known = versions(adapter);
    let wanted: Vec<&str> = requested.split('.').collect();

    known
        .iter()
        .find(|version| {
            let have: Vec<&str> = version.as_str().split('.').collect();
            have.len() >= wanted.len() && have[..wanted.len()] == wanted[..]
        })
        .cloned()
        .ok_or_else(|| Error::UnknownVersion {
            service: adapter.name().to_string(),
            requested: requested.to_string(),
            known: known
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        })
}

/// Versions this adapter can install on this machine, newest first.
pub fn versions(adapter: &dyn ServiceAdapter) -> Vec<Version> {
    let platform = Platform::current();
    adapter
        .pins()
        .iter()
        .filter(|pin| Some(pin.platform) == platform)
        .filter_map(|pin| Version::new(pin.version).ok())
        .collect()
}

pub fn release(adapter: &dyn ServiceAdapter, version: &Version) -> Result<Release> {
    let platform = Platform::current()
        .ok_or_else(|| Error::InvalidId("this platform has no pinned releases".to_string()))?;
    adapter
        .pins()
        .iter()
        .find(|pin| pin.version == version.as_str() && pin.platform == platform)
        .map(|pin| Release {
            version: version.clone(),
            platform: pin.platform,
            url: pin.url.to_string(),
            sha256: pin.sha256.to_string(),
            size: pin.size,
            strip_components: pin.strip_components,
            build: adapter.build(),
        })
        .ok_or_else(|| {
            Error::InvalidId(format!(
                "{} {version} has no pinned release for {platform}",
                adapter.name()
            ))
        })
}

/// Downloads and verifies the release if it is not already installed. Kept a
/// free function so the trait stays object safe.
pub async fn install(adapter: &dyn ServiceAdapter, version: &Version, paths: &Paths) -> Result<()> {
    let release = release(adapter, version)?;
    comb::ensure(paths, adapter.name(), &release).await?;
    Ok(())
}

/// Turns what a person or an agent typed into the instance the engine knows.
/// Accepts `postgres`, `postgres@16`, or a name and version given separately.
/// A version pinned in config.toml counts, the same way it does when the
/// service is started, so `skep stop postgres` names what `skep serve` ran.
pub fn instance(service: &str, version: Option<&str>, paths: &Paths) -> Result<InstanceId> {
    // A branch or a target is named the way it prints: postgres:experiment,
    // cloudflared~myapp-test.
    let (service, tag) = Tag::split(service)?;
    let settings = project::settings(paths)?;
    let (adapter, version) = lookup(service, version, pinned(&settings, service))?;
    Ok(InstanceId {
        service: adapter.name().parse()?,
        version,
        tag,
    })
}

/// The spec for a service with nothing asked of it beyond skep's own settings.
pub fn spec_default(service: &str, version: Option<&str>, paths: &Paths) -> Result<ServiceSpec> {
    spec_for(service, version, &BTreeMap::new(), "", paths)
}

/// The spec for a service, layering what skep is configured to do under what
/// the caller asked for. A project file always wins over skep's own settings,
/// because the repository knows what it needs and the machine only has a
/// preference.
pub fn spec_for(
    service: &str,
    version: Option<&str>,
    ports: &BTreeMap<String, u16>,
    source: &str,
    paths: &Paths,
) -> Result<ServiceSpec> {
    let settings = project::settings(paths)?;
    let (name, _) = service.split_once('@').unwrap_or((service, ""));
    let configured = settings.services.get(name);
    let (adapter, version) = lookup(service, version, pinned(&settings, service))?;

    let mut chosen: BTreeMap<String, (u16, String)> = BTreeMap::new();
    if let Some(entry) = configured {
        if let Some(port) = entry.port {
            chosen.insert(
                main_port(service)?.to_string(),
                (port, project::SETTINGS.to_string()),
            );
        }
        for (port, number) in &entry.ports {
            chosen.insert(port.clone(), (*number, project::SETTINGS.to_string()));
        }
    }
    for (port, number) in ports {
        chosen.insert(port.clone(), (*number, source.to_string()));
    }

    let known: Vec<&str> = adapter.default_ports().iter().map(|(n, _)| *n).collect();
    let mut request = Request::new().with_version(version);
    for (port, (number, _)) in &chosen {
        if !known.contains(&port.as_str()) {
            return Err(Error::UnknownPort {
                service: adapter.name().to_string(),
                port: port.clone(),
                known: known.join(", "),
            });
        }
        request = request.with_port(port.clone(), *number);
    }

    // Annotated afterwards, in one place, so no adapter has to remember to.
    let mut spec = adapter.spec(&request, paths)?;
    for port in &mut spec.ports {
        if let Some((_, source)) = chosen.get(&port.name) {
            port.source = Some(source.clone());
        }
    }
    Ok(spec)
}

/// The version config.toml pins for a service, if it pins one.
fn pinned<'a>(settings: &'a project::Project, service: &str) -> Option<&'a str> {
    let (name, _) = service.split_once('@').unwrap_or((service, ""));
    settings
        .services
        .get(name)
        .and_then(|entry| entry.version.as_deref())
}

/// A version given separately beats one written inline as `postgres@16`,
/// which beats one pinned in config.toml, which beats the newest known.
fn lookup(
    service: &str,
    version: Option<&str>,
    configured: Option<&str>,
) -> Result<(&'static dyn ServiceAdapter, Version)> {
    let (name, inline) = match service.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (service, None),
    };
    let adapter = find(name).ok_or_else(|| Error::UnknownService {
        name: name.to_string(),
        known: names().join(", "),
    })?;
    let version = match version.or(inline).or(configured) {
        Some(text) => resolve(adapter, text)?,
        None => Version::new(adapter.default_version())?,
    };
    Ok((adapter, version))
}

/// Brings one service from a project file up, idempotently. Shared so the CLI
/// and the MCP cannot drift on what "already running" means or how a failure
/// is worded. The error is the sentence to show as it is.
pub async fn bring_up(
    client: &mut Client,
    name: &str,
    wanted: &project::Service,
    running: &[ServiceStatus],
    paths: &Paths,
) -> std::result::Result<(InstanceId, &'static str), String> {
    let mut ports = wanted.ports.clone();
    if let Some(port) = wanted.port {
        ports.insert(
            main_port(name).map_err(|e| e.to_string())?.to_string(),
            port,
        );
    }
    let spec = spec_for(
        name,
        wanted.version.as_deref(),
        &ports,
        project::FILE,
        paths,
    )
    .map_err(|error| error.to_string())?;
    let id = spec.id.clone();

    if let Some(status) = running.iter().find(|status| status.id == id) {
        if status.state.is_running() {
            return Ok((id, "already running"));
        }
        if status.state.is_transitional() {
            return Ok((id, "already starting"));
        }
    }

    for request in [
        Wire::Register {
            spec: Box::new(spec),
        },
        Wire::Start {
            instance: id.clone(),
        },
    ] {
        match client.send(&request).await {
            Ok(Response::Done) => {}
            Ok(Response::Failed { message }) => return Err(message),
            Ok(other) => return Err(format!("unexpected reply: {other:?}")),
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok((id, "started"))
}

/// A branch's spec: the same service on its own data and its own ports, which
/// is all a branch is. Ports are allocated here so a branch never has to be
/// told where to listen.
pub fn branch_spec(from: &InstanceId, label: &Label, paths: &Paths) -> Result<ServiceSpec> {
    let adapter = find(from.service.as_str()).ok_or_else(|| Error::UnknownService {
        name: from.service.to_string(),
        known: names().join(", "),
    })?;

    let mut request = Request::new()
        .with_version(from.version.clone())
        .with_tag(Tag::Branch(label.clone()));
    for (port, _) in adapter.default_ports() {
        let free = comb::free_port()
            .ok_or_else(|| Error::InvalidId("no free port for the branch".to_string()))?;
        request = request.with_port(*port, free);
    }

    let mut spec = adapter.spec(&request, paths)?;
    for port in &mut spec.ports {
        port.source = Some("branch".to_string());
    }
    Ok(spec)
}

/// The port a service is mainly known by, which the `port` shorthand sets.
pub fn main_port(service: &str) -> Result<&'static str> {
    let (adapter, _) = lookup(service, None, None)?;
    adapter
        .default_ports()
        .first()
        .map(|(name, _)| *name)
        .ok_or_else(|| Error::InvalidId(format!("{service} listens on no ports")))
}

pub fn names() -> Vec<&'static str> {
    catalog().iter().map(|adapter| adapter.name()).collect()
}

/// Every service comb ships with: the ones that stand on their own and are
/// registered, stopped, the moment a host comes up.
pub fn catalog() -> &'static [&'static dyn ServiceAdapter] {
    &[&Mailpit, &Mongodb, &Mysql, &Postgres, &Valkey]
}

/// The adapters that only make sense pointed at something, such as a tunnel.
/// Kept apart so the singleton path never has to know what a target is: a
/// host registers the catalog and nothing here.
pub fn targeted() -> &'static [&'static dyn ServiceAdapter] {
    &[&Cloudflared]
}

pub fn find(name: &str) -> Option<&'static dyn ServiceAdapter> {
    catalog()
        .iter()
        .chain(targeted())
        .copied()
        .find(|adapter| adapter.name() == name)
}

/// A tunnel's spec: cloudflared pointed at `origin`, named for what it serves,
/// on a metrics port of its own so two tunnels never collide.
pub fn share_spec(name: &Label, origin: Origin, paths: &Paths) -> Result<ServiceSpec> {
    let adapter: &dyn ServiceAdapter = &Cloudflared;
    let metrics = comb::free_port()
        .ok_or_else(|| Error::InvalidId("no free port for the tunnel".to_string()))?;
    let request = Request::new()
        .with_tag(Tag::Target(name.clone()))
        .with_origin(origin)
        .with_port("metrics", metrics);
    adapter.spec(&request, paths)
}

/// A project's own process, as an instance the engine can supervise.
///
/// The port is allocated here rather than written down anywhere, which is the
/// point of the whole arrangement: a dev server that picks its own port picks
/// a different one depending on what started first, and a hostname pinned to
/// a number is then pinned to whichever project won the race. Skep chooses,
/// tells the command through both the environment and the placeholder, and
/// serves the name at whatever it chose.
///
/// The instance is named for the project's directory, since that is what a
/// person calls it, and versioned `local` because it is not a release anybody
/// pinned.
pub fn run_spec(file: &Path, run: &project::Run, paths: &Paths) -> Result<(ServiceSpec, u16)> {
    let root = file.parent().unwrap_or(Path::new("."));
    let name = project_name(root)?;
    let port = comb::free_port()
        .ok_or_else(|| Error::InvalidId("no free port for the project".to_string()))?;

    let parts = run.command.parts(port);
    let (program, arguments) = parts
        .split_first()
        .ok_or_else(|| Error::InvalidId("the command in [run] is empty".to_string()))?;

    let id = InstanceId::new(name.as_str(), "local")?;
    let working = match &run.dir {
        Some(dir) => root.join(dir),
        None => root.to_path_buf(),
    };

    let mut spec = ServiceSpec::new(
        id,
        BinarySpec::path(program),
        // Nothing of the project's own is kept here; the engine wants
        // somewhere to put a service's data and this one has none.
        paths.data_dir(&InstanceId::new(name.as_str(), "local")?),
    )
    .with_display_name(name.as_str())
    .with_args(arguments.iter().cloned())
    .with_working_dir(working)
    // Both, so a tool that reads the environment needs no flag and a tool
    // that needs a flag has somewhere to put it.
    .with_env("PORT", port.to_string())
    .with_ports([Port::new("http", port)])
    // Listening is as much as skep can know: what a dev server serves is its
    // own business, and asking for a page would start compiling one.
    .with_health(HealthCheck::new(Probe::Tcp { port }));
    for (key, value) in &run.env {
        spec = spec.with_env(key, value);
    }
    Ok((spec, port))
}

/// What a project's file makes it called, which is what its directory is
/// called. The same answer `run_spec` uses, so a window and the engine agree
/// on the name without either guessing.
pub fn project_name_of(file: &Path) -> Result<comb::ServiceName> {
    project_name(file.parent().unwrap_or(Path::new(".")))
}

/// What to call a project: its directory, cut down to what a name may be.
fn project_name(root: &Path) -> Result<comb::ServiceName> {
    let raw = root
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        return Err(Error::InvalidId(format!(
            "{} is not a directory a project can be named after",
            root.display()
        )));
    }
    comb::ServiceName::new(trimmed)
}

/// The tunnel that serves a target, named the way its instance prints. A site
/// keeps its name with the dots turned to dashes, since a label has no dots.
pub fn tunnel_name(target: &str) -> Result<Label> {
    Label::new(target.replace('.', "-"))
}

/// What a tunnel for `target` should point at, worked out from what the
/// engine reports, and the sentence when it cannot be shared. A site is
/// shared through the proxy, forwarded headers and all; a service is shared
/// on its main port, which a quick tunnel carries as http only.
pub fn share_plan(
    target: &str,
    sites: &BTreeMap<String, u16>,
    running: &[ServiceStatus],
    https_port: u16,
    paths: &Paths,
) -> std::result::Result<(Label, Origin), String> {
    if target.contains('.') {
        let host = target.to_ascii_lowercase();
        let port = sites.get(&host).ok_or_else(|| {
            format!("{host} is not a site here; add it with: skep site add {host} <port>")
        })?;
        if std::net::TcpStream::connect(("127.0.0.1", *port)).is_err() {
            return Err(format!(
                "nothing is answering on port {port} behind {host}, so there is nothing to share yet"
            ));
        }
        let name = tunnel_name(&host).map_err(|error| error.to_string())?;
        return Ok((name, Origin::site(&host, https_port)));
    }
    let id = instance(target, None, paths).map_err(|error| error.to_string())?;
    let status = running
        .iter()
        .find(|status| status.id == id)
        .ok_or_else(|| format!("{id} is not registered here"))?;
    if !status.state.is_running() {
        return Err(format!("{id} is {}; start it first", status.state));
    }
    let main = main_port(target).map_err(|error| error.to_string())?;
    let port = status
        .ports
        .get(main)
        .ok_or_else(|| format!("{id} has no {main} port to share"))?;
    let name = tunnel_name(id.service.as_str()).map_err(|error| error.to_string())?;
    Ok((name, Origin::service(*port)))
}

/// Puts a target on a public url and waits for the url. Shared by the CLI and
/// the MCP so neither can drift on what is shareable or how a refusal is
/// worded. The error is the sentence to show as it is.
pub async fn share(
    client: &mut Client,
    target: &str,
    paths: &Paths,
) -> std::result::Result<(InstanceId, String), String> {
    let sites = match client.send(&Wire::Sites).await {
        Ok(Response::Sites { sites }) => sites,
        Ok(other) => return Err(format!("unexpected reply: {other:?}")),
        Err(error) => return Err(error.to_string()),
    };
    let running = match client.send(&Wire::Status).await {
        Ok(Response::Status { overview }) => overview.services,
        Ok(other) => return Err(format!("unexpected reply: {other:?}")),
        Err(error) => return Err(error.to_string()),
    };
    let https_port = comb::public_https_port(&comb::Layout::system(comb::SUFFIX).control).await;
    let (name, origin) = share_plan(target, &sites, &running, https_port, paths)?;

    let expected = InstanceId::target(
        Cloudflared.name(),
        Cloudflared.default_version(),
        name.as_str(),
    )
    .map_err(|error| error.to_string())?;
    if let Some(status) = running.iter().find(|status| status.id == expected)
        && status.state.is_running()
        && let Some(url) = &status.notice
    {
        return Ok((expected, url.clone()));
    }

    let spec = share_spec(&name, origin, paths).map_err(|error| error.to_string())?;
    let id = spec.id.clone();
    for request in [
        Wire::Register {
            spec: Box::new(spec),
        },
        Wire::Start {
            instance: id.clone(),
        },
    ] {
        match client.send(&request).await {
            Ok(Response::Done) => {}
            Ok(Response::Failed { message }) => return Err(message),
            Ok(other) => return Err(format!("unexpected reply: {other:?}")),
            Err(error) => return Err(error.to_string()),
        }
    }

    // The url lands on its own beat after the start returns: the edge says
    // it once a connection is registered, which is also what ready means.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let services = match client.send(&Wire::Status).await {
            Ok(Response::Status { overview }) => overview.services,
            Ok(other) => return Err(format!("unexpected reply: {other:?}")),
            Err(error) => return Err(error.to_string()),
        };
        if let Some(status) = services.iter().find(|status| status.id == id) {
            if let Some(url) = &status.notice {
                return Ok((id, url.clone()));
            }
            if let comb::ServiceState::Failed { reason } = &status.state {
                return Err(format!("the tunnel failed: {reason}"));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "{id} is up but the edge has not handed out a url yet"
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// The tunnel serving `target`, if one is registered, so it can be stopped.
pub fn shared_as(target: &str, running: &[ServiceStatus]) -> Option<InstanceId> {
    let name = tunnel_name(&target.to_ascii_lowercase()).ok()?;
    running
        .iter()
        .map(|status| &status.id)
        .find(|id| {
            id.service.as_str() == Cloudflared.name()
                && id.tag.as_ref().is_some_and(|tag| tag.name() == &name)
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway skep home with the given config.toml in it.
    fn home_with(label: &str, config: &str) -> Paths {
        let root =
            std::env::temp_dir().join(format!("skep-settings-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        if !config.is_empty() {
            std::fs::write(root.join(project::SETTINGS), config).unwrap();
        }
        Paths::new(root)
    }

    #[test]
    fn settings_move_a_port_and_say_so() {
        let paths = home_with("moved", "[services.postgres]\nport = 15432\n");

        let spec = spec_default("postgres", None, &paths).unwrap();

        assert_eq!(spec.primary_port(), Some(15432));
        assert_eq!(
            spec.ports[0].source.as_deref(),
            Some("config.toml"),
            "a chosen port should name the file that chose it"
        );
    }

    #[test]
    fn a_project_wins_over_skep_settings() {
        let paths = home_with("contested", "[services.postgres]\nport = 15432\n");
        let asked = BTreeMap::from([("postgres".to_string(), 25432)]);

        let spec = spec_for("postgres", None, &asked, "skep.toml", &paths).unwrap();

        // The repository knows what it needs; the machine only has a
        // preference.
        assert_eq!(spec.primary_port(), Some(25432));
        assert_eq!(spec.ports[0].source.as_deref(), Some("skep.toml"));
    }

    #[test]
    fn a_default_port_has_nothing_to_explain() {
        let paths = home_with("plain", "");

        let spec = spec_default("postgres", None, &paths).unwrap();

        assert_eq!(spec.primary_port(), Some(5432));
        assert_eq!(spec.ports[0].source, None);
    }

    #[test]
    fn settings_can_pin_a_version_and_a_project_can_override_it() {
        let paths = home_with("versioned", "[services.postgres]\nversion = \"16\"\n");

        assert_eq!(
            spec_default("postgres", None, &paths)
                .unwrap()
                .id
                .to_string(),
            "postgres@16.10.0"
        );
        assert_eq!(
            spec_for(
                "postgres",
                Some("15"),
                &BTreeMap::new(),
                "skep.toml",
                &paths
            )
            .unwrap()
            .id
            .to_string(),
            "postgres@15.14.0"
        );
    }

    #[test]
    fn a_pinned_version_names_the_instance_serve_would_run() {
        let paths = home_with("pinned", "[services.postgres]\nversion = \"16\"\n");

        let id = instance("postgres", None, &paths).unwrap();
        let spec = spec_default("postgres", None, &paths).unwrap();

        assert_eq!(id, spec.id, "stop must name what serve started");
        assert!(id.version.to_string().starts_with("16."), "{id}");
        // What is typed still beats what is pinned.
        let typed = instance("postgres@17", None, &paths).unwrap();
        assert!(typed.version.to_string().starts_with("17."), "{typed}");
        let typed = spec_default("postgres@17", None, &paths).unwrap();
        assert!(
            typed.id.version.to_string().starts_with("17."),
            "{}",
            typed.id
        );
    }

    #[test]
    fn a_port_the_service_does_not_have_is_refused() {
        let paths = home_with(
            "wrong-port",
            "[services.postgres]\nports = { gopher = 70 }\n",
        );

        let error = spec_default("postgres", None, &paths).unwrap_err();

        assert!(
            error.to_string().contains("no port named gopher"),
            "{error}"
        );
    }

    #[test]
    fn a_project_is_told_its_port_twice_over() {
        let paths = home_with("run", "");
        let file = std::env::temp_dir()
            .join(format!("skep-run-{}", std::process::id()))
            .join("my app")
            .join(project::FILE);
        let run = project::Run {
            command: project::Line::Whole("npm run dev -- --port {port} --strictPort".to_string()),
            dir: None,
            site: Some("myapp.test".to_string()),
            env: BTreeMap::from([("NODE_ENV".to_string(), "development".to_string())]),
        };

        let (spec, port) = run_spec(&file, &run, &paths).unwrap();

        // The directory names it, cut down to what a name may be.
        assert_eq!(spec.id.to_string(), "my-app@local");
        assert_eq!(spec.binary.resolve(&paths).to_string_lossy(), "npm");
        assert_eq!(
            spec.args,
            [
                "run",
                "dev",
                "--",
                "--port",
                &port.to_string(),
                "--strictPort"
            ]
        );
        // Both ways, so a tool needs no flag and a tool that does has one.
        assert_eq!(spec.env["PORT"], port.to_string());
        assert_eq!(spec.env["NODE_ENV"], "development");
        assert_eq!(spec.primary_port(), Some(port));
        assert_eq!(spec.health.probe, Probe::Tcp { port });
        assert!(spec.working_dir.as_ref().unwrap().ends_with("my app"));

        // Nothing is written down, so the next one is free to differ.
        let (_, again) = run_spec(&file, &run, &paths).unwrap();
        assert_ne!(port, again, "a project never reuses a port it was given");
    }

    #[test]
    fn a_command_written_as_a_list_keeps_its_spaces() {
        let paths = home_with("run-list", "");
        let file = std::env::temp_dir()
            .join("skep-run-list")
            .join(project::FILE);
        let run = project::Run {
            command: project::Line::Parts(vec![
                "cargo".to_string(),
                "run".to_string(),
                "--".to_string(),
                "--bind".to_string(),
                "127.0.0.1:{port}".to_string(),
            ]),
            dir: Some("server".to_string()),
            site: None,
            env: BTreeMap::new(),
        };

        let (spec, port) = run_spec(&file, &run, &paths).unwrap();

        assert_eq!(spec.args.last().unwrap(), &format!("127.0.0.1:{port}"));
        assert!(spec.working_dir.as_ref().unwrap().ends_with("server"));
    }

    #[test]
    fn two_tunnels_never_share_a_metrics_port() {
        let paths = home_with("tunnels", "");
        let one = share_spec(&Label::new("a").unwrap(), Origin::service(1), &paths).unwrap();
        let two = share_spec(&Label::new("b").unwrap(), Origin::service(2), &paths).unwrap();
        assert_ne!(one.primary_port(), two.primary_port());
        assert_eq!(one.id.to_string(), "cloudflared@2026.8.3~a");
        // A targeted adapter is findable but never in the catalog a host
        // registers on its own.
        assert!(find("cloudflared").is_some());
        assert!(!names().contains(&"cloudflared"));
    }

    /// The status of a service that is up on one port.
    fn up(id: &str, port_name: &str, port: u16) -> ServiceStatus {
        ServiceStatus {
            id: id.parse().unwrap(),
            display_name: id.to_string(),
            state: comb::ServiceState::Ready,
            ports: BTreeMap::from([(port_name.to_string(), port)]),
            ports_from: BTreeMap::new(),
            pid: Some(1),
            activity: None,
            blocked: None,
            notice: None,
            since: comb::Timestamp::from_millis(0),
        }
    }

    #[test]
    fn a_site_is_shared_through_the_proxy_carrying_its_name() {
        let paths = home_with("share-site", "");
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let sites = BTreeMap::from([("myapp.test".to_string(), port)]);

        let (name, origin) = share_plan("MyApp.test", &sites, &[], 8443, &paths).unwrap();

        // The label has no dots, and the origin is loopback with the name
        // carried alongside so the proxy can route it.
        assert_eq!(name.as_str(), "myapp-test");
        assert_eq!(origin, Origin::site("myapp.test", 8443));
    }

    #[test]
    fn a_site_with_nothing_behind_it_says_so_rather_than_tunnelling_to_nothing() {
        let paths = home_with("share-empty", "");
        let free = comb::free_port().unwrap();
        let sites = BTreeMap::from([("myapp.test".to_string(), free)]);

        let error = share_plan("myapp.test", &sites, &[], 8443, &paths).unwrap_err();

        assert!(error.contains("nothing is answering"), "{error}");
    }

    #[test]
    fn an_unknown_site_names_the_command_that_would_add_it() {
        let paths = home_with("share-unknown", "");
        let error = share_plan("nope.test", &BTreeMap::new(), &[], 8443, &paths).unwrap_err();
        assert!(error.contains("skep site add nope.test"), "{error}");
    }

    #[test]
    fn a_service_is_shared_on_its_main_port_and_must_be_running() {
        let paths = home_with("share-service", "");
        let running = [up("mailpit@1.31.0", "http", 8025)];

        let (name, origin) =
            share_plan("mailpit", &BTreeMap::new(), &running, 8443, &paths).unwrap();
        assert_eq!(name.as_str(), "mailpit");
        assert_eq!(origin, Origin::service(8025));

        let mut stopped = running.clone();
        stopped[0].state = comb::ServiceState::Stopped;
        let error = share_plan("mailpit", &BTreeMap::new(), &stopped, 8443, &paths).unwrap_err();
        assert!(error.contains("start it first"), "{error}");
    }

    #[test]
    fn the_tunnel_serving_a_target_is_found_by_the_name_it_was_given() {
        let mut tunnel = up("cloudflared@2026.8.3~myapp-test", "metrics", 20999);
        tunnel.notice = Some("https://x.trycloudflare.com".to_string());
        let running = [up("mailpit@1.31.0", "http", 8025), tunnel];

        let found = shared_as("MyApp.test", &running).unwrap();

        assert_eq!(found.to_string(), "cloudflared@2026.8.3~myapp-test");
        assert!(shared_as("postgres", &running).is_none());
    }

    #[test]
    fn the_catalog_is_consistent() {
        for adapter in catalog().iter().chain(targeted()) {
            assert_eq!(find(adapter.name()).map(|a| a.name()), Some(adapter.name()));
            assert!(!adapter.pins().is_empty(), "{} has no pins", adapter.name());
            // A default nobody pinned would fail only at install time.
            let default = Version::new(adapter.default_version()).unwrap();
            assert!(
                versions(*adapter).contains(&default),
                "{} defaults to an unpinned version",
                adapter.name()
            );
            // resolve() returns the first match, so newest first is load bearing.
            let ordered: Vec<Vec<u32>> = versions(*adapter)
                .iter()
                .map(|version| {
                    version
                        .as_str()
                        .split('.')
                        .map(|part| part.parse().unwrap_or(0))
                        .collect()
                })
                .collect();
            assert!(
                ordered.windows(2).all(|pair| pair[0] > pair[1]),
                "{} pins are not newest first: {ordered:?}",
                adapter.name()
            );

            for pin in adapter.pins() {
                assert!(
                    pin.url.starts_with("https://"),
                    "{} pin is not https",
                    adapter.name()
                );
                assert_eq!(
                    pin.sha256.len(),
                    64,
                    "{} pin has a short hash",
                    adapter.name()
                );
                assert!(pin.size > 0, "{} pin has no size", adapter.name());
            }
        }
    }

    #[test]
    fn an_unknown_version_is_refused_before_any_download() {
        let request = Request::new().with_version(Version::new("0.0.1").unwrap());

        let error = request.resolve_version(&Mailpit).unwrap_err();

        assert!(error.to_string().contains("known versions are"));
    }

    #[test]
    fn a_major_resolves_to_the_newest_pinned_patch() {
        assert_eq!(resolve(&Postgres, "17").unwrap().as_str(), "17.6.0");
        assert_eq!(resolve(&Postgres, "16").unwrap().as_str(), "16.10.0");
        assert_eq!(resolve(&Postgres, "16.10.0").unwrap().as_str(), "16.10.0");

        // Dot boundaries, so a major is never matched by a prefix.
        assert!(resolve(&Postgres, "1").is_err());
        assert_eq!(resolve(&Mailpit, "1").unwrap().as_str(), "1.31.0");

        let error = resolve(&Postgres, "14").unwrap_err().to_string();
        assert!(
            error.contains("known versions are 17.6.0, 16.10.0, 15.14.0"),
            "{error}"
        );
    }

    #[test]
    fn ports_fall_back_to_the_adapter_defaults() {
        let request = Request::new().with_port("smtp", 2025);

        assert_eq!(request.port(&Mailpit, "smtp").unwrap(), 2025);
        assert_eq!(request.port(&Mailpit, "http").unwrap(), 8025);
        assert!(request.port(&Mailpit, "gopher").is_err());
    }
}
