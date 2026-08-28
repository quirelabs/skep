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

/// Sites are a machine-level feature, so they belong to the host rather than
/// to any service, and they stop when it does.
async fn serve_sites(host: &Host) -> Result<()> {
    let paths = host.engine().paths().clone();
    let settings = comb_services::project::settings(&paths)?;
    let sites = comb_services::project::sites(&settings, &Default::default())?;
    if sites.is_empty() {
        return Ok(());
    }

    let authority = Arc::new(Authority::open(&paths)?);
    if !authority.is_trusted() {
        println!("  the certificate authority is not trusted yet: run skep trust");
    }

    let https = tokio::net::TcpListener::bind(("127.0.0.1", comb::HTTPS_PORT))
        .await
        .with_context(|| format!("binding port {}", comb::HTTPS_PORT))?;
    let http = tokio::net::TcpListener::bind(("127.0.0.1", comb::HTTP_PORT))
        .await
        .with_context(|| format!("binding port {}", comb::HTTP_PORT))?;

    for (host_name, port) in &sites {
        println!("  https://{host_name}:{} to port {port}", comb::HTTPS_PORT);
    }

    let mut quitting = host.quitting();
    let sites = Arc::new(sites);
    tokio::spawn(comb::serve_sites(https, sites, authority, async move {
        let _ = quitting.changed().await;
    }));
    let mut quitting = host.quitting();
    tokio::spawn(comb::redirect(http, comb::HTTPS_PORT, async move {
        let _ = quitting.changed().await;
    }));
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
