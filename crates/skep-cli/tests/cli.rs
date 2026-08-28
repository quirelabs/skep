//! Drives the real binary. The CLI is a pure client, so the interesting cases
//! are the ones where there is no engine to talk to.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn skep() -> Command {
    Command::new(env!("CARGO_BIN_EXE_skep"))
}

/// Kills the host even when a test panics before it gets there.
struct Serving(std::process::Child);

impl Serving {
    fn start(home: &Path) -> Self {
        let child = skep()
            .arg("serve")
            .env("SKEP_HOME", home)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let socket = home.join("run").join("engine.sock");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !socket.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(socket.exists(), "the host never bound its socket");
        Self(child)
    }

    fn wind_down(mut self) -> std::process::ExitStatus {
        unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
        self.0.wait().unwrap()
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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
    let socket = home.0.join("run").join("engine.sock");
    let serving = Serving::start(&home.0);

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
    let refusal = String::from_utf8_lossy(&refused.stderr);
    assert!(refusal.contains("running this machine"), "{refusal}");
    // The refusal has to leave the person a way out, not just a no.
    assert!(refusal.contains("skep serve --take-over"), "{refusal}");

    assert!(
        serving.wind_down().success(),
        "the host should exit cleanly"
    );
    assert!(!socket.exists(), "the socket should be cleaned up");
}

#[test]
fn up_without_a_project_file_says_where_it_looked() {
    let home = Home::new("no-project");
    let elsewhere = Home::new("empty-dir");
    let output = skep()
        .arg("up")
        .current_dir(&elsewhere.0)
        .env("SKEP_HOME", &home.0)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("no skep.toml"), "{message}");
}

#[test]
fn up_refuses_a_file_it_does_not_understand() {
    let home = Home::new("typo");
    let project = Home::new("typo-project");
    std::fs::write(
        project.0.join("skep.toml"),
        "[services.postgres]\nverison = \"16\"\n",
    )
    .unwrap();

    let output = skep()
        .arg("up")
        .current_dir(&project.0)
        .env("SKEP_HOME", &home.0)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let message = String::from_utf8_lossy(&output.stderr);
    // The typo, and what it should have been.
    assert!(message.contains("verison"), "{message}");
    assert!(message.contains("expected one of `version`"), "{message}");
}

