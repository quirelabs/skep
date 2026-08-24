//! The foreground host. Services live and die with it, so it is deliberately
//! something you can see running rather than a daemon that drifts away.

use anyhow::{Context, Result};
use comb::{Engine, Host, Paths};
use comb_services::{Request, catalog};

pub async fn run() -> Result<()> {
    let engine = Engine::with_paths(Paths::from_env());

    // Every known service is registered, stopped. Starting one is then a
    // question for the engine rather than a registration dance per command.
    for adapter in catalog() {
        let spec = adapter
            .spec(&Request::new(), engine.paths())
            .with_context(|| format!("building the {} service", adapter.name()))?;
        engine.register(spec).await?;
    }

    let host = Host::claim(engine).await.context("claiming the machine")?;
    let services = host.engine().status().await.len();
    println!("skep is serving {services} services. Press ctrl-c to stop them and exit.");

    host.serve(interrupted()).await?;
    println!("all services stopped.");
    Ok(())
}

async fn interrupted() {
    let mut terminated =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminated.recv() => {}
    }
}
