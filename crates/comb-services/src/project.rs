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
    /// The project's own process: the dev server skep runs and supervises,
    /// so the port it listens on is skep's to choose rather than yours to
    /// keep track of.
    pub run: Option<Run>,
    /// Where the projects skep has been asked to run live. Only ever read
    /// from config.toml, like every other preference: which projects this
    /// machine knows about is the machine's business, and a checkout that
    /// carried the list would be describing somebody else's disk.
    #[serde(default)]
    pub projects: Projects,
    /// How the app behaves. Only ever read from config.toml: a preference is
    /// the machine's, never a repository's, so a project file carrying one is
    /// ignored rather than obeyed.
    #[serde(default)]
    pub app: App,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct App {
    /// Whether clicking a site opens it in the browser rather than in the
    /// pane beside the list.
    #[serde(default)]
    pub sites_in_browser: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Projects {
    /// The directory each one lives in, holding its skep.toml.
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    /// What to start. A string is split on spaces; a list keeps arguments
    /// that have spaces in them. Not a shell: no pipes, no `&&`, no `$VAR`.
    /// Those belong in a script, where they can be read and tested.
    pub command: Line,
    /// Where to run it, relative to this file. The file's own directory by
    /// default, which is what a project usually means.
    pub dir: Option<String>,
    /// The hostname to serve it at. No port here on purpose: the port is
    /// whatever was free when it started, which is the whole point.
    pub site: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// A command, written either way round.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Line {
    Whole(String),
    Parts(Vec<String>),
}

impl Line {
    /// The command as it was written, for showing to somebody. The
    /// placeholder is left in it: what is worth reading is the shape of the
    /// command, and the port it happened to get is on the row already.
    pub fn shown(&self) -> String {
        match self {
            Self::Whole(line) => line.clone(),
            Self::Parts(parts) => parts.join(" "),
        }
    }

    /// The program and its arguments, with `{port}` filled in wherever it
    /// appears. Splitting on whitespace is a convenience for the common case;
    /// anything with a space inside an argument is written as a list.
    pub fn parts(&self, port: u16) -> Vec<String> {
        let filled = |text: &str| text.replace("{port}", &port.to_string());
        match self {
            Self::Whole(line) => line.split_whitespace().map(filled).collect(),
            Self::Parts(parts) => parts.iter().map(|part| filled(part)).collect(),
        }
    }
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

/// Remembers that a project exists, so a window that was never opened in its
/// directory can still show it. Says whether it was new, because telling
/// somebody about the first time is worth more than telling them every time.
pub fn remember_project(path: &Path, directory: &Path) -> Result<bool> {
    let directory = directory.display().to_string();
    let mut document = read(path)?;
    let paths = table(path, &mut document, "projects")?
        .entry("paths")
        .or_insert_with(|| toml_edit::value(toml_edit::Array::new()));
    let Some(list) = paths.as_array_mut() else {
        return Err(Error::Project {
            path: path.display().to_string(),
            message: "projects.paths is not a list".to_string(),
        });
    };
    if list.iter().any(|held| held.as_str() == Some(&directory)) {
        return Ok(false);
    }
    list.push(directory);
    write(path, &document)?;
    Ok(true)
}

/// Forgets one. The project itself is untouched: this is only the list of
/// what to show.
pub fn forget_project(path: &Path, directory: &str) -> Result<bool> {
    let mut document = read(path)?;
    let Some(list) = document
        .get_mut("projects")
        .and_then(|projects| projects.get_mut("paths"))
        .and_then(|paths| paths.as_array_mut())
    else {
        return Ok(false);
    };
    let before = list.len();
    list.retain(|held| held.as_str() != Some(directory));
    if list.len() == before {
        return Ok(false);
    }
    write(path, &document)?;
    Ok(true)
}

/// Writes one of the app's own preferences, leaving the rest of the file as
/// it was found. Only ever config.toml, for the reason on the field itself.
pub fn set_preference(path: &Path, name: &str, value: bool) -> Result<()> {
    let mut document = read(path)?;
    table(path, &mut document, "app")?.insert(name, toml_edit::value(value));
    write(path, &document)
}

/// Puts a site in a config file without disturbing the rest of it.
///
/// A config file belongs to the person who wrote it, so this goes through
/// toml_edit: comments, blank lines and spacing nobody would choose twice all
/// survive, and only the one table asked for changes.
pub fn add_site(path: &Path, host: &str, port: u16) -> Result<()> {
    let host = comb::valid_hostname(host)?.to_ascii_lowercase();
    let mut document = read(path)?;
    table(path, &mut document, "sites")?.insert(&host, toml_edit::value(i64::from(port)));
    write(path, &document)
}

/// Takes one out again. Says whether it was there at all, so a frontend can
/// tell somebody the difference.
pub fn remove_site(path: &Path, host: &str) -> Result<bool> {
    let mut document = read(path)?;
    let had = table(path, &mut document, "sites")?
        .remove(&host.to_ascii_lowercase())
        .is_some();
    if had {
        write(path, &document)?;
    }
    Ok(had)
}

fn read(path: &Path) -> Result<toml_edit::DocumentMut> {
    // Only a missing file is an empty one. Anything else unreadable must not
    // be silently replaced by what gets written back.
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(Error::Io(error)),
    };
    text.parse::<toml_edit::DocumentMut>()
        .map_err(|error| Error::Project {
            path: path.display().to_string(),
            message: error.to_string(),
        })
}

fn write(path: &Path, document: &toml_edit::DocumentMut) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }
    std::fs::write(path, document.to_string()).map_err(Error::Io)
}

