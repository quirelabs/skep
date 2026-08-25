//! The skep command line. A pure client: it never hosts the engine and never
//! starts one behind your back, because a background host nobody asked for is
//! how two of them end up racing.

mod serve;

use anyhow::{Result, anyhow, bail};
use comb::{Client, Error, InstanceId, Paths, Request, Response, ServiceStatus};
use comb_services::{instance as resolve_instance, project};

const USAGE: &str = "\
skep, a local dev services manager

usage:
  skep serve                  host the engine and every service, in the foreground
  skep up                     start everything skep.toml asks for
  skep status                 show every service
  skep start <service>        start a service
  skep stop <service>         stop a service
  skep restart <service>      restart a service
  skep logs <service> [-n N]  show the most recent output

A service is a name, or a name and a version: postgres, or postgres@16.10.0.
";

#[tokio::main]
async fn main() {
    if let Err(error) = dispatch().await {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

async fn dispatch() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".to_string());
    let rest: Vec<String> = args.collect();

    match command.as_str() {
        "serve" => serve::run().await,
        "up" => up().await,
        "status" => status().await,
        "start" => {
            act(Request::Start {
                instance: one(&rest)?,
            })
            .await
        }
        "stop" => {
            act(Request::Stop {
                instance: one(&rest)?,
            })
            .await
        }
        "restart" => {
            act(Request::Restart {
                instance: one(&rest)?,
            })
            .await
        }
        "logs" => logs(&rest).await,
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(())
        }
        "version" | "--version" => {
            println!("skep {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => bail!("unknown command {other}\n\n{USAGE}"),
    }
}

/// Turns `postgres` or `postgres@16` into the instance the engine knows.
fn resolve(name: &str) -> Result<InstanceId> {
    Ok(resolve_instance(name, None)?)
}

/// Brings up everything the project asks for. One service failing never stops
/// the others: a project half up with three clear errors beats one error and
/// no idea about the rest.
async fn up() -> Result<()> {
    let here = std::env::current_dir()?;
    let path = project::find(&here).ok_or_else(|| {
        anyhow!(
            "no {} here or in any parent of {}",
            project::FILE,
            here.display()
        )
    })?;
    let project = project::load(&path)?;
    println!("{}", path.display());
    if project.services.is_empty() {
        println!("  nothing to start");
        return Ok(());
    }

    let mut client = connect().await?;
    let Response::Status { snapshot } = client.send(&Request::Status).await? else {
        bail!("unexpected reply");
    };
    let running = snapshot.services;

    let mut failed = 0;
    for (name, wanted) in &project.services {
        match bring_up(&mut client, &running, name, wanted).await {
            Ok(line) => println!("  {line}"),
            Err(error) => {
                failed += 1;
                println!("  {name}: {error:#}");
            }
        }
    }

    if failed > 0 {
        bail!(
            "{failed} of {} services did not start",
            project.services.len()
        );
    }
    Ok(())
}

async fn bring_up(
    client: &mut Client,
    running: &[ServiceStatus],
    name: &str,
    wanted: &comb_services::project::Service,
) -> Result<String> {
    match comb_services::bring_up(client, name, wanted, running, &Paths::from_env()).await {
        Ok((id, outcome)) => Ok(format!("{id} {outcome}")),
        Err(sentence) => bail!(sentence),
    }
}

fn one(args: &[String]) -> Result<InstanceId> {
    match args.first() {
        Some(name) => resolve(name),
        None => bail!("which service?\n\n{USAGE}"),
    }
}

/// Connects, or explains that there is nothing to connect to. Ordering the
/// suggestions by what actually exists today; the app leads once it ships.
async fn connect() -> Result<Client> {
    match Client::connect(&Paths::from_env()).await {
        Ok(client) => Ok(client),
        Err(Error::NoHost) => bail!("no skep engine is running\n  start one with: skep serve"),
        Err(other) => Err(other.into()),
    }
}

async fn act(request: Request) -> Result<()> {
    match connect().await?.send(&request).await? {
        Response::Done => Ok(()),
        Response::Failed { message } => bail!(message),
        other => bail!("unexpected reply: {other:?}"),
    }
}

async fn status() -> Result<()> {
    let Response::Status { snapshot } = connect().await?.send(&Request::Status).await? else {
        bail!("unexpected reply");
    };
    let services = snapshot.services;
    if services.is_empty() {
        println!("no services are registered");
        return Ok(());
    }
    print!("{}", render(&services));
    Ok(())
}

async fn logs(args: &[String]) -> Result<()> {
    let instance = one(args)?;
    let lines = match args.iter().position(|arg| arg == "-n") {
        Some(at) => args
            .get(at + 1)
            .ok_or_else(|| anyhow!("-n needs a number"))?
            .parse()?,
        None => 50,
    };

    match connect()
        .await?
        .send(&Request::Logs { instance, lines })
        .await?
    {
        Response::Logs { lines } => {
            for line in lines {
                println!("{}", line.text);
            }
            Ok(())
        }
        Response::Failed { message } => bail!(message),
        other => bail!("unexpected reply: {other:?}"),
    }
}

fn render(services: &[ServiceStatus]) -> String {
    let rows: Vec<[String; 4]> = services
        .iter()
        .map(|service| {
            let ports = service
                .ports
                .values()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            // The phase matters more than the bare state during a long start.
            let state = match &service.activity {
                Some(activity) => format!("{} ({activity})", service.state),
                None => service.state.to_string(),
            };
            [
                service.id.to_string(),
                state,
                ports,
                service.pid.map(|pid| pid.to_string()).unwrap_or_default(),
            ]
        })
        .collect();

    let headers = ["SERVICE", "STATE", "PORTS", "PID"];
    let widths: Vec<usize> = (0..headers.len())
        .map(|column| {
            rows.iter()
                .map(|row| row[column].len())
                .chain(std::iter::once(headers[column].len()))
                .max()
                .unwrap_or(0)
        })
        .collect();

    let mut out = String::new();
    for (column, header) in headers.iter().enumerate() {
        out.push_str(&pad(header, widths[column], column == headers.len() - 1));
    }
    out.push('\n');
    for row in &rows {
        for (column, cell) in row.iter().enumerate() {
            out.push_str(&pad(cell, widths[column], column == row.len() - 1));
        }
        out.push('\n');
    }
    out
}

fn pad(cell: &str, width: usize, last: bool) -> String {
    if last {
        cell.to_string()
    } else {
        format!("{cell:<width$}  ")
    }
}
