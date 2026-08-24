use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};

use crate::error::{Error, Result};
use crate::event::{Event, EventKind, LogLine};
use crate::id::InstanceId;
use crate::spec::ServiceSpec;
use crate::state::ServiceState;
use crate::time::Timestamp;

const EVENT_CAPACITY: usize = 1024;
const LOG_CAPACITY: usize = 1024;

/// What a frontend needs to draw one row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub id: InstanceId,
    pub display_name: String,
    #[serde(flatten)]
    pub state: ServiceState,
    pub ports: BTreeMap<String, u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// When the current state was entered.
    pub since: Timestamp,
}

/// Owns the service graph and every state change in it. Cheap to clone: the
/// GUI, the CLI and the MCP server all hold the same engine.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    instances: RwLock<BTreeMap<InstanceId, Instance>>,
    events: broadcast::Sender<Event>,
    seq: AtomicU64,
}

struct Instance {
    spec: ServiceSpec,
    state: ServiceState,
    since: Timestamp,
    pid: Option<u32>,
    logs: broadcast::Sender<LogLine>,
}

impl Engine {
    pub fn new() -> Self {
        Self::with_event_capacity(EVENT_CAPACITY)
    }

    pub fn with_event_capacity(capacity: usize) -> Self {
        let (events, _) = broadcast::channel(capacity);
        Self {
            inner: Arc::new(Inner {
                instances: RwLock::new(BTreeMap::new()),
                events,
                seq: AtomicU64::new(0),
            }),
        }
    }

    pub async fn register(&self, spec: ServiceSpec) -> Result<()> {
        let id = spec.id.clone();
        let mut instances = self.inner.instances.write().await;
        if instances.contains_key(&id) {
            return Err(Error::AlreadyRegistered(id));
        }
        let (logs, _) = broadcast::channel(LOG_CAPACITY);
        instances.insert(
            id.clone(),
            Instance {
                spec,
                state: ServiceState::Stopped,
                since: Timestamp::now(),
                pid: None,
                logs,
            },
        );
        self.inner.emit(Some(id), EventKind::Registered);
        Ok(())
    }

    pub async fn deregister(&self, id: &InstanceId) -> Result<()> {
        let mut instances = self.inner.instances.write().await;
        let instance = instances
            .get(id)
            .ok_or_else(|| Error::UnknownInstance(id.clone()))?;
        if instance.state != ServiceState::Stopped {
            return Err(Error::NotStopped {
                instance: id.clone(),
                state: instance.state.clone(),
            });
        }
        instances.remove(id);
        self.inner.emit(Some(id.clone()), EventKind::Deregistered);
        Ok(())
    }

    /// Ordered by id, so every frontend lists services the same way.
    pub async fn status(&self) -> Vec<ServiceStatus> {
        self.inner
            .instances
            .read()
            .await
            .values()
            .map(Instance::status)
            .collect()
    }

    pub async fn status_of(&self, id: &InstanceId) -> Result<ServiceStatus> {
        self.inner
            .instances
            .read()
            .await
            .get(id)
            .map(Instance::status)
            .ok_or_else(|| Error::UnknownInstance(id.clone()))
    }