/// Finds a table, adding it if it is not there yet.
///
/// Two things it gets right that a bare insert does not. A file that is only
/// comments, which is what the settings template is, keeps all of that as
/// trailing trivia, so a table inserted into one lands above the very
/// comments explaining the file; they are moved in front of the new table
/// instead. And `sites = { ... }` written on one line loads fine but is not a
/// table to edit in place, so that is a sentence rather than a panic, and
/// nobody's file is rewritten into a shape they did not pick.
fn table<'a>(
    path: &Path,
    document: &'a mut toml_edit::DocumentMut,
    name: &'static str,
) -> Result<&'a mut toml_edit::Table> {
    if !document.contains_key(name) {
        let mut fresh = toml_edit::Table::new();
        // A heading rather than one folded line, which is the shape the
        // settings template teaches.
        fresh.set_implicit(false);
        if document.iter().next().is_none() {
            let leading = document.trailing().as_str().unwrap_or_default().to_string();
            if !leading.trim().is_empty() {
                fresh.decor_mut().set_prefix(leading);
                document.set_trailing("");
            }
        }
        document.insert(name, toml_edit::Item::Table(fresh));
    }
    document[name].as_table_mut().ok_or_else(|| Error::Project {
        path: path.display().to_string(),
        message: format!("{name} is written inline; put it under a [{name}] heading to edit it"),
    })
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

/// Writes a commented starting point into a directory that has no skep.toml
/// yet, and returns the path either way. The same bargain the settings file
/// strikes: editing a file that explains itself beats inventing a form for
/// something with as many shapes as a dev command.
pub fn ensure_project(directory: &Path) -> Result<PathBuf> {
    let path = directory.join(FILE);
    if path.is_file() {
        return Ok(path);
    }
    std::fs::write(&path, PROJECT_TEMPLATE).map_err(Error::Io)?;
    Ok(path)
}

/// Writes what to run into a project's file, leaving everything else in it
/// alone. The file is created from the template first if there is none, so a
/// directory that has never been a project becomes one that explains itself.
pub fn write_run(directory: &Path, command: &str, site: Option<&str>) -> Result<PathBuf> {
    let path = ensure_project(directory)?;
    let mut document = read(&path)?;
    let run = table(&path, &mut document, "run")?;
    run.insert("command", toml_edit::value(command));
    match site {
        Some(site) if !site.trim().is_empty() => {
            run.insert("site", toml_edit::value(site.trim().to_ascii_lowercase()));
        }
        // Removed rather than left behind: a name nobody asked for would go
        // on being served.
        _ => {
            run.remove("site");
        }
    }
    write(&path, &document)?;
    Ok(path)
}

