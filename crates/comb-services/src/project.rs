//! `skep.toml`: what a repository needs in order to run. Unknown keys are an
//! error, because a typoed `verison` that silently boots the wrong version is
//! worse than one that refuses to boot at all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use comb::{Error, Result};
use serde::Deserialize;

pub const FILE: &str = "skep.toml";
/// Skep's own settings, which a project file overrides.
pub const SETTINGS: &str = "config.toml";

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    #[serde(default)]
    pub services: BTreeMap<String, Service>,
    /// Hostnames this project wants, each pointing at a port it already runs
    /// something on. Skep gives the app a name and a certificate; it does not
    /// start the app.
    #[serde(default)]
    pub sites: BTreeMap<String, u16>,
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

/// The sites a project asks for, with settings underneath and the project
/// winning where both name the same host. Validated here so a hostname that
/// could never be issued a certificate fails at config time.
pub fn sites(settings: &Project, project: &Project) -> Result<comb::Sites> {
    let mut all = comb::Sites::new();
    for (host, port) in settings.sites.iter().chain(project.sites.iter()) {
        let host = comb::valid_hostname(host)?;
        all.insert(host.to_ascii_lowercase(), *port);
    }
    Ok(all)
}

/// Puts a site in a config file without disturbing the rest of it.
///
/// A config file belongs to the person who wrote it, so this goes through
/// toml_edit: comments, blank lines and spacing nobody would choose twice all
/// survive, and only the one table asked for changes.
pub fn add_site(path: &Path, host: &str, port: u16) -> Result<()> {
    let host = comb::valid_hostname(host)?;
    let mut document = read(path)?;
    sites_table(&mut document).insert(host, toml_edit::value(i64::from(port)));
    write(path, &document)
}

/// Takes one out again. Says whether it was there at all, so a frontend can
/// tell somebody the difference.
pub fn remove_site(path: &Path, host: &str) -> Result<bool> {
    let mut document = read(path)?;
    let had = sites_table(&mut document).remove(host).is_some();
    if had {
        write(path, &document)?;
    }
    Ok(had)
}

fn read(path: &Path) -> Result<toml_edit::DocumentMut> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.parse::<toml_edit::DocumentMut>()
        .map_err(|error| Error::InvalidId(error.to_string()))
}

fn write(path: &Path, document: &toml_edit::DocumentMut) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }
    std::fs::write(path, document.to_string()).map_err(Error::Io)
}

fn sites_table(document: &mut toml_edit::DocumentMut) -> &mut toml_edit::Table {
    if !document.contains_key("sites") {
        let mut fresh = toml_edit::Table::new();
        // Written as [sites] rather than folded onto one line, which is the
        // shape the settings template teaches.
        fresh.set_implicit(false);
        document.insert("sites", toml_edit::Item::Table(fresh));
    }
    document["sites"]
        .as_table_mut()
        .expect("sites is a table because it was just made one")
}

/// Walks up from `start`, so the command works from any subdirectory.
pub fn find(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|directory| {
        let candidate = directory.join(FILE);
        candidate.is_file().then_some(candidate)
    })
}

/// Writes a commented starting point if there is nothing there yet, and
/// returns the path either way. Editing beats inventing a form.
pub fn ensure_settings(paths: &comb::Paths) -> Result<PathBuf> {
    let path = paths.config_file();
    if path.is_file() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }
    std::fs::write(&path, TEMPLATE).map_err(Error::Io)?;
    Ok(path)
}

const TEMPLATE: &str = "\
# Skep's own settings. A project's skep.toml wins wherever both speak.
#
# Move a service that clashes with something already on this machine:
#
#   [services.postgres]
#   port = 15432
#
# Or pin the version skep should use:
#
#   [services.postgres]
#   version = \"16\"
#
# Services with more than one port name them:
#
#   [services.mailpit]
#   ports = { http = 8025, smtp = 1025 }
#
# Give an app you already run a hostname and real https. Skep does not start
# the app, only puts a name and a certificate in front of the port:
#
#   [sites]
#   \"myapp.test\" = 3000
#
# Names ending in .test resolve on their own. Any other name works too, but
# nothing routes it to this machine, so it needs your own /etc/hosts entry.
";

/// Skep's own settings. A machine with no settings has defaults, which is not
/// an error worth reporting.
pub fn settings(paths: &comb::Paths) -> Result<Project> {
    let path = paths.config_file();
    if path.is_file() {
        load(&path)
    } else {
        Ok(Project::default())
    }
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
