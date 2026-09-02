use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::acquire::Release;
use crate::id::{InstanceId, Version};
use crate::paths::Paths;

/// Everything the engine needs to supervise one instance. Adapters produce
/// this; the engine reads it and never calls back into the adapter to spawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceSpec {
    pub id: InstanceId,
    pub display_name: String,
    pub binary: BinarySpec,
    /// Where to get the binary if it is not installed yet. The engine fetches
    /// it as a named phase, so a first start explains its own delay.
    #[serde(default)]
    pub release: Option<Release>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub working_dir: Option<PathBuf>,
    pub data_dir: PathBuf,
    #[serde(default)]
    pub ports: Vec<Port>,
    /// One-time work that must finish before the service itself runs, such as
    /// initdb. The engine runs these; adapters only describe them.
    #[serde(default)]
    pub prepare: Vec<PrepareStep>,
    /// Files inside the data directory that describe a running process rather
    /// than its data. A copied one convinces the next start that a server is
    /// already up, so they are removed from every copy.
    #[serde(default)]
    pub residue: Vec<String>,
    #[serde(default)]
    pub health: HealthCheck,
    #[serde(default)]
    pub depends_on: Vec<InstanceId>,
    #[serde(default)]
    pub restart: RestartSpec,
    #[serde(default)]
    pub shutdown: ShutdownSpec,
    /// The one thing this service says that is worth keeping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<Notice>,
}

impl ServiceSpec {
    pub fn new(id: InstanceId, binary: BinarySpec, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            display_name: id.service.to_string(),
            id,
            binary,
            release: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            working_dir: None,
            data_dir: data_dir.into(),
            ports: Vec::new(),
            prepare: Vec::new(),
            residue: Vec::new(),
            health: HealthCheck::default(),
            depends_on: Vec::new(),
            restart: RestartSpec::default(),
            shutdown: ShutdownSpec::default(),
            notice: None,
        }
    }

    /// The port frontends show in a service row.
    pub fn primary_port(&self) -> Option<u16> {
        self.ports.first().map(|port| port.number)
    }

    pub fn with_release(mut self, release: Release) -> Self {
        self.release = Some(release);
        self
    }

    pub fn with_display_name(mut self, name: impl Into<String>) -> Self {
        self.display_name = name.into();
        self
    }

    pub fn with_args<I: IntoIterator<Item = S>, S: Into<String>>(mut self, args: I) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn with_working_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    pub fn with_ports<I: IntoIterator<Item = Port>>(mut self, ports: I) -> Self {
        self.ports = ports.into_iter().collect();
        self
    }

    pub fn with_prepare<I: IntoIterator<Item = PrepareStep>>(mut self, steps: I) -> Self {
        self.prepare = steps.into_iter().collect();
        self
    }

    pub fn with_residue<I: IntoIterator<Item = S>, S: Into<String>>(mut self, files: I) -> Self {
        self.residue = files.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_health(mut self, health: HealthCheck) -> Self {
        self.health = health;
        self
    }

    pub fn with_depends_on<I: IntoIterator<Item = InstanceId>>(mut self, ids: I) -> Self {
        self.depends_on = ids.into_iter().collect();
        self
    }

    pub fn with_restart(mut self, restart: RestartSpec) -> Self {
        self.restart = restart;
        self
    }

    pub fn with_notice(mut self, notice: Notice) -> Self {
        self.notice = Some(notice);
        self
    }

    pub fn with_shutdown(mut self, shutdown: ShutdownSpec) -> Self {
        self.shutdown = shutdown;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum BinarySpec {
    /// Downloaded and pinned under the skep bin directory.
    Managed {
        name: String,
        version: Version,
        program: String,
    },
    /// An explicit path, for tests and for anyone bringing their own build.
    Path { path: PathBuf },
}

impl BinarySpec {
    pub fn managed(name: impl Into<String>, version: Version, program: impl Into<String>) -> Self {
        Self::Managed {
            name: name.into(),
            version,
            program: program.into(),
        }
    }

    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self::Path { path: path.into() }
    }

    pub fn resolve(&self, paths: &Paths) -> PathBuf {
        match self {
            Self::Managed {
                name,
                version,
                program,
            } => paths.binary_dir(name, version).join(program),
            Self::Path { path } => path.clone(),
        }
    }
}

/// A command the engine runs once, before the service starts. Named so the
/// event stream can say what is happening rather than showing a long silence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrepareStep {
    pub name: String,
    pub binary: BinarySpec,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Skipped when this path is already there, which is what makes a second
    /// start fast and a wiped data directory self healing.
    #[serde(default)]
    pub unless_exists: Option<PathBuf>,
    /// The directory this step builds. It is written to scratch and renamed
    /// here only on success, so the path existing proves the step finished
    /// rather than merely started. Write `{output}` in args and env for the
    /// scratch path.
    #[serde(default)]
    pub produces: Option<PathBuf>,
}

