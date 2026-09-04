//! The skep command line. A pure client: it never hosts the engine and never
//! starts one behind your back, because a background host nobody asked for is
//! how two of them end up racing.

mod domains;
mod mail;
mod serve;
mod site;

use anyhow::{Result, anyhow, bail};
use comb::{Client, Error, InstanceId, Paths, Request, Response, ServiceStatus};
use comb_services::{instance as resolve_instance, project};

const USAGE: &str = "\
skep, a local dev services manager

usage:
  skep serve [--take-over]    host the engine and every service, in the foreground
  skep up                     start everything skep.toml asks for, including
                              the project's own dev server if it declares one
  skep status                 show every service
  skep start <service>        start a service
  skep stop <service>         stop a service
  skep restart <service>      restart a service
  skep logs <service> [-n N]  show the most recent output
  skep help                   show this

  skep trust                  trust skep's certificate authority, so local
                              domains can serve real https (asks for a password)
  skep untrust                stop trusting it
  skep sites                  every hostname this machine serves
  skep mail [query]           what the mail catcher caught
  skep mail read <id>         one message, in full
  skep mail clear             empty it
  skep site add <host> <port> give an app you already run a name and https
  skep site remove <host>     stop serving that name
                              (both edit skep.toml, or config.toml with --global)
  skep domains status         is local https actually working
  skep domains install        take ports 80 and 443 and route .test here
                              (needs sudo, and refuses to trample another tool)
  skep domains uninstall      give all of that back
  skep share <site|service>   put a site or a service on a public url
                              (a quick tunnel through cloudflared, http only,
                              no account needed; a site goes through the proxy)
  skep unshare <site|service> take it back off

  skep snapshot <service> <name>          keep a copy of a service's data
  skep snapshots <service>                list the copies kept
  skep branch <service> <label> [--from <name>]
                                          run a second copy on its own port
  skep branches                           list running branches
  skep delete branch <service>:<label>    remove a branch and its data
  skep delete snapshot <service> <name>   remove a kept copy

A branch is a sibling, not a child: it belongs to the service and version it
was copied from, so branching a branch gives another sibling.

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
        "serve" => serve::run(rest.iter().any(|arg| arg == "--take-over")).await,
        "up" => up().await,
        "status" => status().await,
        "start" => {
            let instance = named(&rest).await?;
            act(Request::Start { instance }).await
        }
        "stop" => {
            let instance = named(&rest).await?;
            act(Request::Stop { instance }).await
        }
        "restart" => {
            let instance = named(&rest).await?;
            act(Request::Restart { instance }).await
        }
        "logs" => logs(&rest).await,
        "domains" => domains::run(&rest).await,
        "mail" => mail::run(&rest).await,
        "sites" => sites().await,
        "site" => site::run(&rest).await,
        "share" => share(&rest).await,
        "unshare" => unshare(&rest).await,
        "trust" => trust(),
        "untrust" => untrust(),
        "snapshot" => snapshot(&rest).await,
        "snapshots" => snapshots(&rest).await,
        "branch" => branch(&rest).await,
        "branches" => branches().await,
        "delete" => delete(&rest).await,
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
/// Every hostname this machine serves, whether it came from settings or from
/// a project that ran skep up.
async fn sites() -> Result<()> {
    let mut client = connect().await?;
    let Response::Sites { sites } = client.send(&Request::Sites).await? else {
        bail!("unexpected reply");
    };
    if sites.is_empty() {
        println!("no sites. Add one to skep.toml or config.toml:");
        println!("\n  [sites]\n  \"myapp.test\" = 3000");
        return Ok(());
    }
    let public = comb::public_https_port(&comb::Layout::system(comb::SUFFIX).control).await;
    for (host, port) in sites {
        println!("{}  to port {port}", comb::site_url(&host, public));
    }
    Ok(())
}

