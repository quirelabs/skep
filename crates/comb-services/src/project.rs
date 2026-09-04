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
    /// Those belong in a script, where they can be read and tested. Leading
    /// `NAME=VALUE` pairs are the one borrowed convention, because a tool
    /// that takes its port from the environment has no other way to be told
    /// which port skep chose.
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

impl Run {
    /// Whether this project says which port to use anywhere it is allowed to.
    /// The environment counts: a tool that reads `PORT` is told through it,
    /// and `[run.env]` carries the placeholder like everything else does.
    pub fn names_the_port(&self) -> bool {
        self.command.names_the_port() || self.env.values().any(|value| value.contains(PLACEHOLDER))
    }
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

    /// Whether the command says anywhere at all which port to use. A project
    /// whose command does not is a project skep watches the wrong port for,
    /// and the only way to find that out is to wait for the start to time
    /// out, so it is worth asking before anything runs.
    pub fn names_the_port(&self) -> bool {
        self.shown().contains(PLACEHOLDER)
    }

    /// The command written the way it should have been, if this is an npm
    /// command whose flags npm will keep for itself.
    ///
    /// The one mistake worth knowing about by name. Everything else skep can
    /// only find out by running it and waiting, but this one is visible in
    /// the words: `npm run dev --port 54233` starts the dev server with no
    /// port at all, npm prints a warning nobody reads, and the server picks
    /// its own port while skep watches the one it chose. Only npm; pnpm, bun
    /// and yarn all pass what follows the script straight through.
    pub fn npm_needs_a_separator(&self) -> Option<String> {
        let words: Vec<String> = match self {
            Self::Whole(line) => line.split_whitespace().map(str::to_string).collect(),
            Self::Parts(parts) => parts.clone(),
        };
        // Whatever the leading assignments set is not the program.
        let start = words.iter().position(|word| assignment(word).is_none())?;
        let words = &words[start..];
        let [program, verb, rest @ ..] = words else {
            return None;
        };
        if program != "npm" {
            return None;
        }
        // `npm run` takes the script's name first; `npm start` and `npm test`
        // are the script.
        let named = match verb.as_str() {
            "run" | "run-script" => 1,
            "start" | "test" => 0,
            _ => return None,
        };
        let tail = rest.get(named..)?;
        if tail.iter().any(|word| word == "--") {
            return None;
        }
        if !tail.iter().any(|word| word.starts_with('-')) {
            return None;
        }
        let mut fixed: Vec<&str> = words[..2 + named].iter().map(String::as_str).collect();
        fixed.push("--");
        fixed.extend(tail.iter().map(String::as_str));
        Some(fixed.join(" "))
    }

    /// The environment the command sets, and the program it runs.
    ///
    /// Leading `NAME=VALUE` pairs become environment, the way they do in
    /// every shell. This is not a shell and never will be, but that one
    /// convention has to be here: a tool that reads its port out of the
    /// environment has no other way of being told which port skep chose, and
    /// `PORT={port} npm start` is how anybody would write it.
    pub fn split(&self, port: u16) -> (BTreeMap<String, String>, Vec<String>) {
        let mut environment = BTreeMap::new();
        let mut rest = Vec::new();
        for word in self.words(port) {
            // Only before the program. After it, an argument that happens to
            // contain an equals sign is an argument.
            if rest.is_empty()
                && let Some(assignment) = assignment(&word)
            {
                environment.insert(assignment.0, assignment.1);
                continue;
            }
            rest.push(word);
        }
        (environment, rest)
    }

    /// The program and its arguments, with `{port}` filled in wherever it
    /// appears and any leading assignments taken out.
    pub fn parts(&self, port: u16) -> Vec<String> {
        self.split(port).1
    }

    /// Every word of the command, with the placeholder filled in. Splitting on
    /// whitespace is a convenience for the common case; anything with a space
    /// inside an argument is written as a list.
    fn words(&self, port: u16) -> Vec<String> {
        let filled = |text: &str| text.replace(PLACEHOLDER, &port.to_string());
        match self {
            Self::Whole(line) => line.split_whitespace().map(filled).collect(),
            Self::Parts(parts) => parts.iter().map(|part| filled(part)).collect(),
        }
    }
}

