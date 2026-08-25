//! Valkey has no macOS binary to download, so this exercises the whole
//! from-source path: pinned source, a compile in scratch, a binary kept only
//! after it answers for itself, and a server that then serves.

mod support;

use comb::{Probe, ServiceState, Version};
use comb_services::{ServiceAdapter, Valkey};
use support::{heavy, history, registered_cold, shared_home, start};

/// One test rather than several, because they would otherwise race over who
/// performs the single shared build, and only the winner sees its output.
#[tokio::test]
async fn builds_from_source_keeps_only_the_binaries_and_serves() {
    if !heavy("valkey builds from source") {
        return;
    }
    let version = Version::new("9.1.1").unwrap();
    let installed = comb::Paths::new(shared_home()).binary_dir("valkey", &version);
    let _ = std::fs::remove_dir_all(&installed);

    // Nothing is pre-installed: starting is what fetches, compiles and keeps.
    let (engine, id, _) = registered_cold(&Valkey, "build", &["valkey"]).await;
    start(&engine, &id).await;

    // The compiler's own words land in the service history, which is where
    // anyone would look when a build fails.
    let text = history(&engine, &id).await;
    assert!(
        text.contains("built Valkey server v=9.1.1"),
        "the verified build should be recorded: {text}"
    );

    let health = engine.spec_of(&id).await.unwrap().health;
    assert!(health.check().await.is_ok(), "it should answer a PING");

    // A build tree is mostly object files, and none of them should survive.
    assert!(installed.join("src/valkey-server").is_file());
    assert!(
        !installed.join("src/server.o").exists(),
        "object files should not have survived"
    );
    assert!(
        !installed.join("Makefile").exists(),
        "the source tree should not have survived"
    );

    engine.stop(&id).await.unwrap();
    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Stopped
    );
    assert!(
        health.check().await.is_err(),
        "a stopped server answers nothing"
    );
}

#[test]
fn its_probe_reads_the_protocol_not_the_port() {
    let spec = Valkey
        .spec(
            &Default::default(),
            &comb::Paths::new("/tmp/skep-valkey-check"),
        )
        .unwrap();
    assert!(matches!(spec.health.probe, Probe::Resp { .. }));
}
