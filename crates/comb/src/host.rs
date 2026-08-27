//! Exactly one engine owns the machine's services. A frontend either becomes
//! that host or connects to the one already running.

use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;

use crate::engine::{Engine, Overview};
use crate::error::{Error, Result};
use crate::event::LogLine;
use crate::id::InstanceId;
use crate::paths::Paths;
use crate::platform;
use crate::spec::ServiceSpec;

/// Bumped whenever [`Request`] or [`Response`] changes shape.
pub const PROTOCOL: u32 = 1;

/// The socket carries the right to drive every service on the machine.
const SOCKET_MODE: u32 = 0o600;
/// How often a host looks for anything else holding the ports it wants.
const SURVEY: Duration = Duration::from_secs(10);
const RUN_DIR_MODE: u32 = 0o700;

/// The first line each side sends. Its shape must never change, or a version
/// mismatch stops being something either side can report.
#[derive(Debug, Serialize, Deserialize)]
struct Hello {
    protocol: u32,
    /// The host's state directory. A client building specs has to agree about
    /// where things live, and disagreeing quietly is worse than not starting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Refusal {
    error: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Greeting {
    Accepted(Hello),
    Refused(Refusal),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    Status,
    /// Asks the host to stop its services and exit, which is how a new host
    /// takes over: by asking, never by fighting for the lock.
    Quit,
    /// Teaches the host about a service, or updates a stopped one. Clients own
    /// the catalog; the host owns the processes.
    Register {
        spec: Box<ServiceSpec>,
    },
    Start {
        instance: InstanceId,
    },
    Stop {
        instance: InstanceId,
    },
    Restart {
        instance: InstanceId,
    },
    Logs {
        instance: InstanceId,
        lines: usize,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "response", rename_all = "snake_case")]
pub enum Response {
    Status { overview: Box<Overview> },
    Logs { lines: Vec<LogLine> },
    Done,
    Failed { message: String },
}

/// Proof that this process owns the machine's services. The kernel releases it
/// when the process exits, however it exits, so a crash cannot wedge the
/// machine. The pid written inside is diagnostic; the lock is the truth.
pub struct Lock {
    _file: File,
}

impl Lock {
    pub fn acquire(paths: &Paths) -> Result<Self> {
        let run = paths.run_dir();
        std::fs::create_dir_all(&run).map_err(Error::Io)?;
        platform::restrict(&run, RUN_DIR_MODE).map_err(Error::Io)?;

        let path = paths.lock_file();
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(Error::Io)?;

        if !platform::try_lock_exclusive(&file).map_err(Error::Io)? {
            return Err(Error::AlreadyHosted { pid: holder(&path) });
        }

        file.set_len(0).map_err(Error::Io)?;
        let _ = write!(file, "{}", std::process::id());
        let _ = file.flush();
        Ok(Self { _file: file })
    }
}

/// Only ever a hint for an error message. Whether a lock is held is answered
/// by flock, never by this.
fn holder(path: &PathBuf) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub struct Host {
    engine: Engine,
    listener: UnixListener,
    socket: PathBuf,
    quit: watch::Sender<bool>,
    _lock: Lock,
}

impl Host {
    /// Claims the machine and binds a fresh socket. Unlinking whatever was
    /// there is safe precisely because the lock already proved nobody is
    /// serving: a socket left by a crashed host is only a file.
    pub async fn claim(engine: Engine) -> Result<Self> {
        let paths = engine.paths().clone();
        let lock = Lock::acquire(&paths)?;

        let socket = paths.socket();
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).map_err(Error::Io)?;
        platform::restrict(&socket, SOCKET_MODE).map_err(Error::Io)?;

        Ok(Self {
            engine,
            listener,
            socket,
            quit: watch::channel(false).0,
            _lock: lock,
        })
    }

