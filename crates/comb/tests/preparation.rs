//! One-time setup before a service can serve. The point of these tests is that
//! a long first start is never a silent Starting.

mod support;

use std::time::Duration;

use comb::{BinarySpec, Engine, Error, EventKind, PrepareStep, ServiceState};
use support::{TestHome, fake_service, fake_spec, wait_for_event};

fn step(name: &str, args: &[&str]) -> PrepareStep {
    PrepareStep::new(name, BinarySpec::path(fake_service())).with_args(args.iter().copied())
}

#[tokio::test]
async fn each_setup_phase_is_announced_while_it_runs() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(&home, "postgres@16", &[]).with_prepare([step(
        "initialise the database",
        &["--exit-after-ms", "300", "--exit-code", "0"],
    )]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut events = engine.subscribe_events();

    let background = {
        let (engine, id) = (engine.clone(), id.clone());
        tokio::spawn(async move { engine.start(&id).await })
    };

    // The phase is set before it is announced, so seeing the event means a
    // caller asking right now would be told what is happening.
    wait_for_event(
        &mut events,
        |kind| matches!(kind, EventKind::Preparing { step } if step == "initialise the database"),
    )
    .await;
    let status = engine.status_of(&id).await.unwrap();
    assert_eq!(status.state, ServiceState::Starting);
    assert_eq!(status.activity.as_deref(), Some("initialise the database"));

    background.await.unwrap().unwrap();

    let finished = wait_for_event(&mut events, |kind| {
        matches!(kind, EventKind::Prepared { .. })
    })
    .await;
    let EventKind::Prepared { took, .. } = finished.kind else {
        unreachable!()
    };
    assert!(took >= Duration::from_millis(300), "took {took:?}");

    let status = engine.status_of(&id).await.unwrap();
    assert_eq!(status.state, ServiceState::Ready);
    assert_eq!(status.activity, None, "a running service has no phase");

    engine.stop(&id).await.unwrap();
}

#[tokio::test]
async fn setup_that_already_ran_is_skipped() {
    let home = TestHome::new();
    let engine = Engine::new();
    let marker = home.path().join("initialised");
    let args = [
        "--fail-if-exists".to_string(),
        marker.display().to_string(),
        "--exit-after-ms".to_string(),
        "10".to_string(),
        "--exit-code".to_string(),
        "0".to_string(),
    ];
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let spec = fake_spec(&home, "postgres@16", &[]).with_prepare([step(
        "initialise the database",
        &borrowed,
    )
    .unless_exists(&marker)]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();

    engine.start(&id).await.unwrap();
    assert!(marker.is_file(), "setup should have run once");
    engine.stop(&id).await.unwrap();

    // Second time round the marker is there, so nothing is announced and the
    // step's binary is never run again.
    let mut events = engine.subscribe_events();
    engine.start(&id).await.unwrap();
    support::expect_none(&mut events, Duration::from_millis(200), |kind| {
        matches!(kind, EventKind::Preparing { .. })
    })
    .await;

    engine.stop(&id).await.unwrap();
}

#[tokio::test]
async fn setup_that_fails_stops_the_start_and_says_why() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(&home, "postgres@16", &[]).with_prepare([step(
        "initialise the database",
        &["--exit-after-ms", "10", "--exit-code", "5"],
    )]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();

    let error = engine.start(&id).await.unwrap_err();

    assert!(matches!(error, Error::Prepare { .. }));
    assert!(
        error
            .to_string()
            .contains("could not initialise the database"),
        "got {error}"
    );
    assert!(error.to_string().contains("exited with code 5"));

    let status = engine.status_of(&id).await.unwrap();
    assert!(matches!(status.state, ServiceState::Failed { .. }));
    assert_eq!(status.activity, None);

    // Setup output lands in the service history, where anyone would look.
    let history = engine.logs(&id, 100).await.unwrap();
    assert!(
        history
            .iter()
            .any(|line| line.text.contains("exiting with 5")),
        "got {history:?}"
    );
}

#[tokio::test]
async fn output_only_appears_once_the_step_finishes() {
    let home = TestHome::new();
    let engine = Engine::new();
    let built = home.path().join("cluster");
    let marker = built.join("VERSION");

    // Writes its output, then dies. initdb behaves the same way under SIGKILL:
    // files on disk, no completion.
    let args = [
        "--touch".to_string(),
        "{output}/VERSION".to_string(),
        "--exit-after-ms".to_string(),
        "10".to_string(),
        "--exit-code".to_string(),
        "9".to_string(),
    ];
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let spec = fake_spec(&home, "postgres@16", &[]).with_prepare([step(
        "initialise the database",
        &borrowed,
    )
    .unless_exists(&marker)
    .produces(&built)]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();

    let error = engine.start(&id).await.unwrap_err();

    assert!(matches!(error, Error::Prepare { .. }));
    assert!(
        !marker.exists(),
        "a killed step must not leave a marker a later run would trust"
    );
    assert!(!built.exists(), "no half built output should survive");
    let leftovers: Vec<_> = std::fs::read_dir(home.path())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().contains("scratch"))
        .collect();
    assert!(leftovers.is_empty(), "scratch survived: {leftovers:?}");
}

#[tokio::test]
async fn finished_output_is_promoted_and_then_trusted() {
    let home = TestHome::new();
    let engine = Engine::new();
    let built = home.path().join("cluster");
    let marker = built.join("VERSION");
    let args = [
        "--touch".to_string(),
        "{output}/VERSION".to_string(),
        "--exit-after-ms".to_string(),
        "10".to_string(),
        "--exit-code".to_string(),
        "0".to_string(),
    ];
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let spec = fake_spec(&home, "postgres@16", &[]).with_prepare([step(
        "initialise the database",
        &borrowed,
    )
    .unless_exists(&marker)
    .produces(&built)]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();

    engine.start(&id).await.unwrap();
    assert!(marker.is_file(), "finished output should be in place");
    engine.stop(&id).await.unwrap();

    // And now the marker genuinely means "already done".
    let mut events = engine.subscribe_events();
    engine.start(&id).await.unwrap();
    support::expect_none(&mut events, Duration::from_millis(200), |kind| {
        matches!(kind, EventKind::Preparing { .. })
    })
    .await;
    engine.stop(&id).await.unwrap();
}
