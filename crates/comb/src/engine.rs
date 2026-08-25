use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::process::{Child, Command};
use tokio::sync::{RwLock, broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

use crate::error::{Error, Result};
use crate::event::{Event, EventKind, LogLine, LogStream};
use crate::graph;
use crate::id::InstanceId;
use crate::logs::{LogSink, RingBuffer, pump};
use crate::paths::Paths;
use crate::platform;
use crate::probe;
use crate::scratch::ScratchDir;
use crate::spec::{
    BinarySpec, HealthCheck, PrepareStep, RestartPolicy, ServiceSpec, ShutdownSpec, StopSignal,
};
use crate::state::ServiceState;
use crate::time::Timestamp;

const EVENT_CAPACITY: usize = 1024;
const LOG_CAPACITY: usize = 1024;
const LOG_HISTORY: usize = 1000;
const FALLBACK_GRACE: Duration = Duration::from_secs(10);

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
    /// The named phase of a long start, so a caller never sees Starting with
    /// no explanation for ten seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
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
    activity: Option<String>,
    running: Option<Running>,
    /// Restart attempts since the last deliberate start.
    attempt: u32,
}

/// The handle a caller uses to interrupt a supervisor, whether it is watching a
/// live child or waiting out a backoff.
struct Running {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

/// A live child and the tasks draining its output. Exactly one supervisor owns
/// one of these at a time, which is what makes waiting on the child safe.
struct Process {
    child: Child,
    pumps: Vec<JoinHandle<()>>,
}

impl Process {
    /// SIGTERM, then SIGKILL once the grace period is up. If asking politely
    /// failed there is nothing to wait for, so the grace is skipped rather
    /// than spent pretending.
    async fn shut_down(&mut self, shutdown: &ShutdownSpec) {
        let asked = match self.child.id() {
            Some(pid) => platform::terminate(pid, shutdown.signal).is_ok(),
            None => false,
        };
        if !asked || timeout(shutdown.grace, self.child.wait()).await.is_err() {
            let _ = self.child.start_kill();
            let _ = self.child.wait().await;
        }
        self.drain().await;
    }