/// What skep writes the port it chose into, wherever it appears.
pub const PLACEHOLDER: &str = "{port}";

/// What to tell somebody whose project never says which port to use. One
/// sentence in one place, because the window asks this question of a form and
/// the engine asks it of a file, and they must not word it differently.
pub const NEEDS_PORT: &str = "skep chooses the port, so the command has to say where it goes: \
                              write {port} in a flag, or in front as PORT={port} if the tool \
                              reads it from the environment";

/// The command to write down, given what somebody typed and the flag their
/// tool takes its port on.
///
/// Composed once, when the project is written down, rather than added to the
/// command every time it runs. A file that says `npm run dev` and quietly
/// runs something else is a file not worth reading, so what is written is
/// what runs, placeholder and all, and can be edited by hand afterwards.
///
/// Nothing here is guessed at: the flag is a thing somebody typed or left as
/// it came, and what this returns is shown to them before it is written.
pub fn with_port(command: &str, flag: &str) -> String {
    let command = command.trim();
    // Already said where the port goes. Nothing to add and nothing to argue
    // with: somebody who writes the placeholder has taken the decision.
    if command.contains(PLACEHOLDER) {
        return command.to_string();
    }
    // No flag means the tool takes its port from the environment, which is
    // what a leading assignment is for.
    let flag = flag.trim();
    if flag.is_empty() {
        return format!("PORT={PLACEHOLDER} {command}");
    }
    // `--port=` joins to its value; `--port` takes the next word.
    let joined = if flag.ends_with('=') {
        format!("{command} {flag}{PLACEHOLDER}")
    } else {
        format!("{command} {flag} {PLACEHOLDER}")
    };
    // And where that puts a flag npm would keep for itself, the check that
    // catches it by hand is the one that fixes it here.
    let line = Line::Whole(joined);
    line.npm_needs_a_separator().unwrap_or_else(|| line.shown())
}

/// The flag most tools take their port on, offered so nobody has to know
/// about the placeholder to add a project.
pub const USUAL_FLAG: &str = "--port";

/// What a folder looks like it needs, for a form to fill in.
#[derive(Debug, PartialEq, Eq)]
pub struct Guess {
    /// What to run, as somebody would have typed it.
    pub command: String,
    /// The flag its port goes on. Empty is a real answer: it means the tool
    /// reads `PORT` out of the environment, or that the command already
    /// carries the placeholder itself.
    pub flag: String,
}

/// What is in the folder, read rather than predicted.
///
/// The answer is written down in the project already: a package.json says
/// which script starts it, a lock file says which runner to say it with, and
/// a framework that has one command leaves a file only it leaves. Reading
/// those is exact, testable, and explains itself; a model asked the same
/// question would be none of the three, and would be guessing at something
/// that is not actually unknown.
///
/// What makes any of this safe is where the answer goes: into a form, before
/// anything is written, next to a line showing exactly what it composes to.
/// A folder nobody recognises gets nothing and the form asks, as it did
/// before. Nothing here is ever consulted at run time.
pub fn guess(directory: &Path) -> Option<Guess> {
    node(directory).or_else(|| conventional(directory))
}

