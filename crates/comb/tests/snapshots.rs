//! A snapshot name becomes a path. The names that could leave the directory
//! are refused before anything is joined, on the way out as well as in.

mod support;

use comb::{BinarySpec, Engine, Error, InstanceId, ServiceSpec};
use support::{TestHome, fake_service, fake_spec};

#[tokio::test]
async fn removing_a_snapshot_cannot_reach_outside_the_snapshots() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(&home, "valkey@8", &[]);
    let id = spec.id.clone();
    let data = spec.data_dir.clone();
    engine.register(spec).await.unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("keep"), "the service's own data").unwrap();

    // `..` from the snapshots directory is the data directory itself.
    let error = engine.remove_snapshot(&id, "..").await.unwrap_err();

    assert!(matches!(error, Error::InvalidId(_)), "{error}");
    assert!(data.join("keep").is_file(), "the data must be untouched");
}

#[tokio::test]
async fn branching_from_a_snapshot_cannot_copy_from_outside_them() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(&home, "valkey@8", &[]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();

    let branch = InstanceId::branch("valkey", "8", "copy").unwrap();
    let into = home.path().join("copy");
    let spec = ServiceSpec::new(branch, BinarySpec::path(fake_service()), into.clone());

    let error = engine.branch(&id, spec, Some("../..")).await.unwrap_err();

    assert!(matches!(error, Error::InvalidId(_)), "{error}");
    assert!(!into.exists(), "nothing should have been copied");
}
