//! Built-in service adapters. An adapter describes a service and how to check
//! it; the engine owns spawning and supervision and never calls back in here.

mod mailpit;
mod postgres;
pub mod project;

use std::collections::BTreeMap;

use comb::{
    Client, Error, InstanceId, Label, Paths, Platform, Release, Request as Wire, Response, Result,
    ServiceSpec, ServiceStatus, Version,
};

pub use mailpit::Mailpit;
pub use postgres::Postgres;

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
    /// Everything the engine needs to supervise one instance of this service.
    fn spec(&self, request: &Request, paths: &Paths) -> Result<ServiceSpec>;
}

/// What a caller asked for. Anything left out falls back to the adapter.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub version: Option<Version>,
    pub label: Option<Label>,
    pub ports: BTreeMap<String, u16>,
}

impl Request {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_version(mut self, version: Version) -> Self {
        self.version = Some(version);
        self
    }

    pub fn with_label(mut self, label: Label) -> Self {
        self.label = Some(label);
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
            label: self.label.clone(),
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
pub fn instance(service: &str, version: Option<&str>) -> Result<InstanceId> {
    let (adapter, version) = lookup(service, version)?;
    Ok(InstanceId {
        service: adapter.name().parse()?,
        version,
        label: None,
    })
}

/// The spec for a service as a caller asked for it, ports included.
pub fn spec_for(
    service: &str,
    version: Option<&str>,
    ports: &BTreeMap<String, u16>,
    paths: &Paths,
) -> Result<ServiceSpec> {
    let (adapter, version) = lookup(service, version)?;
    let mut request = Request::new().with_version(version);
    for (name, number) in ports {
        let known: Vec<&str> = adapter.default_ports().iter().map(|(n, _)| *n).collect();
        if !known.contains(&name.as_str()) {
            return Err(Error::UnknownPort {
                service: adapter.name().to_string(),
                port: name.clone(),
                known: known.join(", "),
            });
        }
        request = request.with_port(name.clone(), *number);
    }
    adapter.spec(&request, paths)
}

fn lookup(service: &str, version: Option<&str>) -> Result<(&'static dyn ServiceAdapter, Version)> {
    let (name, inline) = match service.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (service, None),
    };
    let adapter = find(name).ok_or_else(|| Error::UnknownService {
        name: name.to_string(),
        known: names().join(", "),
    })?;
    let version = match version.or(inline) {
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
    let spec = spec_for(name, wanted.version.as_deref(), &ports, paths)
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

/// The port a service is mainly known by, which the `port` shorthand sets.
pub fn main_port(service: &str) -> Result<&'static str> {
    let (adapter, _) = lookup(service, None)?;
    adapter
        .default_ports()
        .first()
        .map(|(name, _)| *name)
        .ok_or_else(|| Error::InvalidId(format!("{service} listens on no ports")))
}

pub fn names() -> Vec<&'static str> {
    catalog().iter().map(|adapter| adapter.name()).collect()
}

/// Every service comb ships with.
pub fn catalog() -> &'static [&'static dyn ServiceAdapter] {
    &[&Mailpit, &Postgres]
}

pub fn find(name: &str) -> Option<&'static dyn ServiceAdapter> {
    catalog()
        .iter()
        .copied()
        .find(|adapter| adapter.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_is_consistent() {
        for adapter in catalog() {
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
