//! The foreground host. Services live and die with it, so it is deliberately
//! something you can see running rather than a daemon that drifts away.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use comb::{Engine, Error, Host, Paths};
use comb_services::catalog;

pub async fn run(take_over: bool) -> Result<()> {
    let engine = Engine::with_paths(Paths::from_env());

    // Every known service is registered, stopped. Starting one is then a
    // question for the engine rather than a registration dance per command.
    for adapter in catalog() {
        let spec = comb_services::spec_default(adapter.name(), None, engine.paths())
            .with_context(|| format!("building the {} service", adapter.name()))?;
        engine.register(spec).await?;
    }

    // Taking over asks the running host to stop its services and stand down.
    // It never breaks the lock: the other host is doing real work and gets to
    // finish it.
    let host = if take_over {
        Host::take_over(engine, Duration::from_secs(60))
            .await
            .context("taking over the machine")?
    } else {
        match Host::claim(engine).await {
            Ok(host) => host,
            // Naming the pid is not enough. Say what to do about it.
            Err(Error::AlreadyHosted { pid }) => {
                let mut said = match pid {
                    Some(pid) => format!("another skep is running this machine (pid {pid})\n"),
                    None => "another skep is running this machine\n".to_string(),
                };
                if let Some(pid) = pid {
                    said.push_str(&format!("  stop it with:      kill {pid}\n"));
                }
                said.push_str("  or take it over:   skep serve --take-over\n");
                said.push_str(
                    "\nTaking over asks it to stop its services first, so nothing is left\n\
                     running without an owner.",
                );
                bail!(said);
            }
            Err(error) => return Err(error.into()),
        }
    };
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
