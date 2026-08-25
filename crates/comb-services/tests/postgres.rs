//! Boots a real Postgres cluster: initdb, a protocol-level readiness check,
//! and a shutdown that leaves the cluster clean.

mod support;

use std::time::{Duration, Instant};

use comb::{Engine, HealthCheck, InstanceId, ServiceState};
use comb_services::Postgres;
use support::{history, registered, start};

async fn registered_pg(label: &str) -> (Engine, InstanceId, HealthCheck) {
    let (engine, id, _) = registered(&Postgres, label, &["postgres"]).await;
    let health = engine.spec_of(&id).await.unwrap().health;
    (engine, id, health)
}

#[tokio::test]
async fn the_probe_tells_a_starting_cluster_from_a_serving_one() {
    let (engine, id, health) = registered_pg("probe").await;

    // Watch from outside the engine, from before the postmaster exists until
    // it answers. A probe that cannot tell these apart adds nothing over a
    // plain TCP connect, so the test insists on seeing both.
    let observer = tokio::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut seen: Vec<String> = Vec::new();
        while Instant::now() < deadline {
            match health.check().await {
                Ok(()) => {
                    seen.push("accepting connections".to_string());
                    break;
                }
                Err(reason) => {
                    if seen.last() != Some(&reason) {
                        seen.push(reason);
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        seen
    });

    start(&engine, &id).await;
    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Ready
    );

    let seen = observer.await.unwrap();
    // The exact wording proves this came from the postmaster's own error
    // field, not from a connection error that happens to read similarly.
    assert!(
        seen.iter()
            .any(|reason| reason == "the database system is starting up"),
        "the probe never observed a starting cluster: {seen:#?}"
    );
    assert_eq!(
        seen.last().map(String::as_str),
        Some("accepting connections"),
        "{seen:#?}"
    );

    engine.stop(&id).await.unwrap();
}

#[tokio::test]
async fn a_stop_checkpoints_so_the_next_start_is_clean() {
    let (engine, id, _) = registered_pg("shutdown").await;

    start(&engine, &id).await;
    engine.stop(&id).await.unwrap();

    // Postgres says so itself: a fast shutdown that reached the checkpoint.
    let text = history(&engine, &id).await;
    assert!(
        text.contains("received fast shutdown request"),
        "the fast shutdown signal did not arrive: {text}"
    );
    assert!(
        text.contains("database system is shut down"),
        "no clean checkpoint: {text}"
    );

    start(&engine, &id).await;
    let text = history(&engine, &id).await;
    assert!(
        !text.contains("was not properly shut down"),
        "the next start had to recover: {text}"
    );
    assert!(text.contains("database system is ready to accept connections"));

    engine.stop(&id).await.unwrap();
}

#[tokio::test]
async fn setup_runs_once_and_the_cluster_is_reused() {
    let (engine, id, _) = registered_pg("reuse").await;

    start(&engine, &id).await;
    engine.stop(&id).await.unwrap();

    let mut events = engine.subscribe_events();
    start(&engine, &id).await;

    let prepared = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| matches!(event.kind, comb::EventKind::Preparing { .. }))
        .count();
    assert_eq!(prepared, 0, "initdb should not run against a live cluster");

    engine.stop(&id).await.unwrap();
}
