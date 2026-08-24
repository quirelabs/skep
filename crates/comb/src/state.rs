use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ServiceState {
    Stopped,
    Starting,
    Ready,
    Stopping,
    Failed { reason: String },
    Restarting { attempt: u32 },
}

impl ServiceState {
    pub fn failed(reason: impl Into<String>) -> Self {
        Self::Failed {
            reason: reason.into(),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Stopping => "stopping",
            Self::Failed { .. } => "failed",
            Self::Restarting { .. } => "restarting",
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// True while the service is moving between resting states. Frontends paint
    /// this with the transient accent colour.
    pub fn is_transitional(&self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Stopping | Self::Restarting { .. }
        )
    }

    /// The single source of truth for what the supervisor is allowed to do.
    /// Transitions to the same state are never legal, so a repeated event
    /// cannot slip through unnoticed. `Failed` is the only door into
    /// `Restarting`, which makes "every restart has a recorded cause" a
    /// property of the state machine rather than a habit of the supervisor.
    pub fn can_transition_to(&self, next: &Self) -> bool {
        use ServiceState::*;
        matches!(
            (self, next),
            (Stopped, Starting)
                | (Starting, Ready | Stopping | Failed { .. })
                | (Ready, Stopping | Failed { .. })
                | (Stopping, Stopped | Failed { .. })
                | (Failed { .. }, Starting | Restarting { .. } | Stopped)
                | (Restarting { .. }, Starting | Stopped | Failed { .. })
        )
    }
}

impl fmt::Display for ServiceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::ServiceState::*;
    use super::*;

    #[test]
    fn a_normal_run_is_legal_end_to_end() {
        let run = [Stopped, Starting, Ready, Stopping, Stopped];
        for pair in run.windows(2) {
            assert!(
                pair[0].can_transition_to(&pair[1]),
                "{} -> {} should be legal",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn a_crash_loop_is_legal_end_to_end() {
        let cycle = [
            Ready,
            ServiceState::failed("exit 1"),
            Restarting { attempt: 1 },
            Starting,
            ServiceState::failed("exit 1"),
            Restarting { attempt: 2 },
            Starting,
            Ready,
        ];
        for pair in cycle.windows(2) {
            assert!(
                pair[0].can_transition_to(&pair[1]),
                "{} -> {} should be legal",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn shortcuts_and_repeats_are_rejected() {
        assert!(!Stopped.can_transition_to(&Ready));
        assert!(!Stopped.can_transition_to(&Stopped));
        assert!(!Ready.can_transition_to(&Starting));
        assert!(!Ready.can_transition_to(&Ready));
        assert!(!Stopping.can_transition_to(&Ready));
        assert!(!Starting.can_transition_to(&Restarting { attempt: 1 }));
        assert!(!Restarting { attempt: 1 }.can_transition_to(&Restarting { attempt: 2 }));
    }

    #[test]
    fn every_restart_is_preceded_by_a_recorded_failure() {
        assert!(!Ready.can_transition_to(&Restarting { attempt: 1 }));
        assert!(!Starting.can_transition_to(&Restarting { attempt: 1 }));
        assert!(!Stopped.can_transition_to(&Restarting { attempt: 1 }));
        assert!(ServiceState::failed("exit 1").can_transition_to(&Restarting { attempt: 1 }));
    }

    #[test]
    fn a_user_stop_wins_over_a_pending_restart() {
        assert!(Restarting { attempt: 3 }.can_transition_to(&Stopped));
        assert!(ServiceState::failed("boom").can_transition_to(&Stopped));
    }

    #[test]
    fn only_ready_counts_as_running() {
        assert!(Ready.is_running());
        assert!(!Starting.is_running());
        assert!(Starting.is_transitional());
        assert!(Restarting { attempt: 1 }.is_transitional());
        assert!(!ServiceState::failed("boom").is_transitional());
    }

    #[test]
    fn serialises_flat_with_a_state_tag() {
        let json = serde_json::to_string(&ServiceState::failed("exit 1")).unwrap();
        assert_eq!(json, r#"{"state":"failed","reason":"exit 1"}"#);
        assert_eq!(
            serde_json::to_string(&Ready).unwrap(),
            r#"{"state":"ready"}"#
        );
    }
}