    /// The pumps end on their own once the pipes close, so awaiting them keeps
    /// the last lines a process wrote instead of truncating them.
    async fn drain(&mut self) {
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
                activity: None,
                running: None,
                attempt: 0,
            },
        );
        self.inner.emit(Some(id), EventKind::Registered);
        Ok(())
    }

    /// Registers a spec, or updates the one already there. A stopped service
    /// can be redefined freely; a running one only if nothing changed, since
    /// silently reconfiguring something that is serving would be a lie.
    pub async fn upsert(&self, spec: ServiceSpec) -> Result<()> {
        let id = spec.id.clone();
        let mut instances = self.inner.instances.write().await;
        match instances.get_mut(&id) {
            None => {
                drop(instances);
                return self.register(spec).await;
            }
            Some(instance) if instance.spec == spec => Ok(()),
            Some(instance) if instance.state == ServiceState::Stopped => {
                instance.spec = spec;
                Ok(())
            }
            Some(instance) => Err(Error::NotStopped {
                instance: id.clone(),
                state: instance.state.clone(),
            }),
        }
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
        self.set_attempt(id, 0).await;
        let spec = self.spec_of(id).await?;

        let process = match self.launch_and_settle(id, &spec).await {
            Ok(process) => process,
            Err(error) => {
                self.transition(id, ServiceState::failed(error.to_string()))
                    .await?;
                return Err(error);
            }
        };

        // Ready lands before the supervisor exists, so a supervisor can never
        // race the very state it is there to watch. If this transition loses to
        // a concurrent stop, the process drops and kill_on_drop cleans up.
        self.transition(id, ServiceState::Ready).await?;
        self.watch(id, process).await;
        Ok(())
    }

    /// Works from any live state. A service waiting out a backoff has no child
    /// to signal but still has a supervisor to call off, and one that already
    /// gave up has neither.
    pub async fn stop(&self, id: &InstanceId) -> Result<()> {
        self.announce_stopping(id).await?;

        if let Some(running) = self.take_running(id).await {
            let _ = running.stop.send(true);
            let _ = running.task.await;
        }

        // A supervisor can finish a relaunch in the instant before it sees the
        // stop, so the state is read again rather than assumed.
        self.announce_stopping(id).await?;
        self.transition(id, ServiceState::Stopped).await
    }

    /// Moves to `Stopping` only from the states that have something to wind
    /// down, letting the transition matrix decide rather than a second list.
    async fn announce_stopping(&self, id: &InstanceId) -> Result<()> {
        let state = self.status_of(id).await?.state;
        if state.can_transition_to(&ServiceState::Stopping) {
            self.transition(id, ServiceState::Stopping).await?;
        }
        Ok(())
    }

    pub async fn restart(&self, id: &InstanceId) -> Result<()> {
        if !matches!(self.status_of(id).await?.state, ServiceState::Stopped) {
            self.stop(id).await?;
        }
        self.start(id).await
    }

    /// Brings up everything the requested services need. Each one waits only
    /// for its own dependencies to report ready, so independent branches of the
    /// graph boot at the same time.
    pub async fn start_all(&self, ids: &[InstanceId]) -> Result<()> {
        let edges = self.dependency_edges().await;
        let order = graph::plan(&edges, ids)?;
        let waits = graph::upward(&order, &edges);
        self.cascade(&order, &waits, Action::Start).await
    }

    /// The same walk in reverse: nothing is stopped while something that
    /// depends on it is still running.
    pub async fn stop_all(&self, ids: &[InstanceId]) -> Result<()> {
        let edges = self.dependency_edges().await;
        let order = graph::plan(&edges, ids)?;
        let waits = graph::downward(&order, &edges);
        let reversed: Vec<InstanceId> = order.into_iter().rev().collect();
        self.cascade(&reversed, &waits, Action::Stop).await
    }

    /// Stops every service that is not already stopped, in dependency order.
    pub async fn stop_everything(&self) -> Result<()> {
        let running: Vec<InstanceId> = self
            .status()
            .await
            .into_iter()
            .filter(|status| status.state != ServiceState::Stopped)
            .map(|status| status.id)
            .collect();
        if running.is_empty() {
            return Ok(());
        }
        self.stop_all(&running).await
    }

    async fn dependency_edges(&self) -> graph::Edges {
        self.inner
            .instances
            .read()
            .await
            .iter()
            .map(|(id, instance)| (id.clone(), instance.spec.depends_on.clone()))
            .collect()
    }

    /// One task per service, each gated on the services it must not overtake.
    /// A gate that reports failure fails its dependents rather than hanging.
    async fn cascade(
        &self,
        order: &[InstanceId],
        waits: &HashMap<InstanceId, Vec<InstanceId>>,
        action: Action,
    ) -> Result<()> {
        let mut senders = HashMap::new();
        let mut gates = HashMap::new();
        for id in order {
            let (done, gate) = watch::channel(None::<bool>);
            senders.insert(id.clone(), done);
            gates.insert(id.clone(), gate);
        }

        let mut tasks = Vec::with_capacity(order.len());
        for id in order {
            let waiting: Vec<_> = waits
                .get(id)
                .into_iter()
                .flatten()
                .filter_map(|other| gates.get(other).map(|gate| (other.clone(), gate.clone())))
                .collect();
            let done = senders.remove(id).expect("every planned id has a channel");
            let engine = self.clone();
            let id = id.clone();

            tasks.push(tokio::spawn(async move {
                for (other, mut gate) in waiting {
                    loop {
                        let settled = *gate.borrow_and_update();
                        match settled {
                            Some(true) => break,
                            Some(false) => {
                                let _ = done.send(Some(false));
                                return Err(Error::DependencyFailed {
                                    instance: id.clone(),
                                    dependency: other,
                                });
                            }
                            None => {}
                        }
                        if gate.changed().await.is_err() {
                            let _ = done.send(Some(false));
                            return Err(Error::DependencyFailed {
                                instance: id.clone(),
                                dependency: other,
                            });
                        }
                    }
                }

                let outcome = match action {
                    Action::Start => engine.start(&id).await,
                    // A dependency that is already down is not a failure.
                    Action::Stop => match engine.status_of(&id).await {
                        Ok(status) if status.state == ServiceState::Stopped => Ok(()),
                        _ => engine.stop(&id).await,
                    },
                };
                let _ = done.send(Some(outcome.is_ok()));
                outcome
            }));
        }

        let mut first = None;
        for task in tasks {
            if let Ok(Err(error)) = task.await {
                first.get_or_insert(error);
            }
        }
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Hands a freshly launched process to a supervisor that owns it from here.
    async fn watch(&self, id: &InstanceId, process: Process) {
        let (stop, listener) = watch::channel(false);
        let task = tokio::spawn(supervise(self.clone(), id.clone(), process, listener));
        let mut instances = self.inner.instances.write().await;
        if let Some(instance) = instances.get_mut(id) {
            instance.running = Some(Running { stop, task });
        }
    }

    /// Starts the process and waits for it to answer. Dropping the process on
    /// any failure is what stops a half started service from lingering.
    async fn launch_and_settle(&self, id: &InstanceId, spec: &ServiceSpec) -> Result<Process> {
        self.check_ports(id, spec).await?;
        self.provision(id, spec).await?;
        self.ensure_data_dir(id, spec).await?;
        self.prepare(id, spec).await?;
        let mut process = self.launch(id).await?;
        let settled = tokio::select! {
            result = self.await_ready(id, &spec.health) => result,
            exit = process.child.wait() => {
                let reason = match exit {
                    Ok(status) => platform::describe_exit(&status),
                    Err(error) => format!("could not be waited on: {error}"),
                };
                // The port can be taken between the check and the bind, so ask
                // again. A bind failure deserves the same answer either way.
                Err(match self.check_ports(id, spec).await {
                    Err(taken) => taken,
                    Ok(()) => Error::DiedStarting { instance: id.clone(), reason },
                })
            }
        };
        settled.map(|()| process)
    }

    /// Polls until the service answers or the startup budget runs out. Only the
    /// final failure is published: a boot that probes fifty times should not
    /// put fifty events on the stream.
    async fn await_ready(&self, id: &InstanceId, health: &HealthCheck) -> Result<()> {
        let started = Instant::now();
        loop {
            match probe::check(&health.probe, health.timeout).await {
                Ok(()) => {
                    self.inner.emit(
                        Some(id.clone()),
                        EventKind::ProbeSucceeded {
                            after: started.elapsed(),
                        },
                    );
                    return Ok(());
                }
                Err(reason) if started.elapsed() >= health.startup_timeout => {
                    self.inner.emit(
                        Some(id.clone()),
                        EventKind::ProbeFailed {
                            reason: reason.clone(),
                        },
                    );
                    return Err(Error::NotReady {
                        instance: id.clone(),
                        reason,
                    });
                }
                Err(_) => sleep(health.interval).await,
            }
        }
    }

    /// Refuses to start onto a port someone else holds. This narrows the race
    /// rather than closing it, which is why the same check runs again if the
    /// process dies during startup.
    async fn check_ports(&self, id: &InstanceId, spec: &ServiceSpec) -> Result<()> {
        for port in &spec.ports {
            let Some(listener) = platform::listener_on(port.number).await else {
                continue;
            };
            self.inner.emit(
                Some(id.clone()),
                EventKind::PortConflict {
                    port: port.number,
                    pid: Some(listener.pid),
                    process: Some(listener.command.clone()),
                },
            );
            return Err(Error::PortTaken {
                port: port.number,
                message: crate::ports::describe(port.number, &listener),
            });
        }
        Ok(())
    }

    /// Installs the pinned release if the binary is not there yet. Announced
    /// like any other phase, because a first start that downloads forty
    /// megabytes should say so rather than look stuck.
    async fn provision(&self, id: &InstanceId, spec: &ServiceSpec) -> Result<()> {
        let (Some(release), BinarySpec::Managed { name, version, .. }) =
            (&spec.release, &spec.binary)
        else {
            return Ok(());
        };
        if spec.binary.resolve(&self.inner.paths).exists() {
            return Ok(());
        }

        let step = format!("download {name} {version}");
        self.set_activity(id, Some(step.clone())).await;
        self.inner.emit(
            Some(id.clone()),
            EventKind::Preparing { step: step.clone() },
        );

        let started = Instant::now();
        // Build output goes to the service's own history, so a failed compile
        // is diagnosable exactly where everything else about it is.
        let sink = self.sink_of(id).await?;
        crate::acquire::ensure_reported(&self.inner.paths, name, release, &sink).await?;

        self.inner.emit(
            Some(id.clone()),
            EventKind::Prepared {
                step,
                took: started.elapsed(),
            },
        );
        self.set_activity(id, None).await;
        Ok(())
    }

    /// Runs the one-time setup a service needs before it can serve, announcing
    /// each phase so a long first start is legible rather than silent.
    async fn prepare(&self, id: &InstanceId, spec: &ServiceSpec) -> Result<()> {
        for step in &spec.prepare {
            if step
                .unless_exists
                .as_ref()
                .is_some_and(|marker| marker.exists())
            {
                continue;
            }

            self.set_activity(id, Some(step.name.clone())).await;
            self.inner.emit(
                Some(id.clone()),
                EventKind::Preparing {
                    step: step.name.clone(),
                },
            );

            let started = Instant::now();
            match &step.produces {
                // Built in scratch and renamed in as the last act, so a killed
                // step can never leave output that a later run would trust.
                Some(target) => {
                    let failed = |reason: String| Error::Prepare {
                        instance: id.clone(),
                        step: step.name.clone(),
                        reason,
                    };
                    let scratch = ScratchDir::beside(target, id.service.as_str())
                        .map_err(|error| failed(error.to_string()))?;
                    let output = scratch.join("output");
                    tokio::fs::create_dir_all(&output)
                        .await
                        .map_err(|error| failed(error.to_string()))?;

                    self.run_step(id, step, Some(&output)).await?;

                    scratch
                        .promote(&output, target)
                        .await
                        .map_err(|error| failed(error.to_string()))?;
                }
                None => self.run_step(id, step, None).await?,
            }

            self.inner.emit(
                Some(id.clone()),
                EventKind::Prepared {
                    step: step.name.clone(),
                    took: started.elapsed(),
                },
            );
        }
        self.set_activity(id, None).await;
        Ok(())
    }

    async fn run_step(
        &self,
        id: &InstanceId,
        step: &PrepareStep,
        output: Option<&Path>,
    ) -> Result<()> {
        let program = step.binary.resolve(&self.inner.paths);
        let failed = |reason: String| Error::Prepare {
            instance: id.clone(),
            step: step.name.clone(),
            reason,
        };

        let args: Vec<String> = step
            .args
            .iter()
            .map(|arg| substitute(arg, output))
            .collect();
        let env: Vec<(&String, String)> = step
            .env
            .iter()
            .map(|(key, value)| (key, substitute(value, output)))
            .collect();

        let mut child = Command::new(&program)
            .args(&args)
            .envs(env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| failed(format!("{} did not spawn: {error}", program.display())))?;

        // Setup output goes to the same history as the service's own, so the
        // reason a first start failed is where anyone would look for it.
        let sink = self.sink_of(id).await?;
        let pumps = vec![
            pump(
                child.stdout.take().expect("stdout is piped"),
                LogStream::Stdout,
                sink.clone(),
            ),
            pump(
                child.stderr.take().expect("stderr is piped"),
                LogStream::Stderr,
                sink,
            ),
        ];

        let status = child
            .wait()
            .await
            .map_err(|error| failed(error.to_string()))?;
        for pump in pumps {
            let _ = pump.await;
        }

        if status.success() {
            Ok(())
        } else {
            Err(failed(platform::describe_exit(&status)))
        }
    }

    async fn ensure_data_dir(&self, id: &InstanceId, spec: &ServiceSpec) -> Result<()> {
        tokio::fs::create_dir_all(&spec.data_dir)
            .await
            .map_err(|source| Error::DataDir {
                instance: id.clone(),
                path: spec.data_dir.display().to_string(),
                source,
            })
    }

    async fn sink_of(&self, id: &InstanceId) -> Result<LogSink> {
        let instances = self.inner.instances.read().await;
        let instance = instances
            .get(id)
            .ok_or_else(|| Error::UnknownInstance(id.clone()))?;
        Ok(LogSink {
            buffer: instance.history.clone(),
            live: instance.logs.clone(),
        })
    }

    async fn set_activity(&self, id: &InstanceId, activity: Option<String>) {
        let mut instances = self.inner.instances.write().await;
        if let Some(instance) = instances.get_mut(id) {
            instance.activity = activity;
        }
    }

    async fn launch(&self, id: &InstanceId) -> Result<Process> {
        let spec = self.spec_of(id).await?;
        let program = spec.binary.resolve(&self.inner.paths);

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
        instance.pid = pid;
        Ok(Process {
            child,
            pumps: vec![
                pump(stdout, LogStream::Stdout, sink.clone()),
                pump(stderr, LogStream::Stderr, sink),
            ],
        })
    }

    /// Waits out the backoff and brings the service back, or gives up. Returns
    /// `None` when the attempt cap is reached or a stop arrives mid-wait.
    async fn relaunch(&self, id: &InstanceId, stop: &mut watch::Receiver<bool>) -> Option<Process> {
        loop {
            let spec = self.spec_of(id).await.ok()?;
            let attempt = self.bump_attempt(id).await?;
            if spec.restart.backoff.is_exhausted(attempt) {
                return None;
            }
            let delay = spec.restart.backoff.delay_for(attempt);

            self.transition(id, ServiceState::Restarting { attempt })
                .await
                .ok()?;
            self.inner.emit(
                Some(id.clone()),
                EventKind::RestartScheduled { attempt, delay },
            );

            tokio::select! {
                _ = sleep(delay) => {}
                _ = stop.changed() => return None,
            }

            self.transition(id, ServiceState::Starting).await.ok()?;
            match self.launch_and_settle(id, &spec).await {
                Ok(process) => {
                    self.transition(id, ServiceState::Ready).await.ok()?;
                    return Some(process);
                }
                Err(error) => {
                    // A failed relaunch is just another attempt.
                    self.transition(id, ServiceState::failed(error.to_string()))
                        .await
                        .ok()?;
                }
            }
        }
    }

    async fn take_running(&self, id: &InstanceId) -> Option<Running> {
        let mut instances = self.inner.instances.write().await;
        instances.get_mut(id).and_then(|i| i.running.take())
    }

    async fn set_attempt(&self, id: &InstanceId, attempt: u32) {
        let mut instances = self.inner.instances.write().await;
        if let Some(instance) = instances.get_mut(id) {
            instance.attempt = attempt;
        }
    }

    async fn bump_attempt(&self, id: &InstanceId) -> Option<u32> {
        let mut instances = self.inner.instances.write().await;
        let instance = instances.get_mut(id)?;
        instance.attempt += 1;
        Some(instance.attempt)
    }

    async fn shutdown_of(&self, id: &InstanceId) -> ShutdownSpec {
        self.spec_of(id)
            .await
            .map(|spec| spec.shutdown)
            .unwrap_or_else(|_| ShutdownSpec {
                signal: StopSignal::Term,
                grace: FALLBACK_GRACE,
            })
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
        // Any state change ends whatever phase was being reported.
        instance.activity = None;
        if !to.is_running() {
            instance.pid = None;
        }
        // Emitted under the write lock so sequence order matches state order.
        self.inner
            .emit(Some(id.clone()), EventKind::StateChanged { from, to });
        Ok(())
    }
}

