//! Built-in service adapters. An adapter describes a service and how to check
//! it; the engine owns spawning and supervision and never calls back in here.

mod mailpit;

use std::collections::BTreeMap;

use comb::{Error, InstanceId, Label, Paths, Platform, Release, Result, ServiceSpec, Version};

pub use mailpit::Mailpit;

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
            Err(Error::InvalidId(format!(
                "{} has no version {version}, known versions are {}",
                adapter.name(),
                versions(adapter)
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
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

/// Versions this adapter can install on this machine, in catalog order.
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

/// Every service comb ships with.
pub fn catalog() -> &'static [&'static dyn ServiceAdapter] {
    &[&Mailpit]
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
    fn ports_fall_back_to_the_adapter_defaults() {
        let request = Request::new().with_port("smtp", 2025);

        assert_eq!(request.port(&Mailpit, "smtp").unwrap(), 2025);
        assert_eq!(request.port(&Mailpit, "http").unwrap(), 8025);
        assert!(request.port(&Mailpit, "gopher").is_err());
    }
}
