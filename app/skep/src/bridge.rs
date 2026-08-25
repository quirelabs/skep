//! The seam between the engine, which lives in a tokio runtime, and the
//! interface, which lives on GPUI's main thread. Everything crosses as a
//! message; neither side reaches into the other.

use comb::{Engine, Event, Host, InstanceId, Snapshot};
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
}

/// What the engine reports.
pub enum Update {
    /// Everything at an instant, stamped so the replica knows what it covers.
    Snapshot(Box<Snapshot>),
    Event(Box<Event>),
    /// This process now owns the machine's services.
    Hosting,
    /// Someone else does, and this is who.
    Blocked {
        pid: Option<u32>,
    },
    Failed(String),
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
    let _ = reports.send(Update::Snapshot(Box::new(engine.snapshot().await)));

    loop {
        tokio::select! {
            order = orders.recv() => {
                let Some(order) = order else { return };
                // Never awaited here. A start that has to download a hundred
                // megabytes takes a minute, and running it inline would stop
                // this loop forwarding the very events that say so.
                let engine = engine.clone();
                let reports = reports.clone();
                tokio::spawn(async move { act(&engine, order, &reports).await });
            }
            event = events.recv() => match event {
                Ok(event) => {
                    let _ = reports.send(Update::Event(Box::new(event)));
                }
                // Falling behind is answered with the truth, not a guess.
                Err(RecvError::Lagged(_)) => {
                    let _ = reports.send(Update::Snapshot(Box::new(engine.snapshot().await)));
                }
                Err(RecvError::Closed) => return,
            },
        }
    }
}

async fn act(engine: &Engine, order: Command, reports: &UnboundedSender<Update>) {
    let outcome = match order {
        Command::Start(id) => engine.start(&id).await,
        Command::Stop(id) => engine.stop(&id).await,
        Command::Restart(id) => engine.restart(&id).await,
        Command::Resync => {
            let _ = reports.send(Update::Snapshot(Box::new(engine.snapshot().await)));
            return;
        }
        Command::TakeOver => {
            match Host::take_over(engine.clone(), std::time::Duration::from_secs(30)).await {
                Ok(host) => {
                    let _ = reports.send(Update::Hosting);
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