    pub async fn spec_of(&self, id: &InstanceId) -> Result<ServiceSpec> {
        self.inner
            .instances
            .read()
            .await
            .get(id)
            .map(|instance| instance.spec.clone())
            .ok_or_else(|| Error::UnknownInstance(id.clone()))
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    pub async fn subscribe_logs(&self, id: &InstanceId) -> Result<broadcast::Receiver<LogLine>> {
        self.inner
            .instances
            .read()
            .await
            .get(id)
            .map(|instance| instance.logs.subscribe())
            .ok_or_else(|| Error::UnknownInstance(id.clone()))
    }

    pub async fn start(&self, id: &InstanceId) -> Result<()> {
        self.require_registered(id).await?;
        Err(Error::NotImplemented("start"))
    }

    pub async fn stop(&self, id: &InstanceId) -> Result<()> {
        self.require_registered(id).await?;
        Err(Error::NotImplemented("stop"))
    }

    pub async fn restart(&self, id: &InstanceId) -> Result<()> {
        self.require_registered(id).await?;
        Err(Error::NotImplemented("restart"))
    }

    async fn require_registered(&self, id: &InstanceId) -> Result<()> {
        if self.inner.instances.read().await.contains_key(id) {
            Ok(())
        } else {
            Err(Error::UnknownInstance(id.clone()))
        }
    }

    /// The only way a state ever changes. Illegal moves are rejected before
    /// anything is written, so an observer never sees an impossible history.
    #[allow(dead_code)]
    pub(crate) async fn transition(&self, id: &InstanceId, to: ServiceState) -> Result<()> {
        let mut instances = self.inner.instances.write().await;
        let instance = instances
            .get_mut(id)
            .ok_or_else(|| Error::UnknownInstance(id.clone()))?;
        if !instance.state.can_transition_to(&to) {
            return Err(Error::IllegalTransition {
                instance: id.clone(),
                from: instance.state.clone(),
                to,
            });
        }
        let from = std::mem::replace(&mut instance.state, to.clone());
        instance.since = Timestamp::now();
        if !to.is_running() {
            instance.pid = None;
        }
        // Emitted under the write lock so sequence order matches state order.
        self.inner
            .emit(Some(id.clone()), EventKind::StateChanged { from, to });
        Ok(())
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Inner {
    fn emit(&self, instance: Option<InstanceId>, kind: EventKind) {
        let event = Event {
            seq: self.seq.fetch_add(1, Ordering::Relaxed) + 1,
            at: Timestamp::now(),
            instance,
            kind,
        };
        // Errors only mean nobody is listening.
        let _ = self.events.send(event);
    }
}

impl Instance {
    fn status(&self) -> ServiceStatus {
        ServiceStatus {
            id: self.spec.id.clone(),
            display_name: self.spec.display_name.clone(),
            state: self.state.clone(),
            ports: self
                .spec
                .ports
                .iter()
                .map(|port| (port.name.clone(), port.number))
                .collect(),
            pid: self.pid,
            since: self.since,
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::broadcast::error::TryRecvError;

    use super::*;
    use crate::spec::{BinarySpec, Port};

    fn spec(id: &str) -> ServiceSpec {
        let id: InstanceId = id.parse().unwrap();
        let data_dir = format!("/tmp/skep-test/{id}");
        ServiceSpec::new(id, BinarySpec::path("/bin/sleep"), data_dir)
            .with_ports([Port::new("main", 6379)])
    }

    async fn engine_with(ids: &[&str]) -> Engine {
        let engine = Engine::new();
        for id in ids {
            engine.register(spec(id)).await.unwrap();
        }
        engine
    }

    #[tokio::test]
    async fn registered_services_start_out_stopped_and_sorted() {
        let engine = engine_with(&["valkey@8", "postgres@16", "postgres@16:wip"]).await;
        let status = engine.status().await;

        let ids: Vec<String> = status.iter().map(|s| s.id.to_string()).collect();
        assert_eq!(ids, ["postgres@16", "postgres@16:wip", "valkey@8"]);
        assert!(status.iter().all(|s| s.state == ServiceState::Stopped));
        assert_eq!(status[0].ports["main"], 6379);
    }

    #[tokio::test]
    async fn registering_twice_is_an_error() {
        let engine = engine_with(&["valkey@8"]).await;
        assert!(matches!(
            engine.register(spec("valkey@8")).await,
            Err(Error::AlreadyRegistered(_))
        ));
    }

    #[tokio::test]
    async fn transitions_move_state_and_announce_themselves() {
        let engine = engine_with(&["valkey@8"]).await;
        let id: InstanceId = "valkey@8".parse().unwrap();
        let mut events = engine.subscribe_events();

        engine
            .transition(&id, ServiceState::Starting)
            .await
            .unwrap();
        engine.transition(&id, ServiceState::Ready).await.unwrap();

        assert!(engine.status_of(&id).await.unwrap().state.is_running());

        let first = events.recv().await.unwrap();
        let second = events.recv().await.unwrap();
        assert_eq!(first.instance.as_ref(), Some(&id));
        assert!(second.seq > first.seq);
        assert_eq!(
            first.kind,
            EventKind::StateChanged {
                from: ServiceState::Stopped,
                to: ServiceState::Starting,
            }
        );
    }

    #[tokio::test]
    async fn illegal_transitions_change_nothing() {
        let engine = engine_with(&["valkey@8"]).await;
        let id: InstanceId = "valkey@8".parse().unwrap();
        let mut events = engine.subscribe_events();

        let result = engine.transition(&id, ServiceState::Ready).await;

        assert!(matches!(result, Err(Error::IllegalTransition { .. })));
        assert_eq!(
            engine.status_of(&id).await.unwrap().state,
            ServiceState::Stopped
        );
        assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));
    }

    #[tokio::test]
    async fn restart_attempts_increment_through_legal_transitions() {
        let engine = engine_with(&["valkey@8"]).await;
        let id: InstanceId = "valkey@8".parse().unwrap();
        engine
            .transition(&id, ServiceState::Starting)
            .await
            .unwrap();
        engine.transition(&id, ServiceState::Ready).await.unwrap();

        for attempt in 1..=3 {
            engine
                .transition(&id, ServiceState::failed("exit 1"))
                .await
                .unwrap();
            engine
                .transition(&id, ServiceState::Restarting { attempt })
                .await
                .unwrap();
            assert_eq!(
                engine.status_of(&id).await.unwrap().state,
                ServiceState::Restarting { attempt }
            );

            // The counter never bumps in place. It only moves by going back
            // through a start, so every attempt has a recorded outcome.
            assert!(matches!(
                engine
                    .transition(
                        &id,
                        ServiceState::Restarting {
                            attempt: attempt + 1
                        }
                    )
                    .await,
                Err(Error::IllegalTransition { .. })
            ));
            engine
                .transition(&id, ServiceState::Starting)
                .await
                .unwrap();
        }

        engine.transition(&id, ServiceState::Ready).await.unwrap();
    }

    #[tokio::test]
    async fn unknown_instances_are_reported_before_anything_else() {
        let engine = Engine::new();
        let id: InstanceId = "valkey@8".parse().unwrap();

        assert!(matches!(
            engine.start(&id).await,
            Err(Error::UnknownInstance(_))
        ));
        assert!(matches!(
            engine.subscribe_logs(&id).await,
            Err(Error::UnknownInstance(_))
        ));
    }

    #[tokio::test]
    async fn lifecycle_calls_are_still_stubs() {
        let engine = engine_with(&["valkey@8"]).await;
        let id: InstanceId = "valkey@8".parse().unwrap();

        assert!(matches!(
            engine.start(&id).await,
            Err(Error::NotImplemented("start"))
        ));
        assert!(matches!(
            engine.restart(&id).await,
            Err(Error::NotImplemented("restart"))
        ));
    }

    #[tokio::test]
    async fn a_running_service_cannot_be_deregistered() {
        let engine = engine_with(&["valkey@8"]).await;
        let id: InstanceId = "valkey@8".parse().unwrap();
        engine
            .transition(&id, ServiceState::Starting)
            .await
            .unwrap();

        assert!(matches!(
            engine.deregister(&id).await,
            Err(Error::NotStopped { .. })
        ));

        engine
            .transition(&id, ServiceState::Stopping)
            .await
            .unwrap();
        engine.transition(&id, ServiceState::Stopped).await.unwrap();
        engine.deregister(&id).await.unwrap();
        assert!(engine.status().await.is_empty());
    }

    #[tokio::test]
    async fn status_serialises_compactly() {
        let engine = engine_with(&["postgres@16"]).await;
        let status = engine.status().await;
        let json = serde_json::to_value(&status[0]).unwrap();

        assert_eq!(json["id"], "postgres@16");
        assert_eq!(json["state"], "stopped");
        assert_eq!(json["ports"]["main"], 6379);
        assert!(json.get("pid").is_none());
    }
}
