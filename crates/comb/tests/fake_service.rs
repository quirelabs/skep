//! Checks the stand-in process the supervision tests steer, so a failure here
//! never gets mistaken for an engine bug.

mod support;

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

#[test]
fn announces_itself_and_holds() {
    let mut child = Command::new(support::fake_service())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawns");

    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .expect("reads a line");

    assert!(first.starts_with("ready pid="), "got {first:?}");
    assert!(
        child.try_wait().unwrap().is_none(),
        "should still be running"
    );
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn exits_on_cue_with_the_requested_code() {
    let status = Command::new(support::fake_service())
        .args(["--exit-after-ms", "10", "--exit-code", "3"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("runs");

    assert_eq!(status.code(), Some(3));
}

#[test]
fn chatters_on_both_streams() {
    let mut child = Command::new(support::fake_service())
        .args(["--emit-every-ms", "1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawns");

    let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut err = BufReader::new(child.stderr.take().unwrap()).lines();

    assert!(out.next().unwrap().unwrap().starts_with("ready pid="));
    assert_eq!(out.next().unwrap().unwrap(), "out 1");
    assert_eq!(err.next().unwrap().unwrap(), "err 1");

    child.kill().unwrap();
    child.wait().unwrap();
}