/// Local work by design, so it needs no engine: the authority can be set up
/// before anything is running, and it outlives any particular host.
fn trust() -> Result<()> {
    let authority = comb::Authority::open(&Paths::from_env())?;
    let file = authority.root_file();
    if authority.is_trusted() {
        println!("already trusted: {}", file.display());
        return Ok(());
    }
    println!("trusting {}", file.display());
    println!("macOS will ask for your password, because this writes to the system keychain");
    authority.trust()?;
    println!("done, browsers on this machine will accept what skep issues");
    Ok(())
}

fn untrust() -> Result<()> {
    let authority = comb::Authority::open(&Paths::from_env())?;
    if !authority.is_trusted() {
        println!("not trusted, so there is nothing to remove");
        return Ok(());
    }
    authority.untrust()?;
    println!("removed, browsers will warn about local domains again");
    Ok(())
}

/// Starts what the project itself runs, and says where it can be reached.
/// Already running is not an error: the port it has is the port its name
/// should point at.
async fn start_project(
    client: &mut Client,
    file: &std::path::Path,
    run: &project::Run,
    running: &[comb::ServiceStatus],
) -> Result<(String, Option<(String, u16)>)> {
    let paths = Paths::from_env();
    let (spec, port) = comb_services::run_spec(file, run, &paths)?;
    let id = spec.id.clone();
    let site = run.site.clone();

    if let Some(status) = running.iter().find(|status| status.id == id)
        && (status.state.is_running() || status.state.is_transitional())
    {
        let held = status.ports.get("http").copied().unwrap_or(port);
        return Ok((
            format!("{id} already running on {held}"),
            site.map(|host| (host, held)),
        ));
    }

    for request in [
        Request::Register {
            spec: Box::new(spec),
        },
        Request::Start {
            instance: id.clone(),
        },
    ] {
        match client.send(&request).await? {
            Response::Done => {}
            Response::Failed { message } => bail!(message),
            other => bail!("unexpected reply: {other:?}"),
        }
    }
    Ok((
        format!("{id} started on {port}"),
        site.map(|host| (host, port)),
    ))
}

async fn share(args: &[String]) -> Result<()> {
    let Some(target) = args.first() else {
        bail!("share what? a site like myapp.test, or a service like mailpit\n\n{USAGE}");
    };
    let mut client = connect().await?;
    println!("asking the edge for a url");
    match comb_services::share(&mut client, target, &Paths::from_env()).await {
        Ok((id, url)) => {
            println!("{url}");
            println!("  serving {target} as {id}, until: skep unshare {target}");
            Ok(())
        }
        Err(message) => bail!(message),
    }
}

async fn unshare(args: &[String]) -> Result<()> {
    let Some(target) = args.first() else {
        bail!("unshare what?\n\n{USAGE}");
    };
    let mut client = connect().await?;
    let Response::Status { overview } = client.send(&Request::Status).await? else {
        bail!("unexpected reply");
    };
    let Response::Sites { sites } = client.send(&Request::Sites).await? else {
        bail!("unexpected reply");
    };
    let Some(id) = comb_services::shared_as(target, &sites, &overview.services) else {
        println!("{target} is not shared");
        return Ok(());
    };
    match client
        .send(&Request::Stop {
            instance: id.clone(),
        })
        .await?
    {
        Response::Done => {
            println!("{target} is no longer shared");
            Ok(())
        }
        Response::Failed { message } => bail!(message),
        other => bail!("unexpected reply: {other:?}"),
    }
}

