//! The engine behind Skep: it owns the service graph, the state of every
//! instance, and the event stream that every frontend renders.

mod acquire;
mod engine;
mod error;
mod event;
mod graph;
mod id;
mod logs;
mod paths;
mod platform;
mod probe;
mod serde_ms;
mod spec;
mod state;
mod time;

pub use acquire::{Release, ensure};
pub use engine::{Engine, ServiceStatus};
pub use error::{Error, Result};
pub use event::{Event, EventKind, LogLine, LogStream};
pub use id::{InstanceId, Label, ServiceName, Version};
pub use paths::Paths;
pub use platform::Platform;
pub use spec::{
    Backoff, BinarySpec, HealthCheck, Port, Probe, RestartPolicy, RestartSpec, ServiceSpec,
    ShutdownSpec,
};
pub use state::ServiceState;
pub use time::Timestamp;
