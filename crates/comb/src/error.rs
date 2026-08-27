use crate::id::{InstanceId, Version};
use crate::state::ServiceState;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("invalid id: {0}")]
    InvalidId(String),

    #[error("could not issue a certificate: {0}")]
    Certificate(String),

    #[error("{host} is not a hostname a certificate can be issued for")]
    InvalidHost { host: String },

    #[error("unknown service {name}, try one of: {known}")]
    UnknownService { name: String, known: String },

    #[error("{service} has no version matching {requested}, known versions are {known}")]
    UnknownVersion {
        service: String,
        requested: String,
        known: String,
    },

    #[error("{service} has no port named {port}, it has: {known}")]
    UnknownPort {
        service: String,
        port: String,
        known: String,
    },

    #[error("{instance} already has a snapshot called {name}")]
    SnapshotExists { instance: InstanceId, name: String },

    #[error("{instance} has no snapshot called {name}")]
    NoSuchSnapshot { instance: InstanceId, name: String },

    #[error("could not copy {instance}: {reason}")]
    Snapshot {
        instance: InstanceId,
        reason: String,
    },

    #[error("{0} is not a branch, so there is nothing to delete")]
    NotABranch(InstanceId),

    #[error("no instance {0} is registered")]
    UnknownInstance(InstanceId),

    #[error("instance {0} is already registered")]
    AlreadyRegistered(InstanceId),

    #[error("{instance} cannot go from {from} to {to}")]
    IllegalTransition {
        instance: InstanceId,
        from: ServiceState,
        to: ServiceState,
    },

    #[error("{instance} is {state}, it must be stopped first")]
    NotStopped {
        instance: InstanceId,
        state: ServiceState,
    },

    #[error("cannot start {instance}: {program} did not spawn: {source}")]
    Spawn {
        instance: InstanceId,
        program: String,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot prepare the data directory {path} for {instance}: {source}")]
    DataDir {
        instance: InstanceId,
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{step} for {name} {version} failed: {reason}")]
    Acquire {
        step: &'static str,
        name: String,
        version: Version,
        reason: String,
    },

    #[error("{name} has to be built from source, but {problem}")]
    BuildTools { name: String, problem: String },

    #[error(
        "{name} {version} does not match its pinned checksum: expected {expected}, got {actual}"
    )]
    Checksum {
        name: String,
        version: Version,
        expected: String,
        actual: String,
    },

    #[error("{instance} depends on {missing}, which is not registered")]
    UnknownDependency {
        instance: InstanceId,
        missing: InstanceId,
    },

    #[error("dependency cycle: {0}")]
    DependencyCycle(String),

    #[error("{instance} was skipped because {dependency} failed")]
    DependencyFailed {
        instance: InstanceId,
        dependency: InstanceId,
    },

    #[error("{instance} could not {step}: {reason}")]
    Prepare {
        instance: InstanceId,
        step: String,
        reason: String,
    },

    #[error("in {path}: {message}")]
    Project { path: String, message: String },

    #[error("{message}")]
    PortTaken { port: u16, message: String },

    #[error("{instance} never became ready: {reason}")]
    NotReady {
        instance: InstanceId,
        reason: String,
    },

    #[error("{instance} died while starting: {reason}")]
    DiedStarting {
        instance: InstanceId,
        reason: String,
    },

    #[error("the log buffer for {0} was poisoned by a panic")]
    Poisoned(InstanceId),

    #[error("{0} is not implemented yet")]
    NotImplemented(&'static str),

    #[error("no skep engine is running. Start one with `skep serve`.")]
    NoHost,

    #[error("another process is already hosting the engine{}", match pid {
        Some(pid) => format!(" (pid {pid})"),
        None => String::new(),
    })]
    AlreadyHosted { pid: Option<u32> },

    #[error("{message}")]
    Protocol { message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Errors travel in every Result the engine returns, so they stay small.
    #[test]
    fn the_error_type_stays_compact() {
        assert!(
            size_of::<Error>() <= 128,
            "Error grew to {} bytes",
            size_of::<Error>()
        );
    }
}