/// Points a step at its scratch directory instead of its final one.
fn substitute(value: &str, output: Option<&Path>) -> String {
    match output {
        Some(path) => value.replace("{output}", &path.display().to_string()),
        None => value.to_string(),
    }
}

#[derive(Clone, Copy)]
enum Action {
    Start,
    Stop,
}

/// Owns one running service for as long as it lives: it waits on the child,
/// reports the exit, and drives the restart policy until told to stop.
async fn supervise(
    engine: Engine,
    id: InstanceId,
    mut process: Process,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let exit = tokio::select! {
            exit = process.child.wait() => exit,
            _ = stop.changed() => {
                process.shut_down(&engine.shutdown_of(&id).await).await;
                return;
            }
        };

        process.drain().await;

        // A stop that landed in the same instant owns the rest of the story,
        // so a deliberate shutdown is never reported as a crash.
        if *stop.borrow() {
            return;
        }

        let Ok(spec) = engine.spec_of(&id).await else {
            return;
        };
        let (reason, crashed) = match exit {
            Ok(status) => (platform::describe_exit(&status), !status.success()),
            Err(error) => (format!("could not be waited on: {error}"), true),
        };

        // The reason is recorded before any restart is considered, which is
        // what the state machine guarantees for us.
        if engine
            .transition(&id, ServiceState::failed(reason))
            .await
            .is_err()
        {
            return;
        }

        let wanted = match spec.restart.policy {
            RestartPolicy::Never => false,
            RestartPolicy::Always => true,
            RestartPolicy::OnCrash => crashed,
        };
        if !wanted {
            return;
        }

        match engine.relaunch(&id, &mut stop).await {
            Some(next) => process = next,
            None => return,
        }
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
            activity: self.activity.clone(),
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
