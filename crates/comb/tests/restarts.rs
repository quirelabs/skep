//! Crash detection and the restart policy, driven by processes that really die.

mod support;

use std::time::{Duration, Instant};

use comb::{Engine, EventKind, RestartPolicy, ServiceState};
use support::{TestHome, expect_none, fake_spec, restart_after, wait_for_event, wait_for_state};

const QUICK: Duration = Duration::from_millis(20);

fn is_restart(kind: &EventKind) -> bool {
    matches!(kind, EventKind::RestartScheduled { .. })
}

fn attempt_of(kind: &EventKind) -> u32 {
    match kind {
        EventKind::RestartScheduled { attempt, .. } => *attempt,
        other => panic!("expected a restart, got {other:?}"),
    }
}

#[tokio::test]
async fn a_crash_is_reported_with_the_exit_code() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(
        &home,
        "valkey@8",
        &["--exit-after-ms", "20", "--exit-code", "3"],
    )
    .with_restart(restart_after(RestartPolicy::Never, 5, QUICK));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut events = engine.subscribe_events();

    engine.start(&id).await.unwrap();

    let state = wait_for_state(&mut events, |s| matches!(s, ServiceState::Failed { .. })).await;
    let ServiceState::Failed { reason } = state else {
        unreachable!()
    };
    assert_eq!(reason, "exited with code 3");

    let status = engine.status_of(&id).await.unwrap();
    assert_eq!(status.pid, None);
    expect_none(&mut events, QUICK * 5, is_restart).await;
}

#[tokio::test]
async fn the_last_words_of_a_crashing_process_are_kept() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(
        &home,
        "valkey@8",
        &["--exit-after-ms", "20", "--exit-code", "9"],
    )
    .with_restart(restart_after(RestartPolicy::Never, 5, QUICK));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut events = engine.subscribe_events();

    engine.start(&id).await.unwrap();
    wait_for_state(&mut events, |s| matches!(s, ServiceState::Failed { .. })).await;

    // Output is drained before the state changes, so the history is complete
    // by the time anyone reacts to the failure.
    let history = engine.logs(&id, 100).await.unwrap();
    assert!(
        history
            .iter()
            .any(|line| line.text == "fake-service: exiting with 9"),
        "history was {history:?}"
    );
}

#[tokio::test]
async fn on_crash_restarts_until_the_cap_and_then_gives_up() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(
        &home,
        "valkey@8",
        &["--exit-after-ms", "10", "--exit-code", "1"],
    )
    .with_restart(restart_after(RestartPolicy::OnCrash, 3, QUICK));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut events = engine.subscribe_events();

    engine.start(&id).await.unwrap();

    let mut attempts = Vec::new();
    for _ in 0..3 {
        attempts.push(attempt_of(
            &wait_for_event(&mut events, is_restart).await.kind,
        ));
    }
    assert_eq!(attempts, [1, 2, 3]);

    // The crash after the last attempt is the end of the road.
    wait_for_state(&mut events, |s| matches!(s, ServiceState::Failed { .. })).await;
    expect_none(&mut events, QUICK * 10, is_restart).await;
    assert!(matches!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Failed { .. }
    ));
}

#[tokio::test]
async fn a_clean_exit_is_a_crash_only_under_always() {
    let home = TestHome::new();
    let engine = Engine::new();

    let polite = fake_spec(
        &home,
        "valkey@8",
        &["--exit-after-ms", "10", "--exit-code", "0"],
    )
    .with_restart(restart_after(RestartPolicy::OnCrash, 3, QUICK));
    let polite_id = polite.id.clone();
    engine.register(polite).await.unwrap();

    let insistent = fake_spec(
        &home,
        "mailpit@1",
        &["--exit-after-ms", "10", "--exit-code", "0"],
    )
    .with_restart(restart_after(RestartPolicy::Always, 3, QUICK));
    let insistent_id = insistent.id.clone();
    engine.register(insistent).await.unwrap();

    let mut events = engine.subscribe_events();
    engine.start(&polite_id).await.unwrap();
    let state = wait_for_state(&mut events, |s| matches!(s, ServiceState::Failed { .. })).await;
    assert_eq!(state, ServiceState::failed("exited with code 0"));
    expect_none(&mut events, QUICK * 5, is_restart).await;

    engine.start(&insistent_id).await.unwrap();
    let restart = wait_for_event(&mut events, is_restart).await;
    assert_eq!(restart.instance, Some(insistent_id));
    assert_eq!(attempt_of(&restart.kind), 1);
}

#[tokio::test]
async fn a_stop_during_the_backoff_wins() {
    let home = TestHome::new();
    let engine = Engine::new();
    let patient = Duration::from_secs(5);
    let spec = fake_spec(
        &home,
        "valkey@8",
        &["--exit-after-ms", "10", "--exit-code", "1"],
    )
    .with_restart(restart_after(RestartPolicy::OnCrash, 5, patient));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut events = engine.subscribe_events();

    engine.start(&id).await.unwrap();
    wait_for_event(&mut events, is_restart).await;

    let stopping = Instant::now();
    engine.stop(&id).await.unwrap();

    // Interrupting the wait, not sitting through it.
    assert!(stopping.elapsed() < patient / 2);
    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Stopped
    );
    expect_none(&mut events, QUICK * 5, is_restart).await;
}

#[tokio::test]
async fn a_graceful_stop_is_never_reported_as_a_crash() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(&home, "valkey@8", &[]).with_restart(restart_after(
        RestartPolicy::Always,
        5,
        QUICK,
    ));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut events = engine.subscribe_events();

    engine.start(&id).await.unwrap();
    engine.stop(&id).await.unwrap();

    let mut states = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let EventKind::StateChanged { to, .. } = &event.kind {
            states.push(to.name());
        }
        assert!(
            !is_restart(&event.kind),
            "a deliberate stop scheduled a restart"
        );
    }
    assert_eq!(states, ["starting", "ready", "stopping", "stopped"]);
}

#[tokio::test]
async fn a_deliberate_start_clears_the_attempt_counter() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(
        &home,
        "valkey@8",
        &["--exit-after-ms", "10", "--exit-code", "1"],
    )
    .with_restart(restart_after(RestartPolicy::OnCrash, 5, QUICK));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut events = engine.subscribe_events();

    engine.start(&id).await.unwrap();
    for expected in [1, 2] {
        let event = wait_for_event(&mut events, is_restart).await;
        assert_eq!(attempt_of(&event.kind), expected);
    }

    engine.stop(&id).await.unwrap();
    engine.start(&id).await.unwrap();

    let event = wait_for_event(&mut events, is_restart).await;
    assert_eq!(attempt_of(&event.kind), 1, "the counter should start over");
}
