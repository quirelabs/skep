//! A process the supervision tests can steer: it can hold a port, chatter on
//! both output streams, and die on cue. Deliberately dependency free.

use std::io::Write;
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

struct Options {
    ignore_term: bool,
    listen: Option<u16>,
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

    if options.ignore_term {
        ignore_term();
    }

    if let Some(port) = options.listen {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                thread::spawn(move || {
                    for stream in listener.incoming() {
                        drop(stream);
                    }
                });
            }
            Err(error) => {
                eprintln!("fake-service: cannot bind {port}: {error}");
                std::process::exit(1);
            }
        }
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
        ignore_term: false,
        listen: None,
        emit_every: None,
        exit_after: None,
        exit_code: 1,
    };
    let mut args = args.peekable();

    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{flag} needs a value"))?
                .parse::<u64>()
                .map_err(|_| format!("{flag} needs a number"))
        };
        match flag.as_str() {
            "--ignore-term" => options.ignore_term = true,
            "--listen" => options.listen = Some(value()? as u16),
            "--emit-every-ms" => options.emit_every = Some(Duration::from_millis(value()?)),
            "--exit-after-ms" => options.exit_after = Some(Duration::from_millis(value()?)),
            "--exit-code" => options.exit_code = value()? as i32,
            other => return Err(format!("unknown flag {other}")),
        }
    }

    Ok(options)
}
