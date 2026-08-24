//! A process the supervision tests can steer: it can hold a port, answer a
//! protocol, chatter on both output streams, and die on cue.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq)]
enum Speak {
    Nothing,
    Resp,
    Http,
}

struct Options {
    fail_if_exists: Option<String>,
    ignore_term: bool,
    listen: Option<u16>,
    listen_delay: Duration,
    speak: Speak,
    emit_every: Option<Duration>,
    exit_after: Option<Duration>,
    exit_code: i32,
}

fn main() {
    let options = match parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("fake-service: {message}");
            std::process::exit(2);
        }
    };

    // Lets a test make the first run succeed and every later one fail.
    if let Some(marker) = &options.fail_if_exists {
        if std::path::Path::new(marker).exists() {
            eprintln!("fake-service: refusing to start again");
            std::process::exit(1);
        }
        let _ = std::fs::write(marker, b"started");
    }

    if options.ignore_term {
        ignore_term();
    }

    if let Some(port) = options.listen {
        let (delay, speak) = (options.listen_delay, options.speak);
        thread::spawn(move || serve(port, delay, speak));
    }

    println!("ready pid={}", std::process::id());
    let _ = std::io::stdout().flush();

    if let Some(interval) = options.emit_every {
        thread::spawn(move || {
            for line in 1u64.. {
                println!("out {line}");
                eprintln!("err {line}");
                thread::sleep(interval);
            }
        });
    }

    match options.exit_after {
        Some(delay) => {
            thread::sleep(delay);
            eprintln!("fake-service: exiting with {}", options.exit_code);
            std::process::exit(options.exit_code);
        }
        None => loop {
            thread::sleep(Duration::from_secs(3600));
        },
    }
}

/// Binding late is what lets a test prove the engine waits for the port rather
/// than for the clock.
fn serve(port: u16, delay: Duration, speak: Speak) {
    thread::sleep(delay);
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("fake-service: cannot bind {port}: {error}");
            std::process::exit(1);
        }
    };
    for stream in listener.incoming().flatten() {
        answer(stream, speak);
    }
}

fn answer(mut stream: TcpStream, speak: Speak) {
    if speak == Speak::Nothing {
        return;
    }
    let mut request = [0u8; 512];
    let _ = stream.read(&mut request);
    let reply: &[u8] = match speak {
        Speak::Resp => b"+PONG\r\n",
        Speak::Http => b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
        Speak::Nothing => b"",
    };
    let _ = stream.write_all(reply);
}

/// Refuse to die politely, so the supervisor has to escalate.
#[cfg(unix)]
fn ignore_term() {
    // Safety: setting a handler to SIG_IGN touches only this process.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
}

#[cfg(not(unix))]
fn ignore_term() {
    eprintln!("fake-service: --ignore-term needs unix signals");
    std::process::exit(2);
}

fn parse(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        fail_if_exists: None,
        ignore_term: false,
        listen: None,
        listen_delay: Duration::ZERO,
        speak: Speak::Nothing,
        emit_every: None,
        exit_after: None,
        exit_code: 1,
    };
    let mut args = args.peekable();

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--fail-if-exists" => options.fail_if_exists = Some(text(&mut args, &flag)?),
            "--ignore-term" => options.ignore_term = true,
            "--listen" => options.listen = Some(number(&mut args, &flag)? as u16),
            "--listen-delay-ms" => {
                options.listen_delay = Duration::from_millis(number(&mut args, &flag)?)
            }
            "--speak" => {
                options.speak = match text(&mut args, &flag)?.as_str() {
                    "resp" => Speak::Resp,
                    "http" => Speak::Http,
                    other => return Err(format!("unknown dialect {other}")),
                }
            }
            "--emit-every-ms" => {
                options.emit_every = Some(Duration::from_millis(number(&mut args, &flag)?))
            }
            "--exit-after-ms" => {
                options.exit_after = Some(Duration::from_millis(number(&mut args, &flag)?))
            }
            "--exit-code" => options.exit_code = number(&mut args, &flag)? as i32,
            other => return Err(format!("unknown flag {other}")),
        }
    }

    Ok(options)
}

fn text(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("{flag} needs a value"))
}

fn number(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<u64, String> {
    text(args, flag)?
        .parse()
        .map_err(|_| format!("{flag} needs a number"))
}
