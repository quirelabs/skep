//! Branching a real Postgres. The point of the whole feature is that two
//! servers hold different data at the same time and neither disturbs the
//! other, so that is what this asserts.

mod support;

use std::process::Command;

use comb::{Engine, InstanceId, Label, Paths, ServiceState, Version};
use comb_services::{Postgres, branch_spec};
use support::{registered_clean, shared_home, start};

/// Runs one statement against an instance and returns what it printed.
fn psql(port: u16, statement: &str) -> String {
    let program = Paths::new(shared_home())
        .binary_dir("postgres", &Version::new("17.6.0").unwrap())
        .join("bin/psql");
    let output = Command::new(program)
        .args([
            "-h",
            "127.0.0.1",
            "-p",
            &port.to_string(),
            "-U",
            "skep",
            "-d",
            "postgres",
            "-tAc",
            statement,
        ])
        .output()
        .expect("psql runs");
    assert!(
        output.status.success(),
        "psql failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

async fn port_of(engine: &Engine, id: &InstanceId) -> u16 {
    engine.status_of(id).await.unwrap().ports["postgres"]
}

#[tokio::test]
async fn a_branch_holds_its_own_data_while_its_parent_keeps_hers() {
    let (engine, main, _) = registered_clean(&Postgres, "trunk", &["postgres"]).await;
    // Anything a previous run left behind is not part of this one.
    let expected = InstanceId::branch("postgres", "17.6.0", "experiment").unwrap();
    let _ = std::fs::remove_dir_all(Paths::new(shared_home()).data_dir(&expected));
    start(&engine, &main).await;
    let main_port = port_of(&engine, &main).await;

    psql(main_port, "CREATE TABLE notes (body text)");
    psql(main_port, "INSERT INTO notes VALUES ('from main')");

    // Branching stops the parent, copies, and starts it again, so the copy is
    // taken from a database that had finished writing.
    let label = Label::new("experiment").unwrap();
    let spec = branch_spec(&main, &label, &Paths::new(shared_home())).unwrap();
    let branch = engine.branch(&main, spec, None).await.unwrap();

    assert!(branch.is_branch(), "a branch is an ordinary instance");
    // Labels are flat: a branch belongs to the service and version, not to
    // whichever instance it was copied from.
    assert_eq!(branch.to_string(), format!("{}:experiment", main.base()));
    start(&engine, &branch).await;
    let branch_port = port_of(&engine, &branch).await;
    assert_ne!(branch_port, main_port, "a branch gets its own port");

    // The branch starts from what the parent had.
    assert_eq!(psql(branch_port, "SELECT body FROM notes"), "from main");

    // Now they diverge, and both are running at once.
    psql(branch_port, "UPDATE notes SET body = 'from the branch'");
    psql(main_port, "INSERT INTO notes VALUES ('added later')");

    assert_eq!(
        psql(
            branch_port,
            "SELECT string_agg(body, ',' ORDER BY body) FROM notes"
        ),
        "from the branch"
    );
    assert_eq!(
        psql(
            main_port,
            "SELECT string_agg(body, ',' ORDER BY body) FROM notes"
        ),
        "added later,from main",
        "the parent should not have seen the branch's writes"
    );
    assert_eq!(
        engine.status_of(&main).await.unwrap().state,
        ServiceState::Ready,
        "both servers answer at the same time"
    );

    // And removing the branch leaves the parent exactly as she was.
    engine.stop(&branch).await.unwrap();
    engine.remove_branch(&branch).await.unwrap();
    assert!(
        engine.status_of(&branch).await.is_err(),
        "a removed branch is gone from the engine"
    );
    assert_eq!(
        psql(
            main_port,
            "SELECT string_agg(body, ',' ORDER BY body) FROM notes"
        ),
        "added later,from main"
    );

    engine.stop(&main).await.unwrap();
}

#[tokio::test]
async fn a_snapshot_is_a_place_to_branch_from_later() {
    let (engine, main, _) = registered_clean(&Postgres, "shelf", &["postgres"]).await;
    let rewind = InstanceId::branch("postgres", "17.6.0", "rewind").unwrap();
    let _ = std::fs::remove_dir_all(Paths::new(shared_home()).data_dir(&rewind));
    start(&engine, &main).await;
    let main_port = port_of(&engine, &main).await;

    psql(main_port, "CREATE TABLE notes (body text)");
    psql(main_port, "INSERT INTO notes VALUES ('before')");

    let _ = engine.remove_snapshot(&main, "before-the-change").await;
    engine.snapshot(&main, "before-the-change").await.unwrap();
    let kept = engine.snapshots(&main).await.unwrap();
    assert!(kept.iter().any(|kept| kept.name == "before-the-change"));

    // The parent moves on. The snapshot does not.
    assert_eq!(
        engine.status_of(&main).await.unwrap().state,
        ServiceState::Ready,
        "a snapshot puts the service back the way it found it"
    );
    psql(main_port, "UPDATE notes SET body = 'after'");

    let label = Label::new("rewind").unwrap();
    let spec = branch_spec(&main, &label, &Paths::new(shared_home())).unwrap();
    let branch = engine
        .branch(&main, spec, Some("before-the-change"))
        .await
        .unwrap();
    start(&engine, &branch).await;

    assert_eq!(
        psql(port_of(&engine, &branch).await, "SELECT body FROM notes"),
        "before",
        "the branch should hold what the snapshot held"
    );
    assert_eq!(psql(main_port, "SELECT body FROM notes"), "after");

    engine.stop(&branch).await.unwrap();
    engine.remove_branch(&branch).await.unwrap();
    engine
        .remove_snapshot(&main, "before-the-change")
        .await
        .unwrap();
    assert!(
        !engine
            .snapshots(&main)
            .await
            .unwrap()
            .iter()
            .any(|kept| kept.name == "before-the-change")
    );
    engine.stop(&main).await.unwrap();
}
