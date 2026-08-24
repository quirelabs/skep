use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::id::InstanceId;
use crate::state::ServiceState;
use crate::time::Timestamp;

/// One entry in the engine's event stream. This stream is the only thing
/// frontends render, so anything a user should see has to become an event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Monotonic within one engine run. Consumers that fall behind a broadcast
    /// channel can see the gap rather than silently missing transitions.
    pub seq: u64,
    pub at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<InstanceId>,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventKind {
    Registered,
    Deregistered,
    StateChanged {
        from: ServiceState,
        to: ServiceState,
    },
    ProbeSucceeded {
        #[serde(rename = "after_ms", with = "crate::serde_ms")]
        after: Duration,
    },
    ProbeFailed {
        reason: String,
    },
    RestartScheduled {
        attempt: u32,
        #[serde(rename = "delay_ms", with = "crate::serde_ms")]
        delay: Duration,
    },
    PortConflict {
        port: u16,
        pid: Option<u32>,
        process: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    pub at: Timestamp,
    pub stream: LogStream,
    pub text: String,
}

impl LogLine {
    pub fn new(stream: LogStream, text: impl Into<String>) -> Self {
        Self {
            at: Timestamp::now(),
            stream,
            text: text.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialise_flat() {
        let event = Event {
            seq: 7,
            at: Timestamp::from_millis(1_700_000_000_000),
            instance: Some("postgres@16".parse().unwrap()),
            kind: EventKind::StateChanged {
                from: ServiceState::Starting,
                to: ServiceState::Ready,
            },
        };

        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"seq":7,"at":1700000000000,"instance":"postgres@16","event":"state_changed","from":{"state":"starting"},"to":{"state":"ready"}}"#
        );
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
    }

    #[test]
    fn engine_level_events_carry_no_instance() {
        let event = Event {
            seq: 1,
            at: Timestamp::from_millis(0),
            instance: None,
            kind: EventKind::Registered,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("instance"));
    }
}
