//! Boots the real Mailpit binary through the engine. The release installs once
//! into a shared location under target, so only the first run downloads.

use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;

use comb::{Engine, Paths, ServiceState, Version};
use comb_services::{Mailpit, Request, ServiceAdapter, install};

fn shared_home() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/skep-test-home")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn get(url: &str) -> String {
    let output = Command::new("curl")
        .args(["--fail", "--silent", "--max-time", "5", url])
        .output()
        .expect("curl runs");
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Installs the pinned release and registers one labelled instance on free
/// ports, so parallel tests never collide with each other or with a Mailpit
/// the developer happens to be running.
async fn running(label: &str) -> (Engine, comb::InstanceId, u16, u16) {
    let paths = Paths::new(shared_home());
    let version = Version::new(Mailpit.default_version()).unwrap();
    install(&Mailpit, &version, &paths)
        .await
        .expect("the pinned release installs");

    let (http, smtp) = (free_port(), free_port());
    let request = Request::new()
        .with_label(label.parse().unwrap())
        .with_port("http", http)
        .with_port("smtp", smtp);
    let spec = Mailpit.spec(&request, &paths).unwrap();
    let id = spec.id.clone();

    let engine = Engine::with_paths(paths);
    engine.register(spec).await.unwrap();
    engine.start(&id).await.unwrap();
    (engine, id, http, smtp)
}

#[tokio::test]
async fn boots_and_answers_on_both_ports() {
    let (engine, id, http, smtp) = running("serves").await;

    let status = engine.status_of(&id).await.unwrap();
    assert_eq!(status.state, ServiceState::Ready);
    assert_eq!(status.ports["http"], http);
    assert_eq!(status.ports["smtp"], smtp);

    // Ready only means the probe answered. Check the real API and the SMTP
    // listener too, so a probe that passed for the wrong reason is caught.
    assert!(
        get(&format!("http://127.0.0.1:{http}/api/v1/info")).contains("Version"),
        "the HTTP API should be serving"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", smtp)).is_ok(),
        "the SMTP port should be listening"
    );

    engine.stop(&id).await.unwrap();
    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Stopped
    );
}

#[tokio::test]
async fn stopping_hands_the_ports_back() {
    let (engine, id, http, _) = running("releases").await;
    engine.stop(&id).await.unwrap();

    assert!(
        TcpStream::connect(("127.0.0.1", http)).is_err(),
        "the port should be free once the service is stopped"
    );
}

#[tokio::test]
async fn its_logs_reach_the_engine() {
    let (engine, id, http, smtp) = running("logs").await;

    let history = engine.logs(&id, 100).await.unwrap();
    let text = history
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    // The banner names the interface it bound, so this also proves the spec
    // kept it off every interface but loopback.
    assert!(
        text.contains(&format!("[http] starting on 127.0.0.1:{http}")),
        "expected the http banner, got {text}"
    );
    assert!(
        text.contains(&format!("[smtpd] starting on 127.0.0.1:{smtp}")),
        "expected the smtp banner, got {text}"
    );

    engine.stop(&id).await.unwrap();
}
