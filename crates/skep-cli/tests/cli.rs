//! Drives the real binary. The CLI is a pure client, so the interesting cases
//! are the ones where there is no engine to talk to.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn skep() -> Command {
    Command::new(env!("CARGO_BIN_EXE_skep"))
}

struct Home(PathBuf);

impl Home {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("skep-cli-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn without_an_engine_it_says_how_to_start_one() {
    let home = Home::new("no-engine");
    let output = skep()
        .arg("status")
        .env("SKEP_HOME", &home.0)
        .output()
        .unwrap();

    assert!(!output.status.success(), "should have failed");
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("no skep engine is running"), "{message}");
    assert!(
        message.contains("skep serve"),
        "it should say what to do: {message}"
    );
}

#[test]
fn an_unknown_service_lists_the_known_ones() {
    let home = Home::new("unknown");
    let output = skep()
        .args(["start", "redis"])
        .env("SKEP_HOME", &home.0)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("unknown service redis"), "{message}");
    assert!(message.contains("postgres"), "{message}");
}

#[test]
fn it_hosts_answers_and_winds_down() {
    let home = Home::new("round-trip");
    let mut serving = skep()
        .arg("serve")
        .env("SKEP_HOME", &home.0)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    // Wait for the socket rather than for the clock.
    let socket = home.0.join("run").join("engine.sock");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(socket.exists(), "the host never bound its socket");

    let listed = skep()
        .arg("status")
        .env("SKEP_HOME", &home.0)
        .output()
        .unwrap();
    let table = String::from_utf8_lossy(&listed.stdout);
    assert!(listed.status.success(), "{table}");
    assert!(table.contains("mailpit"), "{table}");
    assert!(table.contains("stopped"), "{table}");

    // A second host must not be able to take the machine.
    let refused = skep()
        .arg("serve")
        .env("SKEP_HOME", &home.0)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("already hosting"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );

    unsafe { libc::kill(serving.id() as i32, libc::SIGTERM) };
    let finished = serving.wait().unwrap();
    assert!(finished.success(), "the host should exit cleanly");
    assert!(!socket.exists(), "the socket should be cleaned up");
}
