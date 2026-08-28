//! The seam between the engine, which lives in a tokio runtime, and the
//! interface, which lives on GPUI's main thread. Everything crosses as a
//! message; neither side reaches into the other.

use comb::{Engine, Event, Host, InstanceId, Label, LogLine, Overview, Snapshot};
use tokio::runtime::Runtime;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// What the interface asks for.
#[derive(Clone)]
pub enum Command {
    Start(InstanceId),
    Stop(InstanceId),
    Restart(InstanceId),
    /// Send a fresh snapshot: the replica has fallen behind.
    Resync,
    /// Ask whoever holds the machine to stand down, then take it.
    TakeOver,
    /// Follow one service's output, or nothing.
    Watch(Option<InstanceId>),
    Snapshot(InstanceId, String),
    Snapshots(InstanceId),
    RemoveSnapshot(InstanceId, String),
    Branch(InstanceId, Label, Option<String>),
    RemoveBranch(InstanceId),
}

/// What the engine reports.
pub enum Update {
    /// Everything at an instant, stamped so the replica knows what it covers.
    Overview(Box<Overview>),
    Event(Box<Event>),
    /// This process now owns the machine's services.
    Hosting,
    /// Someone else does, and this is who.
    Blocked {
        pid: Option<u32>,
    },
    Failed(String),
    /// Every hostname this machine serves, and anything stopping it.
    Sites {
        sites: std::collections::BTreeMap<String, u16>,
        trouble: Vec<String>,
        trusted: bool,
    },
    /// The tail that was already there when watching began.
    Logs(Vec<LogLine>),
    /// One line, as it is written.
    Log(Box<LogLine>),
    /// What has been kept for the service being looked at.
    Kept(Vec<Snapshot>),
}

pub struct Bridge {
    pub commands: UnboundedSender<Command>,
    pub updates: UnboundedReceiver<Update>,
}

pub fn start(runtime: &Runtime, engine: Engine) -> Bridge {
    let (commands, orders) = unbounded_channel();
    let (reports, updates) = unbounded_channel();

    runtime.spawn(run(engine, orders, reports));
    Bridge { commands, updates }
}

async fn run(
    engine: Engine,
    mut orders: UnboundedReceiver<Command>,
    reports: UnboundedSender<Update>,
) {
    match Host::claim(engine.clone()).await {
        Ok(host) => {
            let _ = reports.send(Update::Hosting);
            start_sites(&host, &reports).await;
            tokio::spawn(async move {
                let _ = host.serve(std::future::pending()).await;
            });
        }
        Err(comb::Error::AlreadyHosted { pid }) => {
            let _ = reports.send(Update::Blocked { pid });
        }
        Err(error) => {
            let _ = reports.send(Update::Failed(error.to_string()));
        }
    }

    // Subscribe before the first snapshot, so an event that lands between the
    // two is buffered rather than lost. The replica discards what the snapshot
    // already covers.
    let mut events = engine.subscribe_events();
    let _ = reports.send(Update::Overview(Box::new(engine.overview().await)));
    let mut watching: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        tokio::select! {
            order = orders.recv() => {
                let Some(order) = order else { return };
                match order {
                    // Watching is a subscription rather than a task to run, so
                    // it is swapped here rather than spawned alongside.
                    Command::Watch(target) => {
                        if let Some(previous) = watching.take() {
                            previous.abort();
                        }
                        watching = target
                            .map(|id| tokio::spawn(follow(engine.clone(), id, reports.clone())));
                    }
                    // Never awaited here. A start that has to download a
                    // hundred megabytes takes a minute, and running it inline
                    // would stop this loop forwarding the events that say so.
                    order => {
                        let engine = engine.clone();
                        let reports = reports.clone();
                        tokio::spawn(async move { act(&engine, order, &reports).await });
                    }
                }
            }
            event = events.recv() => match event {
                Ok(event) => {
                    let _ = reports.send(Update::Event(Box::new(event)));
                }
                // Falling behind is answered with the truth, not a guess.
                Err(RecvError::Lagged(_)) => {
                    let _ = reports.send(Update::Overview(Box::new(engine.overview().await)));
                }
                Err(RecvError::Closed) => return,
            },
        }
    }
}

