#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::Once;

/// The fake service is its own workspace member, so `cargo test -p comb` has
/// not built it. Build it once, on demand, into its own target directory so
/// the nested cargo never waits on the lock this test run already holds.
pub fn fake_service() -> PathBuf {
    static BUILD: Once = Once::new();
    static DIR: &str = "fake-service-target";

    let target = target_dir().join(DIR);
    BUILD.call_once(|| {
        let status = Command::new(env!("CARGO"))
            .args(["build", "--quiet", "-p", "fake-service", "--target-dir"])
            .arg(&target)
            .status()
            .expect("cargo should be runnable from a test");
        assert!(status.success(), "building fake-service failed");
    });

    let binary = target.join("debug").join("fake-service");
    assert!(binary.is_file(), "no fake service at {}", binary.display());
    binary
}

fn target_dir() -> PathBuf {
    let mut dir = std::env::current_exe().expect("test binary has a path");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    dir
}

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use comb::{BinarySpec, InstanceId, LogLine, ServiceSpec};
use tokio::sync::broadcast::Receiver;
use tokio::time::timeout;

/// A throwaway `~/.skep` that cleans itself up.
pub struct TestHome {
    root: PathBuf,
}

impl TestHome {
    pub fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "skep-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("creates a test home");
        Self { root }
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A spec that runs the fake service with the given flags.
pub fn fake_spec(home: &TestHome, id: &str, args: &[&str]) -> ServiceSpec {
    let id: InstanceId = id.parse().expect("a valid instance id");
    let data_dir = home.root.join(id.service.as_str());
    ServiceSpec::new(id, BinarySpec::path(fake_service()), data_dir).with_args(args.iter().copied())
}

/// Waits for a matching line rather than sleeping for one.
pub async fn wait_for_log(
    logs: &mut Receiver<LogLine>,
    matching: impl Fn(&LogLine) -> bool,
) -> LogLine {
    timeout(Duration::from_secs(5), async {
        loop {
            let line = logs.recv().await.expect("the log stream stays open");
            if matching(&line) {
                return line;
            }
        }
    })
    .await
    .expect("the expected log line should arrive")
}

use comb::{Backoff, Event, EventKind, RestartPolicy, RestartSpec, ServiceState};

/// A restart policy with a short, flat backoff so tests stay quick.
pub fn restart_after(policy: RestartPolicy, max_attempts: u32, delay: Duration) -> RestartSpec {
    RestartSpec {
        policy,
        backoff: Backoff {
            initial: delay,
            max: delay,
            factor: 1.0,
            max_attempts,
        },
    }
}

pub async fn wait_for_event(
    events: &mut Receiver<Event>,
    matching: impl Fn(&EventKind) -> bool,
) -> Event {
    timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.expect("the event stream stays open");
            if matching(&event.kind) {
                return event;
            }
        }
    })
    .await
    .expect("the expected event should arrive")
}

pub async fn wait_for_state(
    events: &mut Receiver<Event>,
    matching: impl Fn(&ServiceState) -> bool + Copy,
) -> ServiceState {
    let event = wait_for_event(events, |kind| match kind {
        EventKind::StateChanged { to, .. } => matching(to),
        _ => false,
    })
    .await;
    match event.kind {
        EventKind::StateChanged { to, .. } => to,
        _ => unreachable!("filtered above"),
    }
}

/// Fails if a matching event shows up inside the window.
pub async fn expect_none(
    events: &mut Receiver<Event>,
    window: Duration,
    matching: impl Fn(&EventKind) -> bool,
) {
    let arrived = timeout(window, async {
        loop {
            let event = events.recv().await.expect("the event stream stays open");
            if matching(&event.kind) {
                return event;
            }
        }
    })
    .await;
    assert!(arrived.is_err(), "unexpected event {:?}", arrived.unwrap());
}

use comb::{HealthCheck, Probe};

/// A port nobody is using yet. Bound and released so the child can take it.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("binds an ephemeral port")
        .local_addr()
        .expect("has an address")
        .port()
}

pub fn health(probe: Probe, startup_timeout: Duration) -> HealthCheck {
    HealthCheck {
        probe,
        interval: Duration::from_millis(20),
        timeout: Duration::from_millis(500),
        startup_timeout,
    }
}