    /// Claims the machine, asking whoever holds it to stand down first. The
    /// previous host stops its own services on the way out, so nothing is left
    /// running without an owner.
    pub async fn take_over(engine: Engine, patience: Duration) -> Result<Self> {
        let paths = engine.paths().clone();
        match Self::claim(engine.clone()).await {
            Err(Error::AlreadyHosted { .. }) => {}
            other => return other,
        }

        if let Ok(mut client) = Client::connect(&paths).await {
            let _ = client.send(&Request::Quit).await;
        }

        // Wait for the lock to come free rather than trying to break it: the
        // previous host is stopping services and that takes as long as it takes.
        let deadline = Instant::now() + patience;
        loop {
            match Self::claim(engine.clone()).await {
                Err(Error::AlreadyHosted { pid }) if Instant::now() < deadline => {
                    let _ = pid;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                other => return other,
            }
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Serves until `until` resolves, then stops every service. In v1 services
    /// live and die with their host; a detached daemon is a later feature, not
    /// an accident of shutdown.
    pub async fn serve(self, until: impl Future<Output = ()>) -> Result<()> {
        tokio::pin!(until);
        let mut asked = self.quit.subscribe();
        // Whoever hosts, surveys. Otherwise an agent talking to a terminal
        // host would never learn what a window-hosted one would have told it.
        // The first tick is immediate.
        let mut sweep = tokio::time::interval(SURVEY);
        let mut surveying: Option<tokio::task::JoinHandle<()>> = None;
        loop {
            tokio::select! {
                _ = sweep.tick() => {
                    // Spawned, and never twice at once: looking up who holds a
                    // port shells out, and must not stall this loop.
                    if surveying.as_ref().is_none_or(|survey| survey.is_finished()) {
                        let engine = self.engine.clone();
                        surveying = Some(tokio::spawn(async move { engine.survey().await }));
                    }
                }
                accepted = self.listener.accept() => {
                    if let Ok((stream, _)) = accepted {
                        let engine = self.engine.clone();
                        let quit = self.quit.clone();
                        tokio::spawn(async move {
                            let _ = talk(engine, stream, quit).await;
                        });
                    }
                }
                _ = &mut until => break,
                _ = asked.changed() => break,
            }
        }

        let stopped = self.engine.stop_everything().await;
        let _ = std::fs::remove_file(&self.socket);
        stopped
    }
}

async fn talk(engine: Engine, stream: UnixStream, quit: watch::Sender<bool>) -> Result<()> {
    let mut stream = BufReader::new(stream);
    let mut line = String::new();

    // The handshake happens before anything else, so a version mismatch is a
    // sentence rather than a deserialize error further in.
    if stream.read_line(&mut line).await.map_err(Error::Io)? == 0 {
        return Ok(());
    }
    let spoken = serde_json::from_str::<Hello>(&line)
        .map(|hello| hello.protocol)
        .unwrap_or(0);
    if spoken != PROTOCOL {
        let refusal = Refusal {
            error: mismatch(spoken),
        };
        return send(&mut stream, &refusal).await;
    }
    send(
        &mut stream,
        &Hello {
            protocol: PROTOCOL,
            root: Some(engine.paths().root().to_path_buf()),
        },
    )
    .await?;

    loop {
        line.clear();
        if stream.read_line(&mut line).await.map_err(Error::Io)? == 0 {
            return Ok(());
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(Request::Quit) => {
                // Answer before winding down, so the asker learns it was heard.
                send(&mut stream, &Response::Done).await?;
                let _ = quit.send(true);
                return Ok(());
            }
            Ok(request) => answer(&engine, request).await,
            Err(error) => Response::Failed {
                message: format!("unintelligible request: {error}"),
            },
        };
        send(&mut stream, &response).await?;
    }
}

async fn answer(engine: &Engine, request: Request) -> Response {
    let outcome = match request {
        Request::Status => {
            return Response::Status {
                overview: Box::new(engine.overview().await),
            };
        }
        Request::Logs { instance, lines } => {
            return match engine.logs(&instance, lines).await {
                Ok(lines) => Response::Logs { lines },
                Err(error) => Response::Failed {
                    message: error.to_string(),
                },
            };
        }
        // Handled before it reaches here, where the connection is in scope.
        Request::Quit => Ok(()),
        Request::Register { spec } => engine.upsert(*spec).await,
        Request::Start { instance } => engine.start(&instance).await,
        Request::Stop { instance } => engine.stop(&instance).await,
        Request::Restart { instance } => engine.restart(&instance).await,
    };
    match outcome {
        Ok(()) => Response::Done,
        Err(error) => Response::Failed {
            message: error.to_string(),
        },
    }
}

async fn send<T: Serialize>(stream: &mut BufReader<UnixStream>, message: &T) -> Result<()> {
    let mut line = serde_json::to_string(message).map_err(|error| Error::Protocol {
        message: error.to_string(),
    })?;
    line.push('\n');
    stream
        .get_mut()
        .write_all(line.as_bytes())
        .await
        .map_err(Error::Io)
}

fn mismatch(spoken: u32) -> String {
    format!(
        "this client speaks protocol {spoken}, the running engine speaks {PROTOCOL}. \
         Quit and reopen Skep, and start a new terminal, so both are the same version."
    )
}

/// A frontend talking to whichever process is hosting the engine.
pub struct Client {
    stream: BufReader<UnixStream>,
}

impl Client {
    pub async fn connect(paths: &Paths) -> Result<Self> {
        let stream = UnixStream::connect(paths.socket())
            .await
            .map_err(|_| Error::NoHost)?;
        let mut client = Self {
            stream: BufReader::new(stream),
        };

        send(
            &mut client.stream,
            &Hello {
                protocol: PROTOCOL,
                root: None,
            },
        )
        .await?;
        match client.read::<Greeting>().await? {
            Greeting::Accepted(hello) if hello.protocol == PROTOCOL => {
                let ours = paths.root();
                match hello.root {
                    Some(theirs) if theirs != ours => Err(Error::Protocol {
                        message: format!(
                            "the running engine keeps its state in {}, this command expects {}. \
                             Check SKEP_HOME.",
                            theirs.display(),
                            ours.display()
                        ),
                    }),
                    _ => Ok(client),
                }
            }
            Greeting::Accepted(hello) => Err(Error::Protocol {
                message: mismatch(hello.protocol),
            }),
            Greeting::Refused(refusal) => Err(Error::Protocol {
                message: refusal.error,
            }),
        }
    }

    pub async fn send(&mut self, request: &Request) -> Result<Response> {
        send(&mut self.stream, request).await?;
        self.read().await
    }

    async fn read<T: for<'de> Deserialize<'de>>(&mut self) -> Result<T> {
        let mut line = String::new();
        if self.stream.read_line(&mut line).await.map_err(Error::Io)? == 0 {
            return Err(Error::Protocol {
                message: "the engine closed the connection".to_string(),
            });
        }
        serde_json::from_str(&line).map_err(|error| Error::Protocol {
            message: format!("unintelligible reply: {error}"),
        })
    }
}
