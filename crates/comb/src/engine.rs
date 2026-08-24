use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command};
use tokio::sync::{RwLock, broadcast};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::error::{Error, Result};
use crate::event::{Event, EventKind, LogLine, LogStream};
use crate::id::InstanceId;
use crate::logs::{LogSink, RingBuffer, pump};
use crate::paths::Paths;
use crate::signal;
use crate::spec::ServiceSpec;
use crate::state::ServiceState;
use crate::time::Timestamp;

const EVENT_CAPACITY: usize = 1024;
const LOG_CAPACITY: usize = 1024;
const LOG_HISTORY: usize = 1000;

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
    paths: Paths,
}

struct Instance {
    spec: ServiceSpec,
    state: ServiceState,
    since: Timestamp,
    pid: Option<u32>,
    logs: broadcast::Sender<LogLine>,
    history: Arc<Mutex<RingBuffer>>,
    running: Option<Running>,
}

/// A live child and the tasks draining its output.
struct Running {
    child: Child,
    pumps: Vec<JoinHandle<()>>,
}

impl Running {
    /// SIGTERM, then SIGKILL once the grace period is up. The pumps end by
    /// themselves when the pipes close, so awaiting them drains the last lines.
    async fn shut_down(&mut self, grace: Duration) {
        if let Some(pid) = self.child.id() {
            let _ = signal::terminate(pid);
        }
        if timeout(grace, self.child.wait()).await.is_err() {
            let _ = self.child.start_kill();
            let _ = self.child.wait().await;
        }
        for pump in self.pumps.drain(..) {
            let _ = pump.await;
        }
    }
}

impl Engine {
    pub fn new() -> Self {
        Self::with_paths(Paths::from_env())
    }

    pub fn with_paths(paths: Paths) -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            inner: Arc::new(Inner {
                instances: RwLock::new(BTreeMap::new()),
                events,
                seq: AtomicU64::new(0),
                paths,
            }),
        }
    }

    pub fn paths(&self) -> &Paths {
        &self.inner.paths
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
                history: Arc::new(Mutex::new(RingBuffer::new(LOG_HISTORY))),
                running: None,
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

    /// The newest captured lines, oldest first.
    pub async fn logs(&self, id: &InstanceId, lines: usize) -> Result<Vec<LogLine>> {
        let instances = self.inner.instances.read().await;
        let instance = instances
            .get(id)
            .ok_or_else(|| Error::UnknownInstance(id.clone()))?;
        let history = instance
            .history
            .lock()
            .map_err(|_| Error::Poisoned(id.clone()))?;
        Ok(history.tail(lines))
    }

    /// Moving to `Starting` first is what makes this safe to call twice: the
    /// second caller is rejected by the state machine, not by a lock.
    pub async fn start(&self, id: &InstanceId) -> Result<()> {
        self.transition(id, ServiceState::Starting).await?;
        match self.spawn(id).await {
            Ok(()) => self.transition(id, ServiceState::Ready).await,
            Err(error) => {
                let failed = ServiceState::failed(error.to_string());
                self.transition(id, failed).await?;
                Err(error)
            }
        }
    }

    pub async fn stop(&self, id: &InstanceId) -> Result<()> {
        self.transition(id, ServiceState::Stopping).await?;

        let (running, grace) = {
            let mut instances = self.inner.instances.write().await;
            let instance = instances
                .get_mut(id)
                .ok_or_else(|| Error::UnknownInstance(id.clone()))?;
            (instance.running.take(), instance.spec.shutdown.grace)
        };
        if let Some(mut running) = running {
            running.shut_down(grace).await;
        }

        self.transition(id, ServiceState::Stopped).await
    }

    pub async fn restart(&self, id: &InstanceId) -> Result<()> {
        match self.status_of(id).await?.state {
            ServiceState::Stopped => {}
            ServiceState::Failed { .. } => self.transition(id, ServiceState::Stopped).await?,
            _ => self.stop(id).await?,
        }
        self.start(id).await
    }

    async fn spawn(&self, id: &InstanceId) -> Result<()> {
        let spec = self.spec_of(id).await?;
        let program = spec.binary.resolve(&self.inner.paths);

        tokio::fs::create_dir_all(&spec.data_dir)
            .await
            .map_err(|source| Error::DataDir {
                instance: id.clone(),
                path: spec.data_dir.display().to_string(),
                source,
            })?;

        let mut command = Command::new(&program);
        command
            .args(&spec.args)
            .envs(&spec.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Never leave a child behind if the engine goes away.
            .kill_on_drop(true);
        if let Some(dir) = &spec.working_dir {
            command.current_dir(dir);
        }

        let mut child = command.spawn().map_err(|source| Error::Spawn {
            instance: id.clone(),
            program: program.display().to_string(),
            source,
        })?;
        let pid = child.id();
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");

        let mut instances = self.inner.instances.write().await;
        let instance = instances
            .get_mut(id)
            .ok_or_else(|| Error::UnknownInstance(id.clone()))?;
        let sink = LogSink {
            buffer: instance.history.clone(),
            live: instance.logs.clone(),
        };
        instance.running = Some(Running {
            child,
            pumps: vec![
                pump(stdout, LogStream::Stdout, sink.clone()),
                pump(stderr, LogStream::Stderr, sink),
            ],
        });
        instance.pid = pid;
        Ok(())
    }

    /// The only way a state ever changes. Illegal moves are rejected before
    /// anything is written, so an observer never sees an impossible history.
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
    async fn a_binary_that_is_not_there_fails_the_start() {
        let engine = Engine::new();
        let id: InstanceId = "valkey@8".parse().unwrap();
        let spec = ServiceSpec::new(
            id.clone(),
            BinarySpec::path("/nonexistent/valkey"),
            std::env::temp_dir().join("skep-missing-binary"),
        );
        engine.register(spec).await.unwrap();

        let error = engine.start(&id).await.unwrap_err();

        assert!(matches!(error, Error::Spawn { .. }));
        assert!(error.to_string().contains("/nonexistent/valkey"));
        let state = engine.status_of(&id).await.unwrap().state;
        assert!(matches!(state, ServiceState::Failed { .. }));
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