/// The script somebody would run by hand, said with the runner whose lock
/// file is sitting there.
fn node(directory: &Path) -> Option<Guess> {
    let manifest = std::fs::read_to_string(directory.join("package.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&manifest).ok()?;
    let scripts = parsed.get("scripts")?.as_object()?;
    // In the order somebody would try them.
    let (name, body) = ["dev", "start", "serve"].into_iter().find_map(|name| {
        scripts
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(|body| (name, body))
    })?;
    Some(Guess {
        command: format!("{} run {name}", runner(directory)),
        // Create React App is the one common tool that takes no port flag at
        // all and reads the environment instead.
        flag: if body.contains("react-scripts") {
            String::new()
        } else {
            USUAL_FLAG.to_string()
        },
    })
}

/// Whichever package manager left its lock file behind, and where a project
/// has been through two of them, whichever wrote one last.
///
/// A repository that changed managers keeps both files, so having one proves
/// nothing on its own. Which was touched most recently is the closest thing
/// to an answer that is actually in the folder, and it is right for the two
/// repositories on this machine that have two.
fn runner(directory: &Path) -> &'static str {
    [
        ("bun.lock", "bun"),
        ("bun.lockb", "bun"),
        ("package-lock.json", "npm"),
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
    ]
    .into_iter()
    .filter_map(|(lock, runner)| {
        let touched = std::fs::metadata(directory.join(lock))
            .ok()?
            .modified()
            .ok()?;
        Some((touched, runner))
    })
    .max_by_key(|(touched, _)| *touched)
    .map(|(_, runner)| runner)
    .unwrap_or("npm")
}

/// Frameworks whose dev command is the same in every project that has one,
/// found by a file only they leave in a directory.
fn conventional(directory: &Path) -> Option<Guess> {
    if directory.join("artisan").is_file() {
        return Some(Guess {
            command: "php artisan serve".to_string(),
            flag: "--port=".to_string(),
        });
    }
    if directory.join("bin").join("rails").is_file() {
        return Some(Guess {
            command: "bin/rails server".to_string(),
            flag: "-p".to_string(),
        });
    }
    if directory.join("manage.py").is_file() {
        // Django takes its port as an argument rather than on a flag, so the
        // placeholder goes in the command and there is no flag to offer.
        return Some(Guess {
            command: format!("python3 manage.py runserver {PLACEHOLDER}"),
            flag: String::new(),
        });
    }
    None
}

/// The same, for the one command shape that names the port and still does not
/// pass it on.
pub const NEEDS_SEPARATOR: &str = "npm keeps flags for itself unless a bare -- comes first, so \
                                   the port would never reach the script. Write it as";

/// `NAME=VALUE`, split, if that is what this word is. The name has to look
/// like an environment variable or `--flag=value` would be swallowed.
fn assignment(word: &str) -> Option<(String, String)> {
    let (name, value) = word.split_once('=')?;
    let head = name.chars().next()?;
    if !(head.is_ascii_alphabetic() || head == '_') {
        return None;
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((name.to_string(), value.to_string()))
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
# Uncomment and change the command. Skep picks a free port and then waits for
# this project to answer on it, so the command has to say where that port
# goes. Write {port} wherever it belongs:
#
#   [run]
#   command = \"npm run dev -- --port {port} --strictPort\"
#   site = \"myapp.test\"
#
# A tool that reads its port from the environment is told the same way. A
# leading NAME=VALUE becomes environment, as it does in a shell:
#
#   [run]
#   command = \"PORT={port} npm start\"
#
# There is no port to write down anywhere: whichever one it gets is the one
# the name points at, which is the whole reason this file exists.
#
# For the rare tool that cannot be told its port at all, do not use [run]:
# start it yourself and give the port it insists on a name. Skep serves the
# name and its certificate and leaves the process alone.
#
#   [sites]
#   \"legacy.test\" = 3000
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
    fn a_leading_assignment_is_environment_rather_than_a_program() {
        let line = Line::Whole("PORT={port} npm start".to_string());

        let (environment, parts) = line.split(4321);

        assert_eq!(environment["PORT"], "4321");
        assert_eq!(parts, ["npm", "start"]);
    }

    #[test]
    fn an_equals_sign_after_the_program_is_an_argument() {
        let line = Line::Whole("npm run dev -- --port={port}".to_string());

        let (environment, parts) = line.split(4321);

        assert!(environment.is_empty(), "a flag is not an assignment");
        assert_eq!(parts, ["npm", "run", "dev", "--", "--port=4321"]);
    }

    /// A folder with the given files in it, each with the given contents.
    fn folder(label: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("skep-guess-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (name, body) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_node_project_is_read_out_of_its_own_manifest() {
        let root = folder(
            "node",
            &[(
                "package.json",
                r#"{"scripts":{"build":"vite build","dev":"vite dev"}}"#,
            )],
        );

        let guessed = guess(&root).unwrap();

        assert_eq!(guessed.command, "npm run dev");
        assert_eq!(guessed.flag, "--port");
        // And what it composes to is the command that actually works.
        assert_eq!(
            with_port(&guessed.command, &guessed.flag),
            "npm run dev -- --port {port}"
        );
    }

    #[test]
    fn the_runner_is_whichever_one_left_its_lock_file() {
        for (lock, expected) in [
            ("pnpm-lock.yaml", "pnpm run dev"),
            ("yarn.lock", "yarn run dev"),
            ("bun.lockb", "bun run dev"),
            ("package-lock.json", "npm run dev"),
        ] {
            let root = folder(
                lock,
                &[
                    ("package.json", r#"{"scripts":{"dev":"vite dev"}}"#),
                    (lock, ""),
                ],
            );
            assert_eq!(guess(&root).unwrap().command, expected);
        }
    }

    /// A real one: a repository here holds both a bun lock from August and an
    /// npm lock from a fortnight later, and npm is what actually runs it.
    #[test]
    fn a_project_that_changed_managers_is_read_as_the_one_it_changed_to() {
        let root = folder(
            "two-locks",
            &[
                ("package.json", r#"{"scripts":{"dev":"vite dev"}}"#),
                ("bun.lock", ""),
                ("package-lock.json", ""),
            ],
        );
        // Written at the same instant by the test, so the older one is aged
        // deliberately rather than by hoping the clock ticked between writes.
        let old = std::fs::File::options()
            .write(true)
            .open(root.join("bun.lock"))
            .unwrap();
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 60 * 24);
        old.set_times(std::fs::FileTimes::new().set_modified(long_ago))
            .unwrap();

        assert_eq!(guess(&root).unwrap().command, "npm run dev");
    }

    #[test]
    fn a_script_that_takes_no_port_flag_is_offered_none() {
        let root = folder(
            "cra",
            &[(
                "package.json",
                r#"{"scripts":{"start":"react-scripts start"}}"#,
            )],
        );

        let guessed = guess(&root).unwrap();

        assert_eq!(guessed.flag, "", "create react app reads PORT instead");
        assert_eq!(
            with_port(&guessed.command, &guessed.flag),
            "PORT={port} npm run start"
        );
    }

    #[test]
    fn a_framework_with_one_command_is_known_by_the_file_only_it_leaves() {
        let laravel = folder("laravel", &[("artisan", "")]);
        assert_eq!(
            guess(&laravel).unwrap(),
            Guess {
                command: "php artisan serve".to_string(),
                flag: "--port=".to_string(),
            }
        );

        let rails = folder("rails", &[("bin/rails", "")]);
        assert_eq!(guess(&rails).unwrap().command, "bin/rails server");

        // Django's port is an argument, so the placeholder is in the command
        // and composing leaves it exactly as it is.
        let django = folder("django", &[("manage.py", "")]);
        let guessed = guess(&django).unwrap();
        assert_eq!(guessed.command, "python3 manage.py runserver {port}");
        assert_eq!(with_port(&guessed.command, &guessed.flag), guessed.command);
    }

    #[test]
    fn a_folder_nobody_recognises_is_left_to_the_person() {
        assert_eq!(guess(&folder("bare", &[("README.md", "")])), None);
        // Including one whose manifest has no script worth starting.
        assert_eq!(
            guess(&folder(
                "no-dev",
                &[("package.json", r#"{"scripts":{"build":"tsc"}}"#)]
            )),
            None
        );
        // And one whose manifest is not valid json, which is not a panic.
        assert_eq!(
            guess(&folder("broken", &[("package.json", "{ oh dear")])),
            None
        );
    }

    #[test]
    fn the_flag_is_put_where_the_tool_will_actually_see_it() {
        // npm needs the separator, and gets it without anybody knowing.
        assert_eq!(
            with_port("npm run dev", "--port"),
            "npm run dev -- --port {port}"
        );
        // Everything else takes the flag as it stands.
        assert_eq!(with_port("vite dev", "--port"), "vite dev --port {port}");
        assert_eq!(with_port("pnpm dev", "--port"), "pnpm dev --port {port}");
        // Rails and its like.
        assert_eq!(
            with_port("bin/rails server", "-p"),
            "bin/rails server -p {port}"
        );
        // A flag that joins to its value.
        assert_eq!(
            with_port("php artisan serve", "--port="),
            "php artisan serve --port={port}"
        );
    }

    #[test]
    fn no_flag_means_the_tool_reads_it_from_the_environment() {
        assert_eq!(with_port("npm start", ""), "PORT={port} npm start");
        assert_eq!(with_port("npm start", "   "), "PORT={port} npm start");
    }

    #[test]
    fn a_command_that_already_says_where_the_port_goes_is_left_alone() {
        // Including the shapes no flag field could have produced, which is
        // the reason writing it yourself has to keep working.
        for command in [
            "npm run dev -- --port {port} --strictPort",
            "python -m http.server {port}",
            "PORT={port} npm start",
        ] {
            assert_eq!(with_port(command, "--port"), command);
        }
    }

    #[test]
    fn what_is_composed_is_something_the_engine_would_accept() {
        for (command, flag) in [
            ("npm run dev", "--port"),
            ("vite dev", "--port"),
            ("npm start", ""),
        ] {
            let written = Line::Whole(with_port(command, flag));
            assert!(written.names_the_port(), "{command} should name the port");
            assert_eq!(
                written.npm_needs_a_separator(),
                None,
                "{command} should need no fixing"
            );
        }
    }

    #[test]
    fn npm_keeping_the_flags_for_itself_is_caught_in_the_words() {
        // The exact command that sent a dev server to 5174 while skep watched
        // the port it had chosen.
        let line = Line::Whole("npm run dev --port {port}".to_string());

        assert_eq!(
            line.npm_needs_a_separator().as_deref(),
            Some("npm run dev -- --port {port}")
        );
    }

    #[test]
    fn the_separator_is_only_wanted_where_npm_would_eat_the_flag() {
        let fine = [
            // Already written correctly.
            "npm run dev -- --port {port}",
            // No flags for npm to take.
            "npm run dev",
            // Every other runner passes what follows the script straight on.
            "pnpm dev --port {port}",
            "bun run dev --port {port}",
            "yarn dev --port {port}",
            // Not a script runner at all.
            "vite dev --port {port}",
        ];
        for command in fine {
            assert_eq!(
                Line::Whole(command.to_string()).npm_needs_a_separator(),
                None,
                "{command} needs no separator"
            );
        }
    }

    #[test]
    fn npm_start_is_the_script_rather_than_taking_one() {
        assert_eq!(
            Line::Whole("npm start --port {port}".to_string())
                .npm_needs_a_separator()
                .as_deref(),
            Some("npm start -- --port {port}")
        );
    }

    #[test]
    fn the_environment_in_front_does_not_hide_the_program() {
        assert_eq!(
            Line::Whole("NODE_ENV=development npm run dev --port {port}".to_string())
                .npm_needs_a_separator()
                .as_deref(),
            Some("npm run dev -- --port {port}")
        );
    }

    #[test]
    fn a_command_says_whether_it_names_the_port() {
        assert!(Line::Whole("npm run dev -- --port {port}".to_string()).names_the_port());
        assert!(Line::Whole("PORT={port} npm start".to_string()).names_the_port());
        assert!(Line::Parts(vec!["npm".into(), "--port".into(), "{port}".into()]).names_the_port());
        assert!(!Line::Whole("npm run dev".to_string()).names_the_port());
        assert!(!Line::Whole("npm run dev -- --port 3000".to_string()).names_the_port());
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

    /// And the door for the one case that refusal shuts: a tool that cannot be
    /// told its port is not something skep runs, it is something skep serves.
    /// The number is allowed here because nothing else chose it.
    #[test]
    fn a_process_skep_does_not_start_may_name_its_own_port() {
        let project = parse("[sites]\n\"legacy.test\" = 3000\n").unwrap();

        assert_eq!(project.sites["legacy.test"], 3000);
        assert!(project.run.is_none(), "nothing here is skep's to start");
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
