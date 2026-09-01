//! The foreground host. Services live and die with it, so it is deliberately
//! something you can see running rather than a daemon that drifts away.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use comb::{Authority, Engine, Error, Host, Paths};
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

    serve_sites(&host).await?;

    host.serve(interrupted()).await?;
    println!("all services stopped.");
    Ok(())
}

/// Sites belong to the host rather than to any service, so they live and die
/// with it. The engine owns the map, and the same starter runs in the app.
async fn serve_sites(host: &Host) -> Result<()> {
    let paths = host.engine().paths().clone();
    let settings = comb_services::project::settings(&paths)?;
    host.engine().add_sites(comb_services::project::sites(
        &settings,
        &Default::default(),
    )?);

    let authority = Arc::new(Authority::open(&paths)?);
    let trusted = authority.is_trusted();
    let serving = comb::serve_alongside(host, authority, comb::SUFFIX).await;

    if serving.https.is_some() {
        if !trusted {
            println!("  the certificate authority is not trusted yet: run skep trust");
        }
        for (name, port) in host.engine().site_list() {
            println!(
                "  {} to port {port}",
                comb::site_url(&name, serving.public_https)
            );
        }
    }
    for trouble in &serving.trouble {
        println!("  {trouble}");
    }
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
