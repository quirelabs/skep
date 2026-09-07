//! Putting `skep` and `skep-mcp` where a shell can find them.
//!
//! The window hosts the engine itself, so somebody who only ever opens the
//! app has a working skep already. What they do not have is the command, and
//! the command is how a project is brought up and how an agent is wired in.
//! Asking them to find the binaries inside an application is asking them to
//! know something they should not have to.
//!
//! Symlinks rather than copies, so the command and the window can never be
//! two different versions of skep.

use std::path::{Path, PathBuf};

/// What gets placed. The helper is deliberately not here: it belongs to
/// `skep domains install`, which puts it somewhere privileged and owns its
/// lifetime.
pub const TOOLS: [&str; 2] = ["skep", "skep-mcp"];

/// What a name on this machine currently is.
#[derive(Debug, PartialEq, Eq)]
pub enum Placed {
    /// Nothing of that name anywhere a shell would look.
    Missing,
    /// There, and it is this build.
    Ours(PathBuf),
    /// There, and it is something else. Never overwritten without being
    /// mentioned first: a name on PATH may not be ours to take.
    Other(PathBuf),
}

/// Where this application keeps its binaries: beside the one that is running,
/// which holds both for `cargo run` and inside a bundle.
pub fn beside() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// The places worth putting a command, best first. `/usr/local/bin` is on the
/// default PATH and needs no explaining; `~/.local/bin` needs no password.
pub fn candidates(home: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/local/bin"),
        home.join(".local").join("bin"),
    ]
}

/// The first candidate this process can actually write to, creating one under
/// the person's own home if that is what it takes. Nothing here escalates:
/// where a password would be needed, the window says the command instead.
pub fn writable(candidates: &[PathBuf], home: &Path) -> Option<PathBuf> {
    for place in candidates {
        if place.is_dir() {
            if writes_here(place) {
                return Some(place.clone());
            }
        } else if place.starts_with(home) && std::fs::create_dir_all(place).is_ok() {
            return Some(place.clone());
        }
    }
    None
}

/// Asked rather than assumed. Directory permissions on macOS are not a
/// question a mode bit answers on its own.
fn writes_here(place: &Path) -> bool {
    let probe = place.join(".skep-write-test");
    let allowed = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    allowed
}

/// What `name` resolves to for a shell with this PATH. Takes the variable
/// rather than reading it, so what it decides can be tested.
pub fn placed(name: &str, path: &str, ours: &Path) -> Placed {
    for dir in path.split(':').filter(|part| !part.is_empty()) {
        let found = Path::new(dir).join(name);
        if !found.exists() {
            continue;
        }
        // Through the symlink: the question is which binary runs, not which
        // file the shell touches first.
        let target = std::fs::canonicalize(&found).unwrap_or_else(|_| found.clone());
        let wanted = std::fs::canonicalize(ours).unwrap_or_else(|_| ours.to_path_buf());
        return if target == wanted {
            Placed::Ours(found)
        } else {
            Placed::Other(found)
        };
    }
    Placed::Missing
}

/// Links every tool from `from` into `into`, replacing a link that is already
/// ours. Returns what a person should be told, which is where they went.
pub fn install(from: &Path, into: &Path) -> std::io::Result<PathBuf> {
    for name in TOOLS {
        let source = from.join(name);
        if !source.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} is not beside the application", source.display()),
            ));
        }
        let link = into.join(name);
        // A dangling link still counts as existing, which is exactly the case
        // that needs clearing: it is a link to a build that has been deleted.
        if link.exists() || link.symlink_metadata().is_ok() {
            std::fs::remove_file(&link)?;
        }
        std::os::unix::fs::symlink(&source, &link)?;
    }
    Ok(into.to_path_buf())
}

/// Whether a directory is somewhere a shell would look, which decides whether
/// installing there is the end of the job or only most of it.
pub fn on_path(place: &Path, path: &str) -> bool {
    path.split(':').any(|part| Path::new(part) == place)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("skep-tools-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_name_nothing_answers_to_is_missing() {
        let root = scratch("missing");
        let ours = root.join("skep");
        std::fs::write(&ours, "").unwrap();

        assert_eq!(placed("skep", "", &ours), Placed::Missing);
        assert_eq!(
            placed("skep", "/nowhere:/also-nowhere", &ours),
            Placed::Missing
        );
    }

    #[test]
    fn a_link_to_this_build_is_ours_and_anything_else_is_not() {
        let root = scratch("resolve");
        let (from, into) = (root.join("app"), root.join("bin"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&into).unwrap();
        for name in TOOLS {
            std::fs::write(from.join(name), "").unwrap();
        }

        install(&from, &into).unwrap();

        let path = into.display().to_string();
        assert_eq!(
            placed("skep", &path, &from.join("skep")),
            Placed::Ours(into.join("skep"))
        );
        // The same name, a different binary: not ours to replace quietly.
        let stranger = root.join("elsewhere");
        std::fs::create_dir_all(&stranger).unwrap();
        std::fs::write(stranger.join("skep"), "").unwrap();
        assert_eq!(
            placed("skep", &path, &stranger.join("skep")),
            Placed::Other(into.join("skep"))
        );
    }

    #[test]
    fn installing_twice_is_the_same_as_installing_once() {
        let root = scratch("twice");
        let (from, into) = (root.join("app"), root.join("bin"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&into).unwrap();
        for name in TOOLS {
            std::fs::write(from.join(name), "").unwrap();
        }

        install(&from, &into).unwrap();
        install(&from, &into).expect("a second install replaces its own link");

        for name in TOOLS {
            assert!(into.join(name).symlink_metadata().unwrap().is_symlink());
        }
    }

    /// The case that breaks a plain create: the link is there, and what it
    /// pointed at is not.
    #[test]
    fn a_link_left_by_a_build_that_is_gone_is_replaced() {
        let root = scratch("dangling");
        let (from, into) = (root.join("app"), root.join("bin"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&into).unwrap();
        for name in TOOLS {
            std::fs::write(from.join(name), "").unwrap();
            std::os::unix::fs::symlink(root.join("deleted"), into.join(name)).unwrap();
        }

        install(&from, &into).unwrap();

        assert!(
            into.join("skep").exists(),
            "the link should point at a file"
        );
    }

    #[test]
    fn nothing_is_installed_from_a_folder_without_the_binaries() {
        let root = scratch("absent");
        let (from, into) = (root.join("app"), root.join("bin"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&into).unwrap();

        assert!(install(&from, &into).is_err());
    }

    #[test]
    fn a_place_under_home_is_made_rather_than_refused() {
        let home = scratch("home");
        let places = candidates(&home);

        let chosen = writable(&places, &home).unwrap();

        assert_eq!(chosen, home.join(".local").join("bin"));
        assert!(chosen.is_dir(), "it should have been created");
    }

    #[test]
    fn a_shell_only_finds_what_is_on_its_path() {
        assert!(on_path(Path::new("/usr/local/bin"), "/bin:/usr/local/bin"));
        assert!(!on_path(Path::new("/opt/skep/bin"), "/bin:/usr/local/bin"));
    }
}
