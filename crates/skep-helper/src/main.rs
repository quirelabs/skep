//! The only part of skep that runs as root, and it does one dumb thing: hold
//! ports 80 and 443 and pass what arrives to the engine's own ports.
//!
//! It speaks no TLS and parses no http. Everything that could be got wrong
//! about a request happens in the unprivileged proxy, and root is given away
//! within milliseconds of the ports being bound. What is left is a pipe.

use std::path::PathBuf;
use std::process::ExitCode;

use comb::{Forward, Health, Hello, Owner};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UnixListener};

const USAGE: &str = "\
skep-helper, which holds the ports skep cannot

usage:
  skep-helper --control <socket> --user <uid> --group <gid> --forward <from>:<to> ...

Installed and removed by skep domains install and skep domains uninstall.
Running it by hand is not useful.
";

fn main() -> ExitCode {
    let settings = match Settings::from_args() {
        Ok(settings) => settings,
        Err(complaint) => {
            eprintln!("{complaint}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("no runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(settings)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(complaint) => {
            eprintln!("{complaint}");
            ExitCode::FAILURE
        }
    }
}

struct Settings {
    control: PathBuf,
    owner: Owner,
    forwards: Vec<Forward>,
}

impl Settings {
    fn from_args() -> Result<Self, String> {
        let mut control = None;
        let mut uid = None;
        let mut gid = None;
        let mut forwards = Vec::new();

        let mut args = std::env::args().skip(1);
        while let Some(flag) = args.next() {
            let mut value = || args.next().ok_or(format!("{flag} needs a value"));
            match flag.as_str() {
                "--control" => control = Some(PathBuf::from(value()?)),
                "--user" => uid = Some(value()?.parse().map_err(|_| "--user takes a number")?),
                "--group" => gid = Some(value()?.parse().map_err(|_| "--group takes a number")?),
                "--forward" => {
                    let pair = value()?;
                    let (from, to) = pair
                        .split_once(':')
                        .ok_or(format!("--forward wants from:to, got {pair}"))?;
                    forwards.push(Forward {
                        from: from.parse().map_err(|_| format!("{from} is not a port"))?,
                        to: to.parse().map_err(|_| format!("{to} is not a port"))?,
                    });
                }
                other => return Err(format!("unknown option {other}")),
            }
        }

        if forwards.is_empty() {
            return Err("nothing to forward".to_string());
        }
        Ok(Self {
            control: control.ok_or("--control is required")?,
            owner: Owner {
                uid: uid.ok_or("--user is required")?,
                gid: gid.ok_or("--group is required")?,
            },
            forwards,
        })
    }
}

async fn run(settings: Settings) -> Result<(), String> {
    // Everything that needs root happens here, before anything else at all.
    let mut listeners = Vec::new();
    for forward in &settings.forwards {
        let listener = TcpListener::bind(("127.0.0.1", forward.from))
            .await
            .map_err(|error| format!("could not take port {}: {error}", forward.from))?;
        listeners.push((listener, forward.to));
    }

    let _ = std::fs::remove_file(&settings.control);
    if let Some(parent) = settings.control.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let control = UnixListener::bind(&settings.control)
        .map_err(|error| format!("could not open the control socket: {error}"))?;

    // The socket outlives root, so it has to belong to whoever is left.
    comb::hand_over(&settings.control, settings.owner)
        .map_err(|error| format!("could not hand over the control socket: {error}"))?;

    // And root ends here.
    comb::become_user(settings.owner)
        .map_err(|error| format!("could not give up root: {error}"))?;

    let forwards = settings.forwards.clone();
    tokio::spawn(async move {
        loop {
            let Some((stream, _)) = accepted(control.accept().await).await else {
                continue;
            };
            let forwards = forwards.clone();
            tokio::spawn(async move {
                let _ = answer(stream, forwards).await;
            });
        }
    });

    let mut serving = Vec::new();
    for (listener, target) in listeners {
        serving.push(tokio::spawn(async move {
            loop {
                let Some((incoming, _)) = accepted(listener.accept().await).await else {
                    continue;
                };
                tokio::spawn(async move {
                    let _ = pipe(incoming, target).await;
                });
            }
        }));
    }
    for task in serving {
        let _ = task.await;
    }
    Ok(())
}

/// An accept that failed, on descriptors or an aborted handshake, is not a
/// reason to stop serving the port: a pause and another try, never an exit
/// that leaves launchd restarting the helper in a loop.
async fn accepted<T>(result: std::io::Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(_) => {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            None
        }
    }
}

/// The whole job: copy bytes both ways until one end stops.
async fn pipe(mut incoming: TcpStream, target: u16) -> std::io::Result<()> {
    let mut onward = TcpStream::connect(("127.0.0.1", target)).await?;
    tokio::io::copy_bidirectional(&mut incoming, &mut onward).await?;
    Ok(())
}

/// The version goes first, the same way it does on the engine's socket, so a
/// helper left behind by an older install says so rather than misbehaving.
async fn answer(stream: tokio::net::UnixStream, forwarding: Vec<Forward>) -> std::io::Result<()> {
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    if stream.read_line(&mut line).await? == 0 {
        return Ok(());
    }
    if serde_json::from_str::<Hello>(line.trim()).is_err() {
        return Ok(());
    }

    let health = Health {
        protocol: comb::HELPER_PROTOCOL,
        pid: std::process::id(),
        forwarding,
    };
    let said = serde_json::to_string(&health).unwrap_or_default();
    stream
        .get_mut()
        .write_all(format!("{said}\n").as_bytes())
        .await
}
