//! Installing the privileged half, proved without privileges. Every path an
//! install touches is redirectable, so the round trip can be checked against a
//! temporary directory instead of against the machine.

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use comb::{Error, Layout, Owner};
use support::{TestHome, shout};

const SUFFIX: &str = "test";

fn everything_under(root: &Path) -> BTreeSet<PathBuf> {
    let mut found = BTreeSet::new();
    let mut looking = vec![root.to_path_buf()];
    while let Some(directory) = looking.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                looking.push(path);
            } else {
                found.insert(path);
            }
        }
    }
    found
}

fn a_helper(home: &TestHome) -> PathBuf {
    let path = home.path().join("skep-helper-source");
    std::fs::write(&path, b"#!/bin/sh\ntrue\n").unwrap();
    path
}

fn me() -> Owner {
    Owner { uid: 501, gid: 20 }
}

#[test]
fn installing_and_removing_leaves_nothing_behind() {
    let home = TestHome::new();
    let root = home.path().join("machine");
    std::fs::create_dir_all(&root).unwrap();
    let layout = Layout::under(&root, SUFFIX);

    let before = everything_under(&root);
    let touched = comb::place(&layout, &a_helper(&home), me(), false).unwrap();
    let after = everything_under(&root);

    assert!(
        after.len() > before.len(),
        "an install that writes nothing is not an install"
    );
    for path in &touched {
        assert!(
            path.exists(),
            "{} was reported but not written",
            path.display()
        );
    }

    comb::remove(&layout).unwrap();
    assert_eq!(
        everything_under(&root),
        before,
        "uninstall has to put the machine back exactly as it found it"
    );
}

#[test]
fn a_resolver_file_somebody_else_wrote_is_refused() {
    let home = TestHome::new();
    let root = home.path().join("machine");
    let layout = Layout::under(&root, SUFFIX);
    std::fs::create_dir_all(layout.resolver.parent().unwrap()).unwrap();
    std::fs::write(&layout.resolver, "nameserver 127.0.0.1\n").unwrap();

    let refused = comb::place(&layout, &a_helper(&home), me(), false);

    assert!(matches!(refused, Err(Error::ResolverInUse { .. })));
    // And it was left exactly as it was.
    assert_eq!(
        std::fs::read_to_string(&layout.resolver).unwrap(),
        "nameserver 127.0.0.1\n"
    );
    assert!(!layout.plist.exists(), "a refusal must not half install");
}

#[test]
fn taking_over_keeps_the_original_and_gives_it_back() {
    let home = TestHome::new();
    let root = home.path().join("machine");
    let layout = Layout::under(&root, SUFFIX);
    std::fs::create_dir_all(layout.resolver.parent().unwrap()).unwrap();
    let theirs = "nameserver 127.0.0.1\nport 53\n";
    std::fs::write(&layout.resolver, theirs).unwrap();

    comb::place(&layout, &a_helper(&home), me(), true).unwrap();
    assert!(
        layout.backup.is_file(),
        "the original should have been kept"
    );
    assert!(
        std::fs::read_to_string(&layout.resolver)
            .unwrap()
            .contains("port 15353"),
        "skep should be the one being pointed at now"
    );

    comb::remove(&layout).unwrap();
    assert_eq!(
        std::fs::read_to_string(&layout.resolver).unwrap(),
        theirs,
        "the other tool's setup has to survive a round trip"
    );
    assert!(!layout.backup.exists(), "the copy is not left lying around");
}

#[test]
fn our_own_resolver_file_is_not_treated_as_a_stranger() {
    let home = TestHome::new();
    let root = home.path().join("machine");
    let layout = Layout::under(&root, SUFFIX);
    std::fs::create_dir_all(layout.resolver.parent().unwrap()).unwrap();
    std::fs::write(&layout.resolver, "nameserver 127.0.0.1\nport 15353\n").unwrap();

    assert!(comb::foreign(&layout).is_none());
    comb::place(&layout, &a_helper(&home), me(), false).expect("reinstalling is not a conflict");
    assert!(!layout.backup.exists(), "there was nothing to back up");
}

#[test]
fn the_daemon_asks_for_the_ports_it_should_hold() {
    let home = TestHome::new();
    let root = home.path().join("machine");
    let layout = Layout::under(&root, SUFFIX);
    comb::place(&layout, &a_helper(&home), me(), false).unwrap();

    let plist = std::fs::read_to_string(&layout.plist).unwrap();
    assert!(plist.contains("<string>80:8080</string>"), "{plist}");
    assert!(plist.contains("<string>443:8443</string>"), "{plist}");
    assert!(plist.contains("<string>501</string>"), "{plist}");
    // It has to come back on its own, or every local domain fails silently
    // after a reboot.
    assert!(plist.contains("<key>KeepAlive</key>"), "{plist}");
    assert!(plist.contains("<key>RunAtLoad</key>"), "{plist}");
}

#[tokio::test]
async fn a_helper_from_another_version_says_so() {
    let home = TestHome::new();
    let control = home.path().join("helper.sock");

    // Stands in for a helper an older install left behind.
    let listener = tokio::net::UnixListener::bind(&control).unwrap();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            let _ = stream.read_line(&mut line).await;
            let _ = stream
                .get_mut()
                .write_all(b"{\"protocol\":99,\"pid\":1,\"forwarding\":[]}\n")
                .await;
        }
    });

    let complaint = comb::health(&control).await.unwrap_err().to_string();
    assert!(complaint.contains("99"), "{complaint}");
    assert!(complaint.contains("skep domains install"), "{complaint}");
}

#[tokio::test]
async fn no_helper_at_all_is_a_sentence_not_a_hang() {
    let home = TestHome::new();
    let complaint = comb::health(&home.path().join("nothing.sock"))
        .await
        .unwrap_err()
        .to_string();
    assert!(complaint.contains("no helper"), "{complaint}");
}

/// Loading a launchd daemon and flushing the resolver cache need root, so this
/// is skipped rather than faked. Loudly, because a silent skip reads as a pass.
#[test]
fn handing_the_daemon_to_launchd_needs_root() {
    if std::env::var("SKEP_TEST_ROOT").is_err() {
        shout(
            "  SKIPPED handing_the_daemon_to_launchd_needs_root: needs root. \
             Set SKEP_TEST_ROOT=1 and run as root to cover launchctl and the resolver cache.",
        );
        return;
    }
    assert!(
        comb::is_root(),
        "SKEP_TEST_ROOT is set but this is not root"
    );

    let home = TestHome::new();
    let root = home.path().join("machine");
    let layout = Layout::under(&root, SUFFIX);
    comb::place(&layout, &a_helper(&home), comb::invoking_user(), false).unwrap();
    let verified = comb::activate(&layout, SUFFIX);
    comb::deactivate(&layout).ok();
    comb::remove(&layout).unwrap();
    verified.expect("a real install should verify");
}
