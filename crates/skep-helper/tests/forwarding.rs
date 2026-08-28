//! The helper doing its real job, with the real binary. Ports 80 and 443 need
//! root, but nothing about the forwarding does, so this runs the same code
//! against ports anyone can bind.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Kills the helper even when a test panics before it gets there.
struct Helping(Child);

impl Drop for Helping {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// An app that says back whatever it is told.
async fn echoing() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut reading, mut writing) = stream.split();
                let _ = tokio::io::copy(&mut reading, &mut writing).await;
            });
        }
    });
    port
}

async fn wait_for(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(path.exists(), "the helper never opened its control socket");
}

#[tokio::test]
async fn it_carries_bytes_to_the_port_behind_it() {
    let backend = echoing().await;
    let front = free_port();
    let control =
        std::env::temp_dir().join(format!("skep-helper-{}-carry.sock", std::process::id()));
    let _ = std::fs::remove_file(&control);

    let owner = comb::invoking_user();
    let _helper = Helping(
        Command::new(env!("CARGO_BIN_EXE_skep-helper"))
            .args(["--control", &control.display().to_string()])
            .args(["--user", &owner.uid.to_string()])
            .args(["--group", &owner.gid.to_string()])
            .args(["--forward", &format!("{front}:{backend}")])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    wait_for(&control).await;

    let mut through = TcpStream::connect(("127.0.0.1", front)).await.unwrap();
    through.write_all(b"straight through").await.unwrap();

    let mut back = [0u8; 16];
    through.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"straight through");

    let _ = std::fs::remove_file(&control);
}

#[tokio::test]
async fn it_says_which_version_it_is_and_what_it_holds() {
    let backend = echoing().await;
    let front = free_port();
    let control =
        std::env::temp_dir().join(format!("skep-helper-{}-version.sock", std::process::id()));
    let _ = std::fs::remove_file(&control);

    let owner = comb::invoking_user();
    let _helper = Helping(
        Command::new(env!("CARGO_BIN_EXE_skep-helper"))
            .args(["--control", &control.display().to_string()])
            .args(["--user", &owner.uid.to_string()])
            .args(["--group", &owner.gid.to_string()])
            .args(["--forward", &format!("{front}:{backend}")])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    wait_for(&control).await;

    let health = comb::health(&control)
        .await
        .expect("the helper should answer");
    assert_eq!(health.protocol, comb::HELPER_PROTOCOL);
    assert_eq!(health.forwarding.len(), 1);
    assert_eq!(health.forwarding[0].from, front);
    assert_eq!(health.forwarding[0].to, backend);

    let _ = std::fs::remove_file(&control);
}

#[test]
fn it_refuses_to_run_without_being_told_what_to_do() {
    let output = Command::new(env!("CARGO_BIN_EXE_skep-helper"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("nothing to forward"), "{said}");
}