/// The tail that exists, then everything written from now on. Subscribing
/// before reading the tail means a line written in between is repeated rather
/// than lost, which is the better of the two.
/// Local domains, started the moment this process owns the machine. The same
/// call the command line makes, so the two cannot drift apart.
async fn start_sites(host: &comb::Host, reports: &UnboundedSender<Update>) {
    let paths = host.engine().paths().clone();
    if let Ok(settings) = comb_services::project::settings(&paths)
        && let Ok(wanted) = comb_services::project::sites(&settings, &Default::default())
    {
        host.engine().add_sites(wanted);
    }

    let Ok(authority) = comb::Authority::open(&paths) else {
        return;
    };
    let trusted = authority.is_trusted();
    let serving = comb::serve_alongside(host, std::sync::Arc::new(authority), comb::SUFFIX).await;

    let _ = reports.send(Update::Sites {
        sites: host.engine().site_list(),
        trouble: serving.trouble,
        trusted,
    });
}

async fn follow(engine: Engine, id: InstanceId, reports: UnboundedSender<Update>) {
    let stream = engine.subscribe_logs(&id).await;
    if let Ok(tail) = engine.logs(&id, 300).await {
        let _ = reports.send(Update::Logs(tail));
    }
    let Ok(mut stream) = stream else { return };
    while let Ok(line) = stream.recv().await {
        let _ = reports.send(Update::Log(Box::new(line)));
    }
}

async fn act(engine: &Engine, order: Command, reports: &UnboundedSender<Update>) {
    let outcome = match order {
        Command::Start(id) => engine.start(&id).await,
        Command::Stop(id) => engine.stop(&id).await,
        Command::Restart(id) => engine.restart(&id).await,
        Command::Resync => {
            let _ = reports.send(Update::Overview(Box::new(engine.overview().await)));
            return;
        }
        Command::Watch(_) => return,
        Command::Snapshot(id, name) => {
            let taken = engine.snapshot(&id, &name).await;
            // The list is what the interface shows, so refresh it either way.
            if let Ok(kept) = engine.snapshots(&id).await {
                let _ = reports.send(Update::Kept(kept));
            }
            taken
        }
        Command::Snapshots(id) => {
            if let Ok(kept) = engine.snapshots(&id).await {
                let _ = reports.send(Update::Kept(kept));
            }
            Ok(())
        }
        Command::RemoveSnapshot(id, name) => {
            let removed = engine.remove_snapshot(&id, &name).await;
            if let Ok(kept) = engine.snapshots(&id).await {
                let _ = reports.send(Update::Kept(kept));
            }
            removed
        }
        Command::Branch(from, label, snapshot) => {
            match comb_services::branch_spec(&from, &label, engine.paths()) {
                Ok(spec) => {
                    let id = spec.id.clone();
                    match engine.branch(&from, spec, snapshot.as_deref()).await {
                        // A branch nobody can connect to is not much of a branch.
                        Ok(_) => engine.start(&id).await,
                        Err(error) => Err(error),
                    }
                }
                Err(error) => Err(error),
            }
        }
        Command::RemoveBranch(id) => engine.remove_branch(&id).await,
        Command::TakeOver => {
            match Host::take_over(engine.clone(), std::time::Duration::from_secs(30)).await {
                Ok(host) => {
                    let _ = reports.send(Update::Hosting);
                    start_sites(&host, reports).await;
                    tokio::spawn(async move {
                        let _ = host.serve(std::future::pending()).await;
                    });
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
    };

    // A failure is a sentence the engine already wrote. It reaches the row it
    // belongs to through the service's own state, so nothing is invented here.
    if let Err(error) = outcome {
        let _ = reports.send(Update::Failed(error.to_string()));
    }
}
