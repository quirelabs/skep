//! `skep.toml`: what a repository needs in order to run. Unknown keys are an
//! error, because a typoed `verison` that silently boots the wrong version is
//! worse than one that refuses to boot at all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use comb::{Error, Result};
use serde::Deserialize;

pub const FILE: &str = "skep.toml";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    #[serde(default)]
    pub services: BTreeMap<String, Service>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    /// A major like "17" resolves to the newest pinned patch of it.
    pub version: Option<String>,
    /// Shorthand for the service's main port.
    pub port: Option<u16>,
    /// For services that listen on more than one, by name.
    #[serde(default)]
    pub ports: BTreeMap<String, u16>,
}

/// Walks up from `start`, so the command works from any subdirectory.
pub fn find(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|directory| {
        let candidate = directory.join(FILE);
        candidate.is_file().then_some(candidate)
    })
}

pub fn load(path: &Path) -> Result<Project> {
    let text = std::fs::read_to_string(path).map_err(Error::Io)?;
    // The parse error names the offending key and its position, so it is
    // repeated verbatim rather than summarised.
    toml::from_str(&text).map_err(|error| Error::Project {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> std::result::Result<Project, toml::de::Error> {
        toml::from_str(text)
    }

    #[test]
    fn a_project_lists_services_versions_and_ports() {
        let project = parse(
            r#"
            [services.postgres]
            version = "17"
            port = 5432

            [services.mailpit]
            ports = { http = 8025, smtp = 1025 }
            "#,
        )
        .unwrap();

        let postgres = &project.services["postgres"];
        assert_eq!(postgres.version.as_deref(), Some("17"));
        assert_eq!(postgres.port, Some(5432));
        assert_eq!(project.services["mailpit"].ports["smtp"], 1025);
        assert_eq!(project.services["mailpit"].version, None);
    }

    #[test]
    fn a_typo_names_the_key_it_did_not_recognise() {
        let error = parse(
            r#"
            [services.postgres]
            verison = "17"
            "#,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("verison"), "{error}");
        assert!(
            error.contains("version"),
            "it should list the real keys: {error}"
        );
    }

    #[test]
    fn a_stray_top_level_key_is_refused_too() {
        let error = parse("servcies = {}").unwrap_err().to_string();
        assert!(error.contains("servcies"), "{error}");
    }

    #[test]
    fn an_empty_file_is_a_project_with_nothing_in_it() {
        assert!(parse("").unwrap().services.is_empty());
    }
}
