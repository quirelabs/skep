//! Drives real child processes through the engine. Readiness is always a
//! captured log line or an observed state, never a sleep.

mod support;

use std::time::{Duration, Instant};

use comb::{
    Engine, Error, EventKind, LogStream, Port, Probe, ServiceState, ShutdownSpec, StopSignal,
};
use support::{TestHome, fake_spec, free_port, health, wait_for_log};

#[tokio::test]
async fn starts_a_process_and_reports_the_pid_it_actually_has() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(&home, "valkey@8", &[]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut logs = engine.subscribe_logs(&id).await.unwrap();

    engine.start(&id).await.unwrap();

    let status = engine.status_of(&id).await.unwrap();
    assert_eq!(status.state, ServiceState::Ready);
    let pid = status.pid.expect("a running service has a pid");

    // The process announces its own pid, so this checks the engine is not
    // reporting a number it merely hopes is right.
    let announced = wait_for_log(&mut logs, |line| line.text.starts_with("ready pid=")).await;
    assert_eq!(announced.stream, LogStream::Stdout);
    assert_eq!(announced.text, format!("ready pid={pid}"));

    engine.stop(&id).await.unwrap();
    let status = engine.status_of(&id).await.unwrap();
    assert_eq!(status.state, ServiceState::Stopped);
    assert_eq!(status.pid, None);
}

#[tokio::test]
async fn captures_both_output_streams_into_the_history() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(&home, "valkey@8", &["--emit-every-ms", "1"]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut logs = engine.subscribe_logs(&id).await.unwrap();

    engine.start(&id).await.unwrap();
    wait_for_log(&mut logs, |line| line.text == "err 3").await;
    engine.stop(&id).await.unwrap();

    let history = engine.logs(&id, 500).await.unwrap();
    assert!(history.iter().any(|line| line.text == "out 1"));
    assert!(
        history
            .iter()
            .any(|line| line.stream == LogStream::Stderr && line.text == "err 1")
    );
    // Timestamps come from capture order, so the history stays sorted.
    assert!(history.windows(2).all(|pair| pair[0].at <= pair[1].at));
}

#[tokio::test]
async fn a_cooperative_process_stops_promptly_and_in_order() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(&home, "valkey@8", &[]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut events = engine.subscribe_events();

    engine.start(&id).await.unwrap();
    let stopping = Instant::now();
    engine.stop(&id).await.unwrap();

    // Default grace is ten seconds, so a SIGTERM that worked is obvious.
    assert!(stopping.elapsed() < Duration::from_secs(1));

    let mut states = Vec::new();
    let mut sequence = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let EventKind::StateChanged { to, .. } = &event.kind {
            states.push(to.name());
            sequence.push(event.seq);
        }
    }
    assert_eq!(states, ["starting", "ready", "stopping", "stopped"]);
    assert!(sequence.windows(2).all(|pair| pair[0] < pair[1]));
}

#[tokio::test]
async fn an_uncooperative_process_is_killed_once_the_grace_expires() {
    let home = TestHome::new();
    let engine = Engine::new();
    let grace = Duration::from_millis(300);
    let spec = fake_spec(&home, "valkey@8", &["--ignore-term"])
        .with_shutdown(ShutdownSpec::new(StopSignal::Term, grace));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut logs = engine.subscribe_logs(&id).await.unwrap();

    engine.start(&id).await.unwrap();
    wait_for_log(&mut logs, |line| line.text.starts_with("ready pid=")).await;

    let stopping = Instant::now();
    engine.stop(&id).await.unwrap();
    let took = stopping.elapsed();

    assert!(
        took >= grace,
        "should have waited out the grace, took {took:?}"
    );
    assert!(
        took < Duration::from_secs(3),
        "should have escalated, took {took:?}"
    );
    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Stopped
    );
}

#[tokio::test]
async fn starting_a_running_service_is_rejected_without_disturbing_it() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(&home, "valkey@8", &[]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();

    engine.start(&id).await.unwrap();
    let pid = engine.status_of(&id).await.unwrap().pid;

    assert!(matches!(
        engine.start(&id).await,
        Err(Error::IllegalTransition { .. })
    ));

    let status = engine.status_of(&id).await.unwrap();
    assert_eq!(status.state, ServiceState::Ready);
    assert_eq!(status.pid, pid);

    engine.stop(&id).await.unwrap();
}

#[tokio::test]
async fn restart_replaces_the_process() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(&home, "valkey@8", &[]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();

    engine.start(&id).await.unwrap();
    let before = engine.status_of(&id).await.unwrap().pid.unwrap();

    engine.restart(&id).await.unwrap();

    let status = engine.status_of(&id).await.unwrap();
    assert_eq!(status.state, ServiceState::Ready);
    assert_ne!(status.pid.unwrap(), before);

    engine.stop(&id).await.unwrap();
}

#[tokio::test]
async fn a_stop_during_a_slow_start_wins() {
    let home = TestHome::new();
    let engine = Engine::new();
    let port = free_port();
    // Listens only after a long delay, so the start sits in its probe loop.
    let spec = fake_spec(
        &home,
        "valkey@8",
        &["--listen", &port.to_string(), "--listen-delay-ms", "4000"],
    )
    .with_ports([Port::new("main", port)])
    .with_health(health(Probe::Tcp { port }, Duration::from_secs(10)));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut logs = engine.subscribe_logs(&id).await.unwrap();

    let starting = {
        let (engine, id) = (engine.clone(), id.clone());
        tokio::spawn(async move { engine.start(&id).await })
    };
    let launched = wait_for_log(&mut logs, |line| line.text.starts_with("ready pid=")).await;
    let pid = launched.text.trim_start_matches("ready pid=").to_string();

    let stopping = Instant::now();
    engine.stop(&id).await.unwrap();

    assert!(
        stopping.elapsed() < Duration::from_secs(2),
        "the stop should interrupt the start, not sit out its budget"
    );
    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Stopped
    );
    let error = starting.await.unwrap().unwrap_err();
    assert!(matches!(error, Error::Interrupted(_)), "{error}");
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid])
        .status()
        .unwrap()
        .success();
    assert!(!alive, "the half-started process {pid} should be gone");
}

#[tokio::test]
async fn a_restart_during_a_slow_start_replaces_the_process() {
    let home = TestHome::new();
    let engine = Engine::new();
    let port = free_port();
    let spec = fake_spec(
        &home,
        "valkey@8",
        &["--listen", &port.to_string(), "--listen-delay-ms", "300"],
    )
    .with_ports([Port::new("main", port)])
    .with_health(health(Probe::Tcp { port }, Duration::from_secs(10)));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut logs = engine.subscribe_logs(&id).await.unwrap();

    let starting = {
        let (engine, id) = (engine.clone(), id.clone());
        tokio::spawn(async move { engine.start(&id).await })
    };
    let first = wait_for_log(&mut logs, |line| line.text.starts_with("ready pid=")).await;

    // Before the fix this met the engine's own child on the port.
    engine.restart(&id).await.unwrap();

    assert!(matches!(
        starting.await.unwrap().unwrap_err(),
        Error::Interrupted(_)
    ));
    let status = engine.status_of(&id).await.unwrap();
    assert_eq!(status.state, ServiceState::Ready);
    assert_ne!(
        format!("ready pid={}", status.pid.unwrap()),
        first.text,
        "the restart should have brought up a fresh process"
    );
    engine.stop(&id).await.unwrap();
}
