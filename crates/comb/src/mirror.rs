//! A replica of engine state built from the event stream. Frontends render
//! this rather than polling, and it says when it has fallen behind instead of
//! guessing its way forward.

use std::collections::BTreeMap;

use crate::engine::{Overview, ServiceStatus};
use crate::event::{Event, EventKind};
use crate::id::InstanceId;

/// What applying an event asked of the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// The replica moved forward.
    Moved,
    /// Already covered by the snapshot this replica was built from.
    Ignored,
    /// Events were missed, or something appeared that this replica knows
    /// nothing about. Fetch a snapshot; do not guess.
    Resync,
}

#[derive(Clone, Debug, Default)]
pub struct Mirror {
    services: BTreeMap<InstanceId, ServiceStatus>,
    seq: u64,
    seeded: bool,
}

impl Mirror {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces everything. The snapshot's stamp is what makes it possible to
    /// tell which buffered events it already accounts for.
    pub fn reset(&mut self, overview: Overview) {
        self.services = overview
            .services
            .into_iter()
            .map(|status| (status.id.clone(), status))
            .collect();
        self.seq = overview.seq;
        self.seeded = true;
    }

    pub fn apply(&mut self, event: &Event) -> Applied {
        if !self.seeded {
            return Applied::Resync;
        }
        if event.seq <= self.seq {
            return Applied::Ignored;
        }
        if event.seq != self.seq + 1 {
            // Leave the sequence where it is: the replica is still correct as
            // of the last event it actually saw.
            return Applied::Resync;
        }

        match (&event.instance, &event.kind) {
            // A new instance arrives with no ports and no name of its own, so
            // there is nothing honest to render until a snapshot describes it.
            (_, EventKind::Registered) => return Applied::Resync,
            (Some(id), EventKind::Deregistered) => {
                self.services.remove(id);
            }
            (Some(id), EventKind::StateChanged { to, .. }) => {
                let Some(service) = self.services.get_mut(id) else {
                    return Applied::Resync;
                };
                service.state = to.clone();
                service.since = event.at;
                service.activity = None;
                service.blocked = None;
                // The stream carries no pid, so claiming one would be a guess.
                service.pid = None;
            }
            (Some(id), EventKind::Notice { text }) => {
                if let Some(service) = self.services.get_mut(id) {
                    service.notice = text.clone();
                }
            }
            (Some(id), EventKind::Preparing { step } | EventKind::Progress { step }) => {
                if let Some(service) = self.services.get_mut(id) {
                    service.activity = Some(step.clone());
                }
            }
            (Some(id), EventKind::Blocked { by }) => {
                if let Some(service) = self.services.get_mut(id) {
                    service.blocked = by.clone();
                }
            }
            (Some(id), EventKind::Prepared { .. }) => {
                if let Some(service) = self.services.get_mut(id) {
                    service.activity = None;
                }
            }
            _ => {}
        }

        self.seq = event.seq;
        Applied::Moved
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn services(&self) -> impl Iterator<Item = &ServiceStatus> {
        self.services.values()
    }

    pub fn get(&self, id: &InstanceId) -> Option<&ServiceStatus> {
        self.services.get(id)
    }

    pub fn summary(&self) -> Summary {
        let mut summary = Summary::default();
        for service in self.services.values() {
            summary.total += 1;
            if service.state.is_running() {
                summary.running += 1;
            }
            if service.state.is_transitional() || service.activity.is_some() {
                summary.working += 1;
            }
            if matches!(service.state, crate::ServiceState::Failed { .. }) {
                summary.failed += 1;
            }
        }
        summary
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub total: usize,
    pub running: usize,
    pub working: usize,
    pub failed: usize,
}

/// What the menubar should say at a glance. Colour is the frontend's business;
/// which of these applies is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Idle,
    Running(usize),
    Working,
    Failed,
}

impl Summary {
    /// Trouble outranks motion, motion outranks steady state. Anything else
    /// would let a failure hide behind a service that happens to be starting.
    pub fn glyph(&self) -> Glyph {
        if self.failed > 0 {
            Glyph::Failed
        } else if self.working > 0 {
            Glyph::Working
        } else if self.running > 0 {
            Glyph::Running(self.running)
        } else {
            Glyph::Idle
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::state::ServiceState;
    use crate::time::Timestamp;

    fn status(id: &str, state: ServiceState) -> ServiceStatus {
        ServiceStatus {
            id: id.parse().unwrap(),
            display_name: id.to_string(),
            state,
            ports: BTreeMap::new(),
            ports_from: BTreeMap::new(),
            pid: Some(42),
            activity: None,
            blocked: None,
            notice: None,
            since: Timestamp::from_millis(0),
        }
    }

    fn event(seq: u64, id: &str, kind: EventKind) -> Event {
        Event {
            seq,
            at: Timestamp::from_millis(0),
            instance: Some(id.parse().unwrap()),
            kind,
        }
    }

    fn changed(to: ServiceState) -> EventKind {
        EventKind::StateChanged {
            from: ServiceState::Stopped,
            to,
        }
    }

    fn seeded() -> Mirror {
        let mut mirror = Mirror::new();
        mirror.reset(Overview {
            seq: 10,
            services: vec![status("valkey@9", ServiceState::Stopped)],
        });
        mirror
    }

    #[test]
    fn an_unseeded_replica_asks_for_a_snapshot() {
        let mut mirror = Mirror::new();
        assert_eq!(
            mirror.apply(&event(1, "valkey@9", changed(ServiceState::Starting))),
            Applied::Resync
        );
    }

    #[test]
    fn events_the_snapshot_already_covers_are_ignored() {
        let mut mirror = seeded();

        assert_eq!(
            mirror.apply(&event(10, "valkey@9", changed(ServiceState::Ready))),
            Applied::Ignored
        );
        assert_eq!(
            mirror.get(&"valkey@9".parse().unwrap()).unwrap().state,
            ServiceState::Stopped,
            "a stale event must not move the replica"
        );
    }

    #[test]
    fn a_gap_asks_for_a_snapshot_and_changes_nothing() {
        let mut mirror = seeded();

        // 11 is next; 13 means two were missed.
        assert_eq!(
            mirror.apply(&event(13, "valkey@9", changed(ServiceState::Ready))),
            Applied::Resync
        );
        assert_eq!(mirror.seq(), 10, "the replica should stay where it was");
        assert_eq!(
            mirror.get(&"valkey@9".parse().unwrap()).unwrap().state,
            ServiceState::Stopped
        );

        // And a snapshot puts it right again.
        mirror.reset(Overview {
            seq: 13,
            services: vec![status("valkey@9", ServiceState::Ready)],
        });
        assert_eq!(mirror.seq(), 13);
        assert_eq!(
            mirror.apply(&event(14, "valkey@9", changed(ServiceState::Stopping))),
            Applied::Moved
        );
    }

    #[test]
    fn a_new_registration_needs_a_snapshot_to_describe_it() {
        let mut mirror = seeded();

        // The event says something exists but not its ports or its name.
        assert_eq!(
            mirror.apply(&event(11, "postgres@17", EventKind::Registered)),
            Applied::Resync
        );
    }

    #[test]
    fn phases_arrive_and_clear() {
        let mut mirror = seeded();
        let id = "valkey@9".parse().unwrap();

        mirror.apply(&event(
            11,
            "valkey@9",
            EventKind::Preparing {
                step: "initialise the database".to_string(),
            },
        ));
        assert_eq!(
            mirror.get(&id).unwrap().activity.as_deref(),
            Some("initialise the database")
        );

        mirror.apply(&event(
            12,
            "valkey@9",
            EventKind::Prepared {
                step: "initialise the database".to_string(),
                took: std::time::Duration::from_secs(1),
            },
        ));
        assert_eq!(mirror.get(&id).unwrap().activity, None);
    }

    #[test]
    fn progress_rewords_the_phase_without_disturbing_the_state() {
        let mut mirror = seeded();
        let id = "valkey@9".parse().unwrap();

        mirror.apply(&event(
            11,
            "valkey@9",
            EventKind::Preparing {
                step: "download valkey 9.1.1".to_string(),
            },
        ));
        mirror.apply(&event(
            12,
            "valkey@9",
            EventKind::Progress {
                step: "download valkey 9.1.1 42%".to_string(),
            },
        ));

        let service = mirror.get(&id).unwrap();
        assert_eq!(
            service.activity.as_deref(),
            Some("download valkey 9.1.1 42%")
        );
        assert_eq!(
            service.state,
            ServiceState::Stopped,
            "progress is not a state"
        );
    }

    #[test]
    fn what_is_in_the_way_arrives_and_clears() {
        let mut mirror = seeded();
        let id = "valkey@9".parse().unwrap();

        mirror.apply(&event(
            11,
            "valkey@9",
            EventKind::Blocked {
                by: Some("port 6379 is held by redis-server".to_string()),
            },
        ));
        assert!(mirror.get(&id).unwrap().blocked.is_some());

        // Starting settles it: a service in motion is not blocked, and the
        // next survey says so again if it still is.
        mirror.apply(&event(12, "valkey@9", changed(ServiceState::Starting)));
        assert_eq!(mirror.get(&id).unwrap().blocked, None);
    }

    #[test]
    fn a_replica_never_claims_a_pid_it_was_not_told() {
        let mut mirror = seeded();
        let id = "valkey@9".parse().unwrap();
        assert_eq!(mirror.get(&id).unwrap().pid, Some(42), "from the snapshot");

        mirror.apply(&event(11, "valkey@9", changed(ServiceState::Ready)));

        assert_eq!(
            mirror.get(&id).unwrap().pid,
            None,
            "the stream carries no pid, so the replica must not invent one"
        );
    }

    #[test]
    fn trouble_outranks_motion_and_motion_outranks_running() {
        let glyph = |services: Vec<ServiceStatus>| {
            let mut mirror = Mirror::new();
            mirror.reset(Overview { seq: 0, services });
            mirror.summary().glyph()
        };

        assert_eq!(glyph(vec![]), Glyph::Idle);
        assert_eq!(
            glyph(vec![status("a@1", ServiceState::Stopped)]),
            Glyph::Idle
        );
        assert_eq!(
            glyph(vec![
                status("a@1", ServiceState::Ready),
                status("b@1", ServiceState::Ready)
            ]),
            Glyph::Running(2)
        );
        assert_eq!(
            glyph(vec![
                status("a@1", ServiceState::Ready),
                status("b@1", ServiceState::Starting)
            ]),
            Glyph::Working
        );
        assert_eq!(
            glyph(vec![
                status("a@1", ServiceState::Starting),
                status("b@1", ServiceState::failed("boom"))
            ]),
            Glyph::Failed,
            "a failure must not hide behind something that is starting"
        );
    }
}