#[test]
fn up_reports_every_service_even_when_one_fails() {
    let home = Home::new("partial");
    let project = Home::new("partial-project");
    // Neither name is a service skep has, so nothing is downloaded or bound and
    // the point being tested is that the second is still attempted.
    std::fs::write(
        project.0.join("skep.toml"),
        "[services.redis]\nversion = \"7\"\n\n[services.memcached]\nversion = \"1\"\n",
    )
    .unwrap();

    let serving = Serving::start(&home.0);

    let output = skep()
        .arg("up")
        .current_dir(&project.0)
        .env("SKEP_HOME", &home.0)
        .output()
        .unwrap();

    let listed = String::from_utf8_lossy(&output.stdout);
    assert!(listed.contains("redis: unknown service redis"), "{listed}");
    assert!(
        listed.contains("memcached: unknown service memcached"),
        "{listed}"
    );
    assert!(!output.status.success(), "a failure should be a failure");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("2 of 2"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    serving.wind_down();
}

#[test]
fn up_without_an_engine_says_how_to_start_one() {
    let home = Home::new("up-no-engine");
    let project = Home::new("up-no-engine-project");
    std::fs::write(project.0.join("skep.toml"), "[services.mailpit]\n").unwrap();

    let output = skep()
        .arg("up")
        .current_dir(&project.0)
        .env("SKEP_HOME", &home.0)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no skep engine is running"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A config file with the things people actually put in one: comments, blank
/// lines, and spacing nobody would choose twice.
const HAND_WRITTEN: &str = "\
# What this repository needs.
#
# Postgres is pinned because the staging box runs 16.
[services.postgres]
version = \"16\"     # keep in step with staging

# Mail goes nowhere in development.
[services.mailpit]
ports = { http = 8025, smtp = 1025 }
";

#[test]
fn adding_a_site_leaves_the_rest_of_the_file_byte_for_byte() {
    let home = Home::new("site-add");
    let project = Home::new("site-add-project");
    let file = project.0.join("skep.toml");
    std::fs::write(&file, HAND_WRITTEN).unwrap();

    let output = skep()
        .args(["site", "add", "shop.test", "3000"])
        .current_dir(&project.0)
        .env("SKEP_HOME", &home.0)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let after = std::fs::read_to_string(&file).unwrap();
    assert!(
        after.starts_with(HAND_WRITTEN),
        "everything that was there before has to survive unchanged:\n{after}"
    );
    assert!(after.contains("[sites]"), "{after}");
    assert!(after.contains("\"shop.test\" = 3000"), "{after}");
}

#[test]
fn adding_to_a_table_that_exists_keeps_its_comments() {
    let home = Home::new("site-second");
    let project = Home::new("site-second-project");
    let file = project.0.join("skep.toml");
    let original = "\
[sites]
# The storefront, which the checkout service talks to.
\"shop.test\" = 3000

# Databases below.
[services.postgres]
version = \"16\"
";
    std::fs::write(&file, original).unwrap();

    let output = skep()
        .args(["site", "add", "admin.test", "4000"])
        .current_dir(&project.0)
        .env("SKEP_HOME", &home.0)
        .output()
        .unwrap();
    assert!(output.status.success());

    let after = std::fs::read_to_string(&file).unwrap();
    for kept in [
        "# The storefront, which the checkout service talks to.",
        "\"shop.test\" = 3000",
        "# Databases below.",
        "[services.postgres]",
        "version = \"16\"",
    ] {
        assert!(after.contains(kept), "{kept} was lost:\n{after}");
    }
    assert!(after.contains("\"admin.test\" = 4000"), "{after}");
}

#[test]
fn removing_a_site_leaves_the_others_and_their_comments() {
    let home = Home::new("site-remove");
    let project = Home::new("site-remove-project");
    let file = project.0.join("skep.toml");
    let original = "\
# Two sites.
[sites]
\"shop.test\" = 3000
# The one we are keeping.
\"admin.test\" = 4000
";
    std::fs::write(&file, original).unwrap();

    let output = skep()
        .args(["site", "remove", "shop.test"])
        .current_dir(&project.0)
        .env("SKEP_HOME", &home.0)
        .output()
        .unwrap();
    assert!(output.status.success());

    let after = std::fs::read_to_string(&file).unwrap();
    assert!(!after.contains("shop.test"), "{after}");
    assert!(after.contains("# Two sites."), "{after}");
    assert!(after.contains("# The one we are keeping."), "{after}");
    assert!(after.contains("\"admin.test\" = 4000"), "{after}");
}

#[test]
fn a_hostname_that_could_never_be_served_never_reaches_the_file() {
    let home = Home::new("site-bad");
    let project = Home::new("site-bad-project");
    let file = project.0.join("skep.toml");
    std::fs::write(&file, HAND_WRITTEN).unwrap();

    for host in ["../../etc/passwd", "not a host", ""] {
        let output = skep()
            .args(["site", "add", host, "3000"])
            .current_dir(&project.0)
            .env("SKEP_HOME", &home.0)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{host:?} should have been refused"
        );
    }

    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        HAND_WRITTEN,
        "a refusal must not touch the file at all"
    );
}

#[test]
fn removing_something_that_is_not_there_says_so() {
    let home = Home::new("site-absent");
    let project = Home::new("site-absent-project");
    std::fs::write(project.0.join("skep.toml"), HAND_WRITTEN).unwrap();

    let output = skep()
        .args(["site", "remove", "nothing.test"])
        .current_dir(&project.0)
        .env("SKEP_HOME", &home.0)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("nothing.test"), "{message}");
}

#[test]
fn without_a_project_file_it_says_to_use_global() {
    let home = Home::new("site-nowhere");
    let elsewhere = Home::new("site-nowhere-dir");

    let output = skep()
        .args(["site", "add", "shop.test", "3000"])
        .current_dir(&elsewhere.0)
        .env("SKEP_HOME", &home.0)
        .output()
        .unwrap();

    assert!(!output.status.success());
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("--global"), "{message}");
}
