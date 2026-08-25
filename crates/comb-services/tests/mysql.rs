//! Boots a real MySQL server: initialize, a handshake-level readiness check,
//! and a clean shutdown.

mod support;

use std::net::TcpStream;

use comb::{Probe, ServiceState};
use comb_services::{Mysql, ServiceAdapter};
use support::{heavy, history, registered, start};

#[tokio::test]
async fn boots_and_greets_over_the_wire() {
    if !heavy("mysql boots") {
        return;
    }
    let (engine, id, ports) = registered(&Mysql, "boot", &["mysql"]).await;
    let port = ports[0];

    start(&engine, &id).await;

    // Ready means it greeted us. Prove the greeting was real rather than a
    // bound port by reading the handshake independently.
    let mut greeting = [0u8; 128];
    let read = {
        use std::io::Read;
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.read(&mut greeting).unwrap()
    };
    assert!(read > 5, "no handshake");
    assert_eq!(greeting[4], 0x0a, "not a MySQL protocol 10 handshake");

    engine.stop(&id).await.unwrap();
    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Stopped
    );
    let text = history(&engine, &id).await;
    assert!(
        text.contains("Shutdown complete"),
        "unclean shutdown: {text}"
    );
}

#[tokio::test]
async fn setup_runs_once_and_the_data_directory_is_reused() {
    if !heavy("mysql reuses its data directory") {
        return;
    }
    let (engine, id, _) = registered(&Mysql, "reuse", &["mysql"]).await;

    start(&engine, &id).await;
    engine.stop(&id).await.unwrap();

    let mut events = engine.subscribe_events();
    start(&engine, &id).await;
    let prepared = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| matches!(event.kind, comb::EventKind::Preparing { .. }))
        .count();
    assert_eq!(prepared, 0, "initialisation should not run twice");

    engine.stop(&id).await.unwrap();
}

#[test]
fn its_probe_reads_the_protocol_not_the_port() {
    // Guarded unconditionally: a downgrade to a bare TCP check would make the
    // heavy test above pass for the wrong reason.
    let spec = Mysql
        .spec(
            &Default::default(),
            &comb::Paths::new("/tmp/skep-mysql-check"),
        )
        .unwrap();
    assert!(matches!(spec.health.probe, Probe::Mysql { .. }));
}
