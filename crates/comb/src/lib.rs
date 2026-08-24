//! The engine behind Skep: it owns the service graph, the state of every
//! instance, and the event stream that every frontend renders.

mod acquire;
mod engine;
mod error;
mod event;
mod graph;
mod host;
mod id;
mod logs;
mod paths;
mod platform;
mod ports;
mod probe;
mod scratch;
mod serde_ms;
mod spec;
mod state;
mod time;

pub use acquire::{Release, ensure};
pub use engine::{Engine, ServiceStatus};
pub use error::{Error, Result};
pub use event::{Event, EventKind, LogLine, LogStream};
pub use host::{Client, Host, Lock, PROTOCOL, Request, Response};
pub use id::{InstanceId, Label, ServiceName, Version};
pub use paths::Paths;
pub use platform::Platform;
pub use spec::{
    Backoff, BinarySpec, HealthCheck, Port, PrepareStep, Probe, RestartPolicy, RestartSpec,
    ServiceSpec, ShutdownSpec, StopSignal,
};
pub use state::ServiceState;
pub use time::Timestamp;
