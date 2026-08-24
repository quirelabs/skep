//! Who is holding a port, and what to tell the user about it. The lookup is
//! platform specific; deciding what it means is not.

use std::path::Path;

/// A process listening on a port we wanted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Listener {
    pub(crate) pid: u32,
    pub(crate) command: String,
    /// Full path when we could get it, which is what makes the holder
    /// recognisable rather than merely named.
    pub(crate) executable: Option<String>,
}

enum Origin {
    Homebrew(String),
    Docker,
    Skep,
    Unknown,
}

/// The one place a port conflict is put into words, so the check before spawn
/// and the check after a bind failure cannot drift apart.
pub(crate) fn describe(port: u16, listener: &Listener) -> String {
    let origin = classify(listener);
    let tag = match &origin {
        Origin::Homebrew(_) => ", Homebrew",
        Origin::Docker => ", Docker",
        Origin::Skep => ", another skep",
        Origin::Unknown => "",
    };
    let advice = match &origin {
        Origin::Homebrew(formula) => {
            format!("Stop it with `brew services stop {formula}`, or change the port in skep.toml.")
        }
        Origin::Docker => {
            "Stop the container publishing that port (`docker ps`), or change the port in \
             skep.toml."
                .to_string()
        }
        Origin::Skep => {
            "Another skep host already has it. Stop that one, or change the port in skep.toml."
                .to_string()
        }
        Origin::Unknown => "Stop it, or change the port in skep.toml.".to_string(),
    };

    format!(
        "port {port} is held by {} (pid {}{tag}). {advice}",
        listener.command, listener.pid
    )
}

fn classify(listener: &Listener) -> Origin {
    let path = listener.executable.as_deref().unwrap_or_default();
    if let Some(formula) = homebrew_formula(path) {
        return Origin::Homebrew(formula);
    }
    if path.contains("/Docker") || listener.command.starts_with("com.docker") {
        return Origin::Docker;
    }
    let program = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&listener.command);
    if matches!(program, "skep" | "skep-app") {
        return Origin::Skep;
    }
    Origin::Unknown
}

/// `/opt/homebrew/Cellar/postgresql@17/17.6/bin/postgres` names a formula that
/// `brew services stop` will accept.
fn homebrew_formula(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix("/opt/homebrew/")
        .or_else(|| path.strip_prefix("/usr/local/"))?;
    let rest = rest
        .strip_prefix("Cellar/")
        .or_else(|| rest.strip_prefix("opt/"))?;
    Some(rest.split('/').next()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listener(command: &str, executable: Option<&str>) -> Listener {
        Listener {
            pid: 811,
            command: command.to_string(),
            executable: executable.map(ToString::to_string),
        }
    }

    #[test]
    fn a_homebrew_service_is_named_with_the_command_that_stops_it() {
        let cellar = listener(
            "postgres",
            Some("/opt/homebrew/Cellar/postgresql@17/17.6/bin/postgres"),
        );
        let message = describe(5432, &cellar);

        assert_eq!(
            message,
            "port 5432 is held by postgres (pid 811, Homebrew). \
             Stop it with `brew services stop postgresql@17`, or change the port in skep.toml."
        );

        // The linked path names the same formula.
        let linked = listener(
            "postgres",
            Some("/opt/homebrew/opt/postgresql@17/bin/postgres"),
        );
        assert!(describe(5432, &linked).contains("brew services stop postgresql@17"));
    }

    #[test]
    fn docker_and_another_skep_get_their_own_advice() {
        let docker = listener("com.docker.backend", Some("/Applications/Docker.app/x"));
        assert!(describe(5432, &docker).contains("docker ps"));

        let ours = listener("skep", Some("/usr/local/bin/skep"));
        assert!(describe(5432, &ours).contains("Another skep host"));
    }

    #[test]
    fn an_unrecognised_holder_is_still_named() {
        let stranger = listener("weird-daemon", None);
        let message = describe(1025, &stranger);

        assert!(message.starts_with("port 1025 is held by weird-daemon (pid 811)."));
        assert!(message.contains("change the port in skep.toml"));
    }

    #[test]
    fn paths_that_only_look_like_homebrew_are_not_claimed() {
        assert_eq!(homebrew_formula("/usr/bin/postgres"), None);
        assert_eq!(homebrew_formula("/opt/homebrew/bin/postgres"), None);
    }
}