fn resolve(name: &str) -> Result<InstanceId> {
    Ok(resolve_instance(name, None, &Paths::from_env())?)
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
    if project.services.is_empty() && project.sites.is_empty() && project.run.is_none() {
        println!("  nothing to start");
        return Ok(());
    }

    let mut client = connect().await?;
    let Response::Status { overview } = client.send(&Request::Status).await? else {
        bail!("unexpected reply");
    };
    let running = overview.services;

    // The project's own process, if it has one. Before the sites, because the
    // name it is served at points at a port that does not exist until it has
    // been started.
    let mut wanted_sites = project::sites(&Default::default(), &project)?;
    if let Some(run) = &project.run {
        // Worth remembering only once it is a project skep runs: a file that
        // only lists services is not something the window has anything to
        // show about.
        if let Some(directory) = path.parent()
            && let Ok(settings) = project::ensure_settings(&Paths::from_env())
            && project::remember_project(&settings, directory).unwrap_or(false)
        {
            println!("  remembered, so the app can start it too");
        }
        match start_project(&mut client, &path, run, &running).await {
            Ok((line, served)) => {
                println!("  {line}");
                if let Some((host, port)) = served {
                    wanted_sites.insert(host, port);
                }
            }
            Err(error) => println!("  the project itself: {error:#}"),
        }
    }

    // Sites next: they need nothing else running, and a name that cannot be
    // served is better said before a person waits for a database to boot.
    if !wanted_sites.is_empty() {
        let count = wanted_sites.len();
        match client
            .send(&Request::AddSites {
                sites: wanted_sites.into_iter().collect(),
            })
            .await?
        {
            Response::Sites { .. } => {
                println!("  {count} site{} served", if count == 1 { "" } else { "s" })
            }
            Response::Failed { message } => bail!(message),
            other => bail!("unexpected reply: {other:?}"),
        }
    }

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

/// The instance a name means, asking the engine when the catalog does not
/// know it. A project runs under its own name, which no catalog can contain,
/// and `skep stop myapp` should mean the obvious thing.
async fn named(args: &[String]) -> Result<InstanceId> {
    let Some(name) = args.first() else {
        bail!("which service?\n\n{USAGE}");
    };
    match resolve(name) {
        Ok(id) => Ok(id),
        Err(unknown) => {
            // Only ask if there is somebody to ask. With no engine running,
            // the catalog's own complaint is the better one: it lists what a
            // typo might have meant, where "no engine is running" does not.
            let Ok(mut client) = Client::connect(&Paths::from_env()).await else {
                return Err(unknown);
            };
            let Response::Status { overview } = client.send(&Request::Status).await? else {
                bail!("unexpected reply");
            };
            let (wanted, _) = name.split_once('@').unwrap_or((name.as_str(), ""));
            overview
                .services
                .into_iter()
                .map(|status| status.id)
                .find(|id| id.service.as_str() == wanted)
                .ok_or(unknown)
        }
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
    let Response::Status { overview } = connect().await?.send(&Request::Status).await? else {
        bail!("unexpected reply");
    };
    let services = overview.services;
    if services.is_empty() {
        println!("no services are registered");
        return Ok(());
    }
    print!("{}", render(&services));
    Ok(())
}

async fn logs(args: &[String]) -> Result<()> {
    let instance = named(args).await?;
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

async fn snapshot(args: &[String]) -> Result<()> {
    let (Some(service), Some(name)) = (args.first(), args.get(1)) else {
        bail!("which service, and what should the copy be called?\n\n{USAGE}");
    };
    let instance = resolve(service)?;
    // Taking a copy stops the service and starts it again, so say so rather
    // than appearing to hang.
    println!("stopping {instance} to copy its data");
    reply(
        connect()
            .await?
            .send(&Request::Snapshot {
                instance: instance.clone(),
                name: name.clone(),
            })
            .await?,
    )?;
    println!("{instance} snapshot {name}");
    Ok(())
}

async fn snapshots(args: &[String]) -> Result<()> {
    let instance = one(args)?;
    let Response::Snapshots { snapshots } = connect()
        .await?
        .send(&Request::Snapshots {
            instance: instance.clone(),
        })
        .await?
    else {
        bail!("unexpected reply");
    };
    if snapshots.is_empty() {
        println!("{instance} has no snapshots");
        return Ok(());
    }
    for kept in snapshots {
        println!("  {}", kept.name);
    }
    Ok(())
}

/// Creates a branch and starts it, because a branch nobody can connect to is
/// not much of a branch.
async fn branch(args: &[String]) -> Result<()> {
    let (Some(service), Some(label)) = (args.first(), args.get(1)) else {
        bail!("which service, and what should the branch be called?\n\n{USAGE}");
    };
    let from = resolve(service)?;
    let label = comb::Label::new(label.as_str())?;
    let source = args
        .iter()
        .position(|arg| arg == "--from")
        .map(|at| {
            args.get(at + 1)
                .cloned()
                .ok_or_else(|| anyhow!("--from needs the name of a snapshot"))
        })
        .transpose()?;

    let paths = Paths::from_env();
    let spec = comb_services::branch_spec(&from, &label, &paths)?;
    let id = spec.id.clone();
    let mut client = connect().await?;

    if source.is_none() {
        println!("stopping {from} to copy its data");
    }
    reply(
        client
            .send(&Request::Branch {
                from,
                spec: Box::new(spec),
                snapshot: source,
            })
            .await?,
    )?;
    reply(
        client
            .send(&Request::Start {
                instance: id.clone(),
            })
            .await?,
    )?;

    let Response::Status { overview } = client.send(&Request::Status).await? else {
        bail!("unexpected reply");
    };
    match overview.services.iter().find(|status| status.id == id) {
        Some(status) => println!("{id} {}", render_ports(status)),
        None => println!("{id}"),
    }
    Ok(())
}

async fn branches() -> Result<()> {
    let Response::Status { overview } = connect().await?.send(&Request::Status).await? else {
        bail!("unexpected reply");
    };
    let branches: Vec<ServiceStatus> = overview
        .services
        .into_iter()
        .filter(|status| status.id.is_branch())
        .collect();
    if branches.is_empty() {
        println!("no branches");
        return Ok(());
    }
    print!("{}", render(&branches));
    Ok(())
}

async fn delete(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("branch") => {
            let instance = one(&args[1..])?;
            reply(
                connect()
                    .await?
                    .send(&Request::RemoveBranch {
                        instance: instance.clone(),
                    })
                    .await?,
            )?;
            println!("deleted {instance}");
            Ok(())
        }
        Some("snapshot") => {
            let (Some(service), Some(name)) = (args.get(1), args.get(2)) else {
                bail!("which service, and which snapshot?\n\n{USAGE}");
            };
            let instance = resolve(service)?;
            reply(
                connect()
                    .await?
                    .send(&Request::RemoveSnapshot {
                        instance,
                        name: name.clone(),
                    })
                    .await?,
            )?;
            println!("deleted snapshot {name}");
            Ok(())
        }
        _ => bail!("delete what? a branch or a snapshot\n\n{USAGE}"),
    }
}

fn reply(response: Response) -> Result<()> {
    match response {
        Response::Done => Ok(()),
        Response::Failed { message } => bail!(message),
        other => bail!("unexpected reply: {other:?}"),
    }
}

fn render_ports(service: &ServiceStatus) -> String {
    service
        .ports
        .iter()
        .map(|(name, number)| match service.ports_from.get(name) {
            Some(source) => format!("{number} ({source})"),
            None => number.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn render(services: &[ServiceStatus]) -> String {
    let rows: Vec<[String; 4]> = services
        .iter()
        .map(|service| {
            // A number nobody chose needs no explanation. One that was chosen
            // says which file chose it, so a surprising port is never a
            // mystery.
            let ports = render_ports(service);
            // The phase matters more than the bare state during a long start.
            let state = match (&service.activity, &service.notice) {
                (Some(activity), _) => format!("{} ({activity})", service.state),
                (None, Some(notice)) => format!("{} {notice}", service.state),
                (None, None) => service.state.to_string(),
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
