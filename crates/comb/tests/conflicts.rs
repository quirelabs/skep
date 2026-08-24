//! Port conflicts, from both directions: caught before the spawn, and caught
//! after a bind that lost a race. Both must say the same thing.

mod support;

use std::net::TcpListener;
use std::time::Duration;

use comb::{Engine, Error, EventKind, Port, Probe, ServiceState};
use support::{TestHome, fake_spec, free_port, health};

#[tokio::test]
async fn a_held_port_is_refused_before_anything_is_spawned() {
    let home = TestHome::new();
    let engine = Engine::new();
    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();

    let spec = fake_spec(&home, "valkey@8", &[]).with_ports([Port::new("main", port)]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut events = engine.subscribe_events();

    let error = engine.start(&id).await.unwrap_err();

    assert!(matches!(error, Error::PortTaken { .. }));
    let message = error.to_string();
    // The test binary is the holder, so its own pid must be named.
    assert!(
        message.starts_with(&format!("port {port} is held by")),
        "{message}"
    );
    assert!(
        message.contains(&std::process::id().to_string()),
        "{message}"
    );
    assert!(
        message.contains("change the port in skep.toml"),
        "{message}"
    );

    let conflicts = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| matches!(event.kind, EventKind::PortConflict { .. }))
        .count();
    assert_eq!(conflicts, 1, "the conflict should reach the event stream");

    assert!(matches!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Failed { .. }
    ));
}

#[tokio::test]
async fn a_port_lost_after_the_check_gets_the_same_answer() {
    let home = TestHome::new();
    let engine = Engine::new();
    let contested = free_port();
    let listen = contested.to_string();

    // The service binds late. Readiness watches a different port that nobody
    // will ever open, so the race is between the bind and the check, not
    // between the bind and a probe.
    let spec = fake_spec(
        &home,
        "valkey@8",
        &["--listen", &listen, "--listen-delay-ms", "400"],
    )
    .with_ports([Port::new("main", contested)])
    .with_health(health(
        Probe::Tcp { port: free_port() },
        Duration::from_secs(20),
    ));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();

    // Free at the check, taken by the time the service tries to bind.
    let stealing = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        TcpListener::bind(("127.0.0.1", contested)).unwrap()
    });

    let error = engine.start(&id).await.unwrap_err();
    let _thief = stealing.await.unwrap();

    assert!(
        matches!(error, Error::PortTaken { .. }),
        "a bind failure should be explained, got: {error}"
    );
    let message = error.to_string();
    assert!(
        message.starts_with(&format!("port {contested} is held by")),
        "{message}"
    );
    assert!(
        message.contains(&std::process::id().to_string()),
        "{message}"
    );

    // Both paths word it identically, so prove this was the later one: the
    // service actually ran, which means the check before the spawn passed.
    let history = engine.logs(&id, 50).await.unwrap();
    assert!(
        history
            .iter()
            .any(|line| line.text.starts_with("ready pid=")),
        "the service never started, so this was the pre-spawn check: {history:?}"
    );
}

#[tokio::test]
async fn a_free_port_is_not_reported_as_a_conflict() {
    let home = TestHome::new();
    let engine = Engine::new();
    let port = free_port();
    let listen = port.to_string();

    let spec = fake_spec(&home, "valkey@8", &["--listen", &listen])
        .with_ports([Port::new("main", port)])
        .with_health(health(Probe::Tcp { port }, Duration::from_secs(10)));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();

    engine.start(&id).await.unwrap();

    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Ready
    );
    engine.stop(&id).await.unwrap();
}
