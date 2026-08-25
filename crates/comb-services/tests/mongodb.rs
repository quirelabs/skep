//! Boots a real mongod and pings it over the wire protocol.

mod support;

use comb::{Probe, ServiceState};
use comb_services::{Mongodb, ServiceAdapter};
use support::{heavy, registered, start};

#[tokio::test]
async fn boots_and_answers_a_wire_protocol_ping() {
    if !heavy("mongodb boots") {
        return;
    }
    let (engine, id, _) = registered(&Mongodb, "boot", &["mongodb"]).await;

    start(&engine, &id).await;

    let status = engine.status_of(&id).await.unwrap();
    assert_eq!(status.state, ServiceState::Ready);
    assert!(status.pid.is_some());

    // Ready means the ping was answered with ok: 1. Ask again directly, so a
    // probe that somehow passed on a bound port would be caught here.
    let health = engine.spec_of(&id).await.unwrap().health;
    assert!(
        health.check().await.is_ok(),
        "the server should still answer"
    );

    engine.stop(&id).await.unwrap();
    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Stopped
    );
    assert!(
        health.check().await.is_err(),
        "a stopped server should not answer"
    );
}

#[test]
fn its_probe_reads_the_protocol_not_the_port() {
    // Guarded unconditionally, so a downgrade cannot make the gated test above
    // pass for the wrong reason on a machine that never runs it.
    let spec = Mongodb
        .spec(
            &Default::default(),
            &comb::Paths::new("/tmp/skep-mongo-check"),
        )
        .unwrap();
    assert!(matches!(spec.health.probe, Probe::Mongo { .. }));
}
