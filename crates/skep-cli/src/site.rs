//! Editing the file that declares sites. Config belongs to the person who
//! wrote it, so this changes the one table it was asked to and leaves every
//! comment, blank line and quirk of spacing exactly where it was.

use std::path::PathBuf;

use anyhow::{Result, bail};
use comb::{Paths, Request, Response};
use comb_services::project;

pub async fn run(args: &[String]) -> Result<()> {
    let global = args.iter().any(|arg| arg == "--global");
    let rest: Vec<&String> = args
        .iter()
        .filter(|arg| !arg.starts_with("--"))
        .skip(1)
        .collect();

    match args.first().map(String::as_str) {
        Some("add") => {
            let [host, port] = rest[..] else {
                bail!("skep site add <host> <port>");
            };
            let port: u16 = port
                .parse()
                .map_err(|_| anyhow::anyhow!("{port} is not a port"))?;
            add(host, port, global).await
        }
        Some("remove") => {
            let [host] = rest[..] else {
                bail!("skep site remove <host>");
            };
            remove(host, global).await
        }
        Some(other) => bail!("skep site {other}? try add or remove"),
        None => bail!("skep site add <host> <port>, or skep site remove <host>"),
    }
}

async fn add(host: &str, port: u16, global: bool) -> Result<()> {
    let path = which(global)?;
    project::add_site(&path, host, port)?;
    println!("{} now has {host} on port {port}", path.display());

    match tell(Request::AddSites {
        sites: [(host.to_string(), port)].into_iter().collect(),
    })
    .await
    {
        Ok(()) => {
            let public = comb::public_https_port(&comb::Layout::system(comb::SUFFIX).control).await;
            println!("serving it now at {}", comb::site_url(host, public));
        }
        Err(quiet) => println!("{quiet}"),
    }
    Ok(())
}

async fn remove(host: &str, global: bool) -> Result<()> {
    let path = which(global)?;
    if !project::remove_site(&path, host)? {
        bail!("{} does not mention {host}", path.display());
    }
    println!("{} no longer has {host}", path.display());

    match tell(Request::RemoveSite {
        host: host.to_string(),
    })
    .await
    {
        Ok(()) => println!("stopped serving it"),
        Err(quiet) => println!("{quiet}"),
    }
    Ok(())
}

/// A project's file when standing in one, skep's own otherwise. Sites in a
/// repository belong to the repository.
fn which(global: bool) -> Result<PathBuf> {
    if global {
        let path = Paths::from_env().config_file();
        project::ensure_settings(&Paths::from_env())?;
        return Ok(path);
    }
    match project::find(&std::env::current_dir()?) {
        Some(path) => Ok(path),
        None => bail!(
            "no {} here or in any parent. Write one, or use --global to put \
             the site in skep's own settings.",
            project::FILE
        ),
    }
}

/// Best effort: editing the file is the command's job, and a host that is not
/// running is a note rather than a failure.
async fn tell(request: Request) -> Result<(), String> {
    let mut client = comb::Client::connect(&Paths::from_env())
        .await
        .map_err(|_| "no engine is running, so it will be served when one starts".to_string())?;
    match client.send(&request).await {
        Ok(Response::Sites { .. }) => Ok(()),
        Ok(Response::Failed { message }) => Err(message),
        Ok(_) => Err("the engine said something unexpected".to_string()),
        Err(error) => Err(error.to_string()),
    }
}
