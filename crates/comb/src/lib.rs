//! The engine behind Skep: it owns the service graph, the state of every
//! instance, and the event stream that every frontend renders.

mod acquire;
mod book;
#[cfg(feature = "sites")]
mod certs;
#[cfg(feature = "sites")]
mod dns;
#[cfg(feature = "sites")]
mod domains;
mod engine;
mod error;
mod event;
mod graph;
mod host;
mod id;
mod logs;
mod mirror;
mod paths;
mod platform;
mod ports;
mod probe;
#[cfg(feature = "sites")]
mod proxy;
mod scratch;
mod serde_ms;
mod snapshot;
mod spec;
mod state;
mod time;

pub use acquire::{Build, Release, ensure};
pub use book::{HTTP_PORT, HTTPS_PORT, Sites, port_suffix, site_url};
#[cfg(feature = "sites")]
pub use certs::{Authority, Issued, valid_hostname};
#[cfg(feature = "sites")]
pub use dns::{PORT as DNS_PORT, Routing, SUFFIX, reply as dns_reply, routing, serve as serve_dns};
#[cfg(feature = "sites")]
pub use domains::{
    Foreign, Forward, HELPER_PROTOCOL, Health, Hello, Layout, Owner, Serving, activate,
    become_user, deactivate, foreign, hand_over, health, invoking_user, is_root, place,
    public_https_port, remove, serve_alongside,
};
pub use engine::{Engine, Overview, ServiceStatus, Snapshot};
pub use error::{Error, Result};
pub use event::{Event, EventKind, LogLine, LogStream};
pub use host::{Client, Host, Lock, PROTOCOL, Request, Response};
pub use id::{InstanceId, Label, ServiceName, Tag, Version};
pub use mirror::{Applied, Glyph, Mirror, Summary};
pub use paths::Paths;
pub use platform::Platform;
pub use ports::free_port;
#[cfg(feature = "sites")]
pub use proxy::{redirect, serve as serve_sites};
pub use spec::{
    Backoff, BinarySpec, HealthCheck, Notice, Port, PrepareStep, Probe, RestartPolicy, RestartSpec,
    ServiceSpec, ShutdownSpec, StopSignal,
};
pub use state::ServiceState;
pub use time::Timestamp;
