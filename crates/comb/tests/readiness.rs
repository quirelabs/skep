//! Readiness is measured, not waited out. These tests give the engine a long
//! budget and check it finishes as soon as the service answers.

mod support;

use std::time::{Duration, Instant};

use comb::{Engine, Error, EventKind, Probe, ServiceState};
use support::{TestHome, fake_spec, free_port, health, wait_for_event};

const PATIENT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn waits_for_the_port_rather_than_the_clock() {
    let home = TestHome::new();
    let engine = Engine::new();
    let port = free_port();
    let listen = port.to_string();
    let spec = fake_spec(
        &home,
        "valkey@8",
        &["--listen", &listen, "--listen-delay-ms", "300"],
    )
    .with_health(health(Probe::Tcp { port }, PATIENT));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut events = engine.subscribe_events();

    let starting = Instant::now();
    engine.start(&id).await.unwrap();
    let took = starting.elapsed();

    assert!(
        took >= Duration::from_millis(300),
        "returned early: {took:?}"
    );
    assert!(took < Duration::from_secs(3), "waited too long: {took:?}");
    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Ready
    );

    let event = wait_for_event(&mut events, |kind| {
        matches!(kind, EventKind::ProbeSucceeded { .. })
    })
    .await;
    let EventKind::ProbeSucceeded { after } = event.kind else {
        unreachable!()
    };
    assert!(after >= Duration::from_millis(300));

    engine.stop(&id).await.unwrap();
}

#[tokio::test]
async fn a_service_that_never_answers_gives_up_once() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(&home, "valkey@8", &[]).with_health(health(
        Probe::Tcp { port: free_port() },
        Duration::from_millis(200),
    ));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut events = engine.subscribe_events();

    let error = engine.start(&id).await.unwrap_err();

    assert!(matches!(error, Error::NotReady { .. }));
    assert!(matches!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Failed { .. }
    ));

    // Polling every 20ms for 200ms must not put ten failures on the stream.
    let failures = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| matches!(event.kind, EventKind::ProbeFailed { .. }))
        .count();
    assert_eq!(failures, 1);
}

#[tokio::test]
async fn a_process_that_dies_while_starting_is_reported_at_once() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(
        &home,
        "valkey@8",
        &["--exit-after-ms", "50", "--exit-code", "7"],
    )
    .with_health(health(Probe::Tcp { port: free_port() }, PATIENT));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();

    let starting = Instant::now();
    let error = engine.start(&id).await.unwrap_err();

    // The startup budget is ten seconds; the answer arrives in well under one.
    assert!(starting.elapsed() < Duration::from_secs(2));
    assert!(matches!(error, Error::DiedStarting { .. }));
    assert!(error.to_string().contains("exited with code 7"));
}

#[tokio::test]
async fn answers_a_resp_ping() {
    let home = TestHome::new();
    let engine = Engine::new();
    let port = free_port();
    let listen = port.to_string();
    let spec = fake_spec(&home, "valkey@8", &["--listen", &listen, "--speak", "resp"])
        .with_health(health(Probe::Resp { port }, PATIENT));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();

    engine.start(&id).await.unwrap();

    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Ready
    );
    engine.stop(&id).await.unwrap();
}

#[tokio::test]
async fn checks_the_http_status_it_was_told_to_expect() {
    let home = TestHome::new();
    let engine = Engine::new();
    let port = free_port();
    let listen = port.to_string();
    let args = ["--listen", listen.as_str(), "--speak", "http"];

    let good = fake_spec(&home, "mailpit@1", &args).with_health(health(
        Probe::Http {
            port,
            path: "/".to_string(),
            expect: 200,
        },
        PATIENT,
    ));
    let good_id = good.id.clone();
    engine.register(good).await.unwrap();
    engine.start(&good_id).await.unwrap();
    assert_eq!(
        engine.status_of(&good_id).await.unwrap().state,
        ServiceState::Ready
    );
    engine.stop(&good_id).await.unwrap();

    let picky = fake_spec(&home, "mailpit@2", &args).with_health(health(
        Probe::Http {
            port,
            path: "/".to_string(),
            expect: 204,
        },
        Duration::from_millis(300),
    ));
    let picky_id = picky.id.clone();
    engine.register(picky).await.unwrap();

    let error = engine.start(&picky_id).await.unwrap_err();
    assert!(error.to_string().contains("answered 200, expected 204"));
}