const PROJECT_TEMPLATE: &str = "\
# What skep should run for this project, and the name to serve it at.
#
# Uncomment and change the command. Skep picks a free port, sets PORT in the
# environment, and substitutes {port} anywhere it appears, so a tool that
# reads the environment needs no flag and a tool that needs a flag has
# somewhere to put it.
#
#   [run]
#   command = \"npm run dev -- --port {port} --strictPort\"
#   site = \"myapp.test\"
#
# There is no port to write down anywhere: whichever one it gets is the one
# the name points at, which is the whole reason this file exists.
#
# Services this project needs can go here too:
#
#   [services.postgres]
#   version = \"17\"
";

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

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("skep-sites-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("skep.toml")
    }

    #[test]
    fn a_site_is_kept_in_lowercase_and_found_in_any_case() {
        let path = scratch("case");
        add_site(&path, "Shop.test", 3000).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("\"shop.test\"")
        );
        assert!(remove_site(&path, "SHOP.TEST").unwrap());
    }

    #[test]
    fn an_inline_sites_table_is_a_sentence_rather_than_a_panic() {
        let path = scratch("inline");
        std::fs::write(&path, "sites = { \"a.test\" = 1 }\n").unwrap();
        let error = add_site(&path, "b.test", 2).unwrap_err().to_string();
        assert!(error.contains("inline"), "{error}");
        assert!(std::fs::read_to_string(&path).unwrap().contains("a.test"));
    }

    #[test]
    fn a_project_says_what_to_run_and_where_to_serve_it() {
        let project = parse(
            r#"
            [run]
            command = "npm run dev -- --port {port}"
            site = "myapp.test"
            env = { NODE_ENV = "development" }
            "#,
        )
        .unwrap();

        let run = project.run.expect("a run block");
        assert_eq!(run.site.as_deref(), Some("myapp.test"));
        assert_eq!(run.env["NODE_ENV"], "development");
        assert_eq!(
            run.command.parts(4321),
            ["npm", "run", "dev", "--", "--port", "4321"]
        );
    }

    #[test]
    fn a_port_written_nowhere_is_a_port_that_cannot_go_stale() {
        // The whole point: a site under [run] carries no number, so nothing
        // in the file can disagree with what the dev server actually got.
        let error = parse("[run]\ncommand = \"x\"\nsite = \"a.test\"\nport = 3000\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("port"), "{error}");
    }

    #[test]
    fn a_new_table_goes_under_the_comments_that_explain_the_file() {
        let path = scratch("headed");
        // What ensure_settings writes: a file that is entirely comments.
        std::fs::write(&path, "# Skep's own settings.\n#\n#   [sites]\n").unwrap();

        set_preference(&path, "sites_in_browser", true).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let heading = text.find("[app]").expect("the table was written");
        let comment = text
            .find("# Skep's own settings.")
            .expect("the comments are kept");
        assert!(
            comment < heading,
            "the file's own explanation belongs above what was added to it:\n{text}"
        );
    }

    #[test]
    fn a_folder_with_nothing_in_it_gets_a_file_that_explains_itself() {
        let directory = std::env::temp_dir().join(format!("skep-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();

        let path = ensure_project(&directory).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();

        // It parses, and says nothing to run: a project that was just added.
        let project = load(&path).unwrap();
        assert!(project.run.is_none());
        assert!(text.contains("[run]"), "the shape is shown: {text}");
        assert!(text.contains("{port}"), "and the placeholder is explained");

        // A file already there is somebody's, and is left alone.
        std::fs::write(&path, "[run]\ncommand = \"mine\"\n").unwrap();
        ensure_project(&directory).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[run]\ncommand = \"mine\"\n"
        );
    }

    #[test]
    fn writing_what_to_run_leaves_the_rest_of_the_file_alone() {
        let directory = std::env::temp_dir().join(format!("skep-run-w-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(FILE);
        std::fs::write(&path, "# mine\n[services.postgres]\nversion = \"17\"\n").unwrap();

        write_run(
            &directory,
            "npm run dev -- --port {port}",
            Some("MyApp.test"),
        )
        .unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# mine"), "{text}");
        let loaded = load(&path).unwrap();
        let run = loaded.run.expect("a run block");
        assert_eq!(run.site.as_deref(), Some("myapp.test"), "lowercased");
        assert_eq!(run.command.shown(), "npm run dev -- --port {port}");
        assert_eq!(loaded.services["postgres"].version.as_deref(), Some("17"));

        // A site taken away stops being served rather than lingering.
        write_run(&directory, "npm start", None).unwrap();
        let run = load(&path).unwrap().run.unwrap();
        assert!(run.site.is_none());
        assert_eq!(run.command.shown(), "npm start");
    }

    #[test]
    fn a_project_is_remembered_once_and_can_be_forgotten() {
        let path = scratch("projects");
        std::fs::write(&path, "# mine\n[sites]\n\"a.test\" = 1\n").unwrap();
        let directory = std::path::Path::new("/tmp/some/myapp");

        assert!(
            remember_project(&path, directory).unwrap(),
            "the first time"
        );
        assert!(
            !remember_project(&path, directory).unwrap(),
            "and not again"
        );

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# mine"), "{text}");
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.projects.paths, ["/tmp/some/myapp"]);
        assert_eq!(loaded.sites["a.test"], 1);

        assert!(forget_project(&path, "/tmp/some/myapp").unwrap());
        assert!(load(&path).unwrap().projects.paths.is_empty());
        assert!(
            !forget_project(&path, "/tmp/some/myapp").unwrap(),
            "forgetting what is not there is not a change"
        );
    }

    #[test]
    fn a_preference_survives_a_round_trip_and_leaves_the_file_alone() {
        let path = scratch("preference");
        std::fs::write(
            &path,
            "# my machine\n[sites]\n\"a.test\" = 3000  # the app\n",
        )
        .unwrap();

        set_preference(&path, "sites_in_browser", true).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my machine"), "{text}");
        assert!(
            text.contains("# the app"),
            "an inline comment is theirs too: {text}"
        );
        let loaded = load(&path).unwrap();
        assert!(loaded.app.sites_in_browser);
        assert_eq!(loaded.sites["a.test"], 3000);

        set_preference(&path, "sites_in_browser", false).unwrap();
        assert!(!load(&path).unwrap().app.sites_in_browser);
    }

    #[test]
    fn a_project_file_cannot_set_a_machine_preference() {
        // It parses, because the shape is the same file's shape. What stops a
        // repository from deciding is that only settings() reads it.
        let project = parse("[app]\nsites_in_browser = true\n").unwrap();
        assert!(project.app.sites_in_browser);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_replaced() {
        let path = scratch("unreadable");
        // A directory where the file should be reads as an error, not as empty.
        std::fs::create_dir_all(&path).unwrap();
        assert!(add_site(&path, "a.test", 1).is_err());
        assert!(path.is_dir(), "the write must not have happened");
    }
}
