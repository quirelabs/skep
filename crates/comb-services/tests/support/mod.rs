#![allow(dead_code)]

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use comb::{Engine, InstanceId, Paths, ServiceState, Version};
use comb_services::{Request, ServiceAdapter, install};

/// Adapters whose releases run to hundreds of megabytes. They are opt in, so a
/// laptop checkout stays cheap, and CI sets SKEP_TEST_HEAVY=1 to run them.
pub fn heavy(what: &str) -> bool {
    if std::env::var_os("SKEP_TEST_HEAVY").is_some() {
        return true;
    }
    shout(&format!(
        "  SKIPPED {what}: large download. Set SKEP_TEST_HEAVY=1 to run it."
    ));
    false
}

/// libtest captures the print macros, so a notice written with println only
/// appears when a test fails or with --nocapture. Writing to the real stderr
/// is not captured, which is what keeps a skip from passing for a green run.
fn shout(message: &str) {
    use std::os::fd::FromRawFd;

    // Safety: fd 2 is the process's stderr, and the handle is forgotten below
    // rather than dropped, so the descriptor is never closed.
    let mut stderr = unsafe { std::fs::File::from_raw_fd(2) };
    // One write, so two tests skipping at once cannot interleave.
    let _ = stderr.write_all(format!("{message}\n").as_bytes());
    std::mem::forget(stderr);
}

/// Releases install once here, so only a cold checkout pays the download.
pub fn shared_home() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/skep-test-home")
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub fn curl(url: &str) -> String {
    let output = Command::new("curl")
        .args(["--fail", "--silent", "--max-time", "5", url])
        .output()
        .expect("curl runs");
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Installs the pinned release and registers one labelled instance on free
/// ports, so tests never collide with each other or with the developer's own
/// services.
pub async fn registered(
    adapter: &'static dyn ServiceAdapter,
    label: &str,
    ports: &[&str],
) -> (Engine, InstanceId, Vec<u16>) {
    let paths = Paths::new(shared_home());
    let version = Version::new(adapter.default_version()).unwrap();
    install(adapter, &version, &paths)
        .await
        .expect("the pinned release installs");

    let chosen: Vec<u16> = ports.iter().map(|_| free_port()).collect();
    let mut request = Request::new().with_label(label.parse().unwrap());
    for (name, port) in ports.iter().zip(&chosen) {
        request = request.with_port(*name, *port);
    }

    let spec = adapter.spec(&request, &paths).unwrap();
    let id = spec.id.clone();
    let engine = Engine::with_paths(paths);
    engine.register(spec).await.unwrap();
    (engine, id, chosen)
}

/// Starts and, on failure, shows what the service said. A bare exit code is
/// not something anyone should have to reproduce by hand.
pub async fn start(engine: &Engine, id: &InstanceId) {
    if let Err(error) = engine.start(id).await {
        panic!(
            "{error}\n--- service output ---\n{}",
            history(engine, id).await
        );
    }
    assert_eq!(
        engine.status_of(id).await.unwrap().state,
        ServiceState::Ready
    );
}

pub async fn history(engine: &Engine, id: &InstanceId) -> String {
    engine
        .logs(id, 500)
        .await
        .unwrap()
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}
