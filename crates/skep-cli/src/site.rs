//! Editing the file that declares sites. Config belongs to the person who
//! wrote it, so this changes the one table it was asked to and leaves every
//! comment, blank line and quirk of spacing exactly where it was.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use comb::{Paths, Request, Response};
use comb_services::project;
use toml_edit::{DocumentMut, Item, Table, value};

const TABLE: &str = "sites";

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
    // The same rule the certificate authority uses, so a name that could never
    // be served is refused before it reaches anybody's config file.
    comb::valid_hostname(host)?;

    let path = which(global)?;
    let mut document = read(&path)?;
    table(&mut document).insert(host, value(i64::from(port)));
    write(&path, &document)?;
    println!("{} now has {host} on port {port}", path.display());

    match tell(Request::AddSites {
        sites: [(host.to_string(), port)].into_iter().collect(),
    })
    .await
    {
        Ok(()) => println!("serving it now at https://{host}:{}", comb::HTTPS_PORT),
        Err(quiet) => println!("{quiet}"),
    }
    Ok(())
}

async fn remove(host: &str, global: bool) -> Result<()> {
    let path = which(global)?;
    let mut document = read(&path)?;
    if table(&mut document).remove(host).is_none() {
        bail!("{} does not mention {host}", path.display());
    }
    write(&path, &document)?;
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

fn read(path: &Path) -> Result<DocumentMut> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    Ok(text.parse::<DocumentMut>()?)
}

fn table(document: &mut DocumentMut) -> &mut Table {
    if !document.contains_key(TABLE) {
        let mut fresh = Table::new();
        // Written as [sites] rather than folded into a line, which is the
        // shape the settings template teaches.
        fresh.set_implicit(false);
        document.insert(TABLE, Item::Table(fresh));
    }
    document[TABLE]
        .as_table_mut()
        .expect("sites is a table because we just made it one")
}

fn write(path: &Path, document: &DocumentMut) -> Result<()> {
    std::fs::write(path, document.to_string())?;
    Ok(())
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
