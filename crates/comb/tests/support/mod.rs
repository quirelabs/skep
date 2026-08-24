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
