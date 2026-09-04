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
    /// Write a site into skep's own settings and start serving it.
    AddSite(String, u16),
    /// Whether anything is actually listening behind each site.
    CheckSites,
    /// Remember one of the app's own preferences.
    Prefer(&'static str, bool),
    /// The projects this machine knows about, and what they are doing.
    Projects,
    /// Start what a project runs, on a port skep chooses.
    StartProject(String),
    /// Stop showing a project. Its files are untouched.
    ForgetProject(String),
    /// Take a directory to be a project, writing it a starting point if it
    /// has no skep.toml yet.
    AddProject(String),
    /// What the mail catcher has caught.
    Mail,
    /// One message, in full.
    ReadMail(String),
    ClearMail,
    /// Load this one message's remote images. Never remembered, never global.
    ShowImages(String),
    /// The message as it arrived.
    MailSource(String),
    /// How it would fare in real clients, and whether its links work. Asked
    /// for, because checking links reaches out over the network and a mail
    /// viewer that does that on its own would be lying about the sandbox.
    MailChecks(String),
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
    /// Which sites have something answering behind them.
    SiteHealth(std::collections::BTreeMap<String, bool>),
    /// What the mail catcher has caught, or why it could not be asked.
    Mail {
        messages: Vec<comb_services::mail::Summary>,
        unread: usize,
    },
    MailBody(Box<comb_services::mail::Body>),
    MailTrouble(String),
    /// Which authority this process is actually holding, and where it lives.
    /// Two homes make two of these with the same name, so the screen has to
    /// say which one rather than that there is one.
    Trust {
        home: String,
        root: String,
        fingerprint: String,
        trusted: bool,
    },
    /// Both name the message they are about, so an answer that arrives after
    /// the person has moved on lands nowhere rather than on the wrong one.
    MailSource {
        id: String,
        source: String,
    },
    MailChecks {
        id: String,
        checks: Box<(
            comb_services::mail::Compatibility,
            comb_services::mail::Links,
        )>,
    },
    /// The list changed and nothing else did: what was in the way still is.
    SiteList(std::collections::BTreeMap<String, u16>),
    /// A site could not be written down, and why.
    SiteRefused(String),
    /// The app's own preferences, as config.toml has them.
    Preferences {
        sites_in_browser: bool,
    },
    /// Every project this machine knows about.
    Projects(Vec<Project>),
    /// Every hostname this machine serves, and anything stopping it.
    Sites {
        sites: std::collections::BTreeMap<String, u16>,
        trouble: Vec<String>,
        trusted: bool,
        /// The port to show a person, which is 443 once the helper forwards it.
        public_https: u16,
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
    let _ = reports.send(Update::Trust {
        home: paths.root().display().to_string(),
        root: authority.root_file().display().to_string(),
        fingerprint: authority.fingerprint(),
        trusted,
    });
    let serving = comb::serve_alongside(host, std::sync::Arc::new(authority), comb::SUFFIX).await;

    let _ = reports.send(Update::Sites {
        sites: host.engine().site_list(),
        trouble: serving.trouble,
        trusted,
        public_https: serving.public_https,
    });
    tell_preferences(&paths, reports);
    let _ = reports.send(Update::Projects(known_projects(&paths)));
}

/// A project as the window shows it: what it is called, where it lives, and
/// the name it is served at. What it is doing comes from the replica, since
/// a project is an ordinary instance once it is running.
#[derive(Clone, Debug)]
pub struct Project {
    pub name: String,
    pub directory: String,
    pub site: Option<String>,
    /// None when the file is there but says nothing to run yet, which is
    /// what a project looks like the moment it is added.
    pub command: Option<String>,
}

/// Reads every remembered project's file. A directory that has been deleted
/// or a file that no longer parses is dropped from the list rather than
/// reported: the window shows what it can run.
fn known_projects(paths: &comb::Paths) -> Vec<Project> {
    let settings = comb_services::project::settings(paths).unwrap_or_default();
    settings
        .projects
        .paths
        .iter()
        .filter_map(|directory| {
            let file = std::path::Path::new(directory).join(comb_services::project::FILE);
            // A project whose file says nothing to run yet is still one: it
            // was just added, and the window is where it says what is
            // missing.
            let project = comb_services::project::load(&file).ok()?;
            Some(Project {
                name: comb_services::project_name_of(&file).ok()?.to_string(),
                directory: directory.clone(),
                site: project.run.as_ref().and_then(|run| run.site.clone()),
                command: project.run.as_ref().map(|run| run.command.shown()),
            })
        })
        .collect()
}

/// What config.toml says the app should do, sent the same way everything else
/// the window shows arrives.
fn tell_preferences(paths: &comb::Paths, reports: &UnboundedSender<Update>) {
    let settings = comb_services::project::settings(paths).unwrap_or_default();
    let _ = reports.send(Update::Preferences {
        sites_in_browser: settings.app.sites_in_browser,
    });
}

/// Asked of the engine rather than assumed, because a project is free to move
/// the port, and said in words a person can act on when it cannot be had.
async fn mail_port(engine: &Engine) -> std::result::Result<u16, String> {
    let services = engine.status().await;
    let Some(mailpit) = services
        .iter()
        .find(|service| service.id.service.as_str() == "mailpit")
    else {
        return Err("skep does not have mailpit".to_string());
    };
    if mailpit.state != comb::ServiceState::Ready {
        return Err(format!(
            "mailpit is {}, so nothing is catching mail",
            mailpit.state
        ));
    }
    mailpit
        .ports
        .get("http")
        .copied()
        .ok_or_else(|| "mailpit is running but has no http port".to_string())
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
        Command::Projects => {
            let _ = reports.send(Update::Projects(known_projects(engine.paths())));
            Ok(())
        }
        Command::AddProject(directory) => {
            let paths = engine.paths().clone();
            let outcome = comb_services::project::ensure_project(std::path::Path::new(&directory))
                .and_then(|_| comb_services::project::ensure_settings(&paths))
                .and_then(|settings| {
                    comb_services::project::remember_project(
                        &settings,
                        std::path::Path::new(&directory),
                    )
                });
            match outcome {
                Ok(_) => {
                    let _ = reports.send(Update::Projects(known_projects(&paths)));
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        Command::ForgetProject(directory) => {
            let paths = engine.paths().clone();
            if let Ok(settings) = comb_services::project::ensure_settings(&paths) {
                let _ = comb_services::project::forget_project(&settings, &directory);
            }
            let _ = reports.send(Update::Projects(known_projects(&paths)));
            Ok(())
        }
        Command::StartProject(directory) => {
            let paths = engine.paths().clone();
            let file = std::path::Path::new(&directory).join(comb_services::project::FILE);
            let outcome = comb_services::project::load(&file).and_then(|project| {
                let run = project.run.ok_or_else(|| {
                    comb::Error::InvalidId(format!("{directory} has no [run] to start"))
                })?;
                let spec = comb_services::run_spec(&file, &run, &paths)?;
                Ok((spec, run.site))
            });
            match outcome {
                Ok(((spec, port), site)) => {
                    let id = spec.id.clone();
                    // Upserted rather than registered: the port is new every
                    // time, so the spec it ran under last time is stale.
                    if let Err(error) = engine.upsert(spec).await {
                        Err(error)
                    } else if let Err(error) = engine.start(&id).await {
                        Err(error)
                    } else {
                        // The name follows the port that was actually taken,
                        // which is the whole reason a project runs this way.
                        if let Some(host) = site {
                            engine.add_sites([(host, port)].into_iter().collect());
                            let _ = reports.send(Update::Sites {
                                sites: engine.site_list(),
                                trouble: Vec::new(),
                                trusted: comb::Authority::open(&paths)
                                    .map(|authority| authority.is_trusted())
                                    .unwrap_or(false),
                                public_https: comb::public_https_port(
                                    &comb::Layout::system(comb::SUFFIX).control,
                                )
                                .await,
                            });
                        }
                        Ok(())
                    }
                }
                Err(error) => Err(error),
            }
        }
        Command::Prefer(name, value) => {
            let paths = engine.paths().clone();
            match comb_services::project::ensure_settings(&paths)
                .and_then(|path| comb_services::project::set_preference(&path, name, value))
            {
                Ok(()) => tell_preferences(&paths, reports),
                Err(error) => {
                    let _ = reports.send(Update::Failed(error.to_string()));
                }
            }
            Ok(())
        }
        Command::AddSite(host, port) => {
            let paths = engine.paths().clone();
            match comb_services::project::ensure_settings(&paths)
                .and_then(|path| comb_services::project::add_site(&path, &host, port))
            {
                Ok(()) => {
                    engine.add_sites([(host, port)].into_iter().collect());
                    let _ = reports.send(Update::SiteList(engine.site_list()));
                }
                Err(error) => {
                    let _ = reports.send(Update::SiteRefused(error.to_string()));
                }
            }
            Ok(())
        }
        Command::CheckSites => {
            let sites = engine.site_list();
            let mut answering = std::collections::BTreeMap::new();
            for (host, port) in sites {
                // A short reach to loopback: either something is holding the
                // port or it is not, and neither answer is worth waiting on.
                let alive = tokio::time::timeout(
                    std::time::Duration::from_millis(250),
                    tokio::net::TcpStream::connect(("127.0.0.1", port)),
                )
                .await
                .is_ok_and(|reached| reached.is_ok());
                answering.insert(host, alive);
            }
            let _ = reports.send(Update::SiteHealth(answering));
            Ok(())
        }
        Command::Mail => {
            match mail_port(engine).await {
                Ok(port) => match comb_services::mail::inbox(port, 100).await {
                    Ok((messages, unread)) => {
                        let _ = reports.send(Update::Mail { messages, unread });
                    }
                    Err(error) => {
                        let _ = reports.send(Update::MailTrouble(error.to_string()));
                    }
                },
                Err(why) => {
                    let _ = reports.send(Update::MailTrouble(why));
                }
            }
            Ok(())
        }
        Command::ReadMail(id) => {
            if let Ok(port) = mail_port(engine).await {
                match comb_services::mail::read(port, &id).await {
                    Ok(body) => {
                        let _ = reports.send(Update::MailBody(Box::new(body)));
                        // Opening a message is what makes it read. Mailpit
                        // only changes that when asked, and the list has to be
                        // asked again or the mark stays where it was.
                        let _ = comb_services::mail::mark_read(port, &id).await;
                        if let Ok((messages, unread)) = comb_services::mail::inbox(port, 100).await
                        {
                            let _ = reports.send(Update::Mail { messages, unread });
                        }
                    }
                    Err(error) => {
                        let _ = reports.send(Update::MailTrouble(error.to_string()));
                    }
                }
            }
            Ok(())
        }
        Command::ShowImages(id) => {
            if let Ok(port) = mail_port(engine).await
                && let Ok(body) = comb_services::mail::read_showing(
                    port,
                    &id,
                    comb_services::mail::Images::Allowed,
                )
                .await
            {
                let _ = reports.send(Update::MailBody(Box::new(body)));
            }
            Ok(())
        }
        Command::MailSource(id) => {
            if let Ok(port) = mail_port(engine).await {
                match comb_services::mail::source(port, &id).await {
                    Ok(source) => {
                        let _ = reports.send(Update::MailSource { id, source });
                    }
                    Err(error) => {
                        let _ = reports.send(Update::MailTrouble(error.to_string()));
                    }
                }
            }
            Ok(())
        }
        Command::MailChecks(id) => {
            if let Ok(port) = mail_port(engine).await {
                let clients = comb_services::mail::compatibility(port, &id).await;
                let links = comb_services::mail::links(port, &id).await;
                let _ = reports.send(Update::MailChecks {
                    id,
                    checks: Box::new((clients.unwrap_or_default(), links.unwrap_or_default())),
                });
            }
            Ok(())
        }
        Command::ClearMail => {
            if let Ok(port) = mail_port(engine).await {
                let _ = comb_services::mail::clear(port).await;
                if let Ok((messages, unread)) = comb_services::mail::inbox(port, 100).await {
                    let _ = reports.send(Update::Mail { messages, unread });
                }
            }
            Ok(())
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
