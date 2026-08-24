//! Whether the dependency graph actually boots in parallel, measured against
//! services that take a known time to start answering.

mod support;

use std::slice;
use std::time::{Duration, Instant};

use comb::{Engine, Error, EventKind, InstanceId, Probe, ServiceState};
use support::{TestHome, fake_spec, free_port, health};

const PATIENT: Duration = Duration::from_secs(10);
const SLOW: u64 = 300;

/// A service that takes `delay` milliseconds to start answering on its port.
async fn slow_service(
    engine: &Engine,
    home: &TestHome,
    id: &str,
    delay: u64,
    depends_on: &[&str],
) -> InstanceId {
    let port = free_port();
    let args = [
        "--listen".to_string(),
        port.to_string(),
        "--listen-delay-ms".to_string(),
        delay.to_string(),
    ];
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let spec = fake_spec(home, id, &borrowed)
        .with_health(health(Probe::Tcp { port }, PATIENT))
        .with_depends_on(depends_on.iter().map(|d| d.parse().unwrap()));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    id
}

fn ready_order(events: &mut tokio::sync::broadcast::Receiver<comb::Event>) -> Vec<String> {
    std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|event| match event.kind {
            EventKind::StateChanged { ref to, .. } if *to == ServiceState::Ready => {
                Some(event.instance?.to_string())
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn independent_services_boot_at_the_same_time() {
    let home = TestHome::new();
    let engine = Engine::new();
    let names = [
        "valkey@8",
        "postgres@16",
        "mailpit@1",
        "mariadb@11",
        "mongo@7",
    ];
    let mut ids = Vec::new();
    for name in names {
        ids.push(slow_service(&engine, &home, name, SLOW, &[]).await);
    }

    let booting = Instant::now();
    engine.start_all(&ids).await.unwrap();
    let took = booting.elapsed();

    // Five services at 300ms each: a serial boot would need 1500ms.
    assert!(took >= Duration::from_millis(SLOW), "too fast: {took:?}");
    assert!(
        took < Duration::from_millis(SLOW * 3),
        "not parallel: {took:?}"
    );
    for status in engine.status().await {
        assert_eq!(status.state, ServiceState::Ready, "{}", status.id);
    }

    engine.stop_all(&ids).await.unwrap();
}

#[tokio::test]
async fn a_chain_waits_for_each_link() {
    let home = TestHome::new();
    let engine = Engine::new();
    let step = 150;
    slow_service(&engine, &home, "mongo@7", step, &[]).await;
    slow_service(&engine, &home, "postgres@16", step, &["mongo@7"]).await;
    let last = slow_service(&engine, &home, "valkey@8", step, &["postgres@16"]).await;
    let mut events = engine.subscribe_events();

    let booting = Instant::now();
    engine.start_all(slice::from_ref(&last)).await.unwrap();
    let took = booting.elapsed();

    // Three dependent links cannot overlap, so this one really is a sum.
    assert!(
        took >= Duration::from_millis(step * 3),
        "overlapped: {took:?}"
    );
    assert_eq!(
        ready_order(&mut events),
        ["mongo@7", "postgres@16", "valkey@8"]
    );
}

#[tokio::test]
async fn a_failed_dependency_skips_what_needs_it() {
    let home = TestHome::new();
    let engine = Engine::new();

    // Nothing ever binds this port, so the dependency cannot come up.
    let broken = fake_spec(&home, "postgres@16", &[]).with_health(health(
        Probe::Tcp { port: free_port() },
        Duration::from_millis(200),
    ));
    engine.register(broken).await.unwrap();
    let dependent = slow_service(&engine, &home, "valkey@8", 10, &["postgres@16"]).await;

    let error = engine
        .start_all(slice::from_ref(&dependent))
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            Error::NotReady { .. } | Error::DependencyFailed { .. }
        ),
        "got {error}"
    );
    assert_eq!(
        engine.status_of(&dependent).await.unwrap().state,
        ServiceState::Stopped,
        "a dependent should never be started"
    );
}

#[tokio::test]
async fn shutdown_unwinds_in_reverse() {
    let home = TestHome::new();
    let engine = Engine::new();
    slow_service(&engine, &home, "postgres@16", 10, &[]).await;
    let app = slow_service(&engine, &home, "valkey@8", 10, &["postgres@16"]).await;

    engine.start_all(slice::from_ref(&app)).await.unwrap();
    let mut events = engine.subscribe_events();
    engine.stop_all(slice::from_ref(&app)).await.unwrap();

    let stopped: Vec<String> = std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|event| match event.kind {
            EventKind::StateChanged { ref to, .. } if *to == ServiceState::Stopped => {
                Some(event.instance?.to_string())
            }
            _ => None,
        })
        .collect();

    assert_eq!(stopped, ["valkey@8", "postgres@16"]);
}

#[tokio::test]
async fn a_cycle_is_refused_before_anything_starts() {
    let home = TestHome::new();
    let engine = Engine::new();
    slow_service(&engine, &home, "postgres@16", 10, &["valkey@8"]).await;
    let other = slow_service(&engine, &home, "valkey@8", 10, &["postgres@16"]).await;
    let mut events = engine.subscribe_events();

    let error = engine.start_all(&[other]).await.unwrap_err();

    assert!(matches!(error, Error::DependencyCycle(_)));
    assert!(
        engine
            .status()
            .await
            .iter()
            .all(|s| s.state == ServiceState::Stopped)
    );
    assert!(
        events.try_recv().is_err(),
        "nothing should have been touched"
    );
}