impl PrepareStep {
    pub fn new(name: impl Into<String>, binary: BinarySpec) -> Self {
        Self {
            name: name.into(),
            binary,
            args: Vec::new(),
            env: BTreeMap::new(),
            unless_exists: None,
            produces: None,
        }
    }

    pub fn with_args<I: IntoIterator<Item = S>, S: Into<String>>(mut self, args: I) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn unless_exists(mut self, path: impl Into<PathBuf>) -> Self {
        self.unless_exists = Some(path.into());
        self
    }

    pub fn produces(mut self, path: impl Into<PathBuf>) -> Self {
        self.produces = Some(path.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub name: String,
    pub number: u16,
    /// Which file asked for this number, when it was not the default. A port
    /// nobody chose has nothing to explain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Port {
    pub fn new(name: impl Into<String>, number: u16) -> Self {
        Self {
            name: name.into(),
            number,
            source: None,
        }
    }

    pub fn from(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }
}

/// Readiness is always measured, never slept through.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "probe", rename_all = "snake_case")]
pub enum Probe {
    /// Ready as soon as the process is alive. Only for services with no port.
    None,
    Tcp {
        port: u16,
    },
    Http {
        port: u16,
        path: String,
        expect: u16,
    },
    /// RESP PING, which covers Valkey and Redis alike.
    Resp {
        port: u16,
    },
    Postgres {
        port: u16,
        user: String,
        database: String,
    },
    /// MySQL speaks first, so readiness is its greeting rather than a bound
    /// port.
    Mysql {
        port: u16,
    },
    /// A real OP_MSG ping, so a mongod still starting up cannot pass.
    Mongo {
        port: u16,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthCheck {
    pub probe: Probe,
    #[serde(rename = "interval_ms", with = "crate::serde_ms")]
    pub interval: Duration,
    #[serde(rename = "timeout_ms", with = "crate::serde_ms")]
    pub timeout: Duration,
    /// How long an instance gets to become ready before starting counts as a
    /// failure. Polling is tight so that boot time tracks the service, not us.
    #[serde(rename = "startup_timeout_ms", with = "crate::serde_ms")]
    pub startup_timeout: Duration,
}

impl HealthCheck {
    /// Runs the check once. Public because the question a frontend or a test
    /// wants to ask is the same one the engine asks.
    pub async fn check(&self) -> std::result::Result<(), String> {
        crate::probe::check(&self.probe, self.timeout).await
    }

    pub fn new(probe: Probe) -> Self {
        Self {
            probe,
            ..Self::default()
        }
    }
}

impl Default for HealthCheck {
    fn default() -> Self {
        Self {
            probe: Probe::None,
            interval: Duration::from_millis(50),
            timeout: Duration::from_secs(1),
            startup_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartPolicy {
    Always,
    #[default]
    OnCrash,
    Never,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RestartSpec {
    #[serde(default)]
    pub policy: RestartPolicy,
    #[serde(default)]
    pub backoff: Backoff,
}

impl RestartSpec {
    pub fn new(policy: RestartPolicy) -> Self {
        Self {
            policy,
            backoff: Backoff::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Backoff {
    #[serde(rename = "initial_ms", with = "crate::serde_ms")]
    pub initial: Duration,
    #[serde(rename = "max_ms", with = "crate::serde_ms")]
    pub max: Duration,
    pub factor: f64,
    pub max_attempts: u32,
}

impl Backoff {
    /// Delay before the given attempt, counting from one.
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let steps = attempt.saturating_sub(1).min(32);
        let scaled = self.initial.as_secs_f64() * self.factor.powi(steps as i32);
        Duration::from_secs_f64(scaled.min(self.max.as_secs_f64()))
    }

    pub fn is_exhausted(&self, attempt: u32) -> bool {
        attempt > self.max_attempts
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_millis(200),
            max: Duration::from_secs(30),
            factor: 2.0,
            max_attempts: 5,
        }
    }
}

/// How to ask a service to stop. Which signal means "wind down now" is a
/// property of the service, not of the engine: Postgres reads SIGTERM as
/// "wait for every client to leave first", which is not what a dev tool wants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopSignal {
    #[default]
    Term,
    Int,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownSpec {
    #[serde(default)]
    pub signal: StopSignal,
    /// Time between that signal and SIGKILL.
    #[serde(rename = "grace_ms", with = "crate::serde_ms")]
    pub grace: Duration,
}

impl ShutdownSpec {
    pub fn new(signal: StopSignal, grace: Duration) -> Self {
        Self { signal, grace }
    }
}

impl Default for ShutdownSpec {
    fn default() -> Self {
        Self {
            signal: StopSignal::Term,
            grace: Duration::from_secs(10),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ServiceSpec {
        let id = InstanceId::new("valkey", "8").unwrap();
        ServiceSpec::new(id, BinarySpec::path("/usr/local/bin/valkey"), "/tmp/valkey")
            .with_ports([Port::new("resp", 6379)])
            .with_health(HealthCheck::new(Probe::Resp { port: 6379 }))
    }

    #[test]
    fn backoff_grows_and_then_holds_at_the_cap() {
        let backoff = Backoff::default();
        assert_eq!(backoff.delay_for(1), Duration::from_millis(200));
        assert_eq!(backoff.delay_for(2), Duration::from_millis(400));
        assert_eq!(backoff.delay_for(3), Duration::from_millis(800));
        assert_eq!(backoff.delay_for(30), backoff.max);
        assert!(!backoff.is_exhausted(5));
        assert!(backoff.is_exhausted(6));
    }

    #[test]
    fn defaults_are_the_conservative_ones() {
        let spec = spec();
        assert_eq!(spec.restart.policy, RestartPolicy::OnCrash);
        assert_eq!(spec.shutdown.grace, Duration::from_secs(10));
        assert_eq!(spec.primary_port(), Some(6379));
        assert_eq!(spec.display_name, "valkey");
    }

    #[test]
    fn durations_travel_as_milliseconds() {
        let json = serde_json::to_value(spec()).unwrap();
        assert_eq!(json["health"]["startup_timeout_ms"], 30_000);
        assert_eq!(json["shutdown"]["grace_ms"], 10_000);
        assert_eq!(json["id"], "valkey@8");

        let back: ServiceSpec = serde_json::from_value(json).unwrap();
        assert_eq!(back, spec());
    }
}

/// Something a service announces once it is up that a person needs, such as
/// the public url a tunnel was given. Declared rather than hooked: the engine
/// reads the output and the adapter only says what to look for, so the text
/// rides the event stream and every frontend shows it without special casing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    /// A line containing this is the one. The notice is the word of that line
    /// containing it, so `trycloudflare.com` picks a whole url out of whatever
    /// box cloudflared draws around it.
    pub marker: String,
}

impl Notice {
    pub fn new(marker: impl Into<String>) -> Self {
        Self {
            marker: marker.into(),
        }
    }

    pub fn find(&self, line: &str) -> Option<String> {
        line.split(|c: char| c.is_whitespace() || c == '|')
            .map(|word| {
                word.trim_matches(|c: char| {
                    matches!(c, '"' | '\'' | ',' | ';' | '.' | '(' | ')' | '<' | '>')
                })
            })
            .find(|word| word.contains(self.marker.as_str()))
            .map(str::to_string)
    }
}

#[cfg(test)]
mod notice_tests {
    use super::Notice;

    #[test]
    fn the_word_carrying_the_marker_is_lifted_out_of_its_decoration() {
        let notice = Notice::new("trycloudflare.com");
        let line = "2025-09-02T10:00:00Z INF |  https://tidy-apple-1234.trycloudflare.com  |";
        assert_eq!(
            notice.find(line).as_deref(),
            Some("https://tidy-apple-1234.trycloudflare.com")
        );
    }

    #[test]
    fn a_mention_of_the_host_is_not_the_url() {
        let notice = Notice::new(".trycloudflare.com");
        assert_eq!(
            notice.find("INF Requesting new quick Tunnel on trycloudflare.com..."),
            None
        );
    }

    #[test]
    fn a_line_without_the_marker_is_not_a_notice() {
        assert_eq!(
            Notice::new("trycloudflare.com").find("INF Starting tunnel"),
            None
        );
    }
}
