//! Cold boot of a five service project. The two cases differ only in their
//! dependency edges, so the gap between them is the parallelism.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Once;
use std::time::Duration;

use comb::{BinarySpec, Engine, HealthCheck, InstanceId, Probe, ServiceSpec};
use criterion::{Criterion, criterion_group, criterion_main};
use tokio::runtime::Runtime;

const DELAY_MS: u64 = 30;

fn fake_service() -> PathBuf {
    static BUILD: Once = Once::new();
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join("fake-service-target");
    BUILD.call_once(|| {
        let status = Command::new(env!("CARGO"))
            .args(["build", "--quiet", "-p", "fake-service", "--target-dir"])
            .arg(&target)
            .status()
            .expect("cargo should be runnable");
        assert!(status.success(), "building fake-service failed");
    });
    target.join("debug").join("fake-service")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Five services, each taking DELAY_MS to answer. `chained` decides whether
/// they may overlap.
async fn boot(chained: bool) {
    let engine = Engine::new();
    let names = ["a@1", "b@1", "c@1", "d@1", "e@1"];
    let mut previous: Option<InstanceId> = None;
    let mut ids = Vec::new();

    for name in names {
        let port = free_port();
        let id: InstanceId = name.parse().unwrap();
        let spec = ServiceSpec::new(
            id.clone(),
            BinarySpec::path(fake_service()),
            std::env::temp_dir().join("skep-bench"),
        )
        .with_args([
            "--listen".to_string(),
            port.to_string(),
            "--listen-delay-ms".to_string(),
            DELAY_MS.to_string(),
        ])
        .with_health(HealthCheck {
            probe: Probe::Tcp { port },
            interval: Duration::from_millis(2),
            timeout: Duration::from_millis(500),
            startup_timeout: Duration::from_secs(10),
        })
        .with_depends_on(previous.clone());

        engine.register(spec).await.unwrap();
        previous = chained.then(|| id.clone());
        ids.push(id);
    }

    engine.start_all(&ids).await.unwrap();
    engine.stop_all(&ids).await.unwrap();
}

fn benchmark(c: &mut Criterion) {
    let runtime = Runtime::new().expect("a tokio runtime");
    let mut group = c.benchmark_group("boot");
    group.sample_size(10);

    group.bench_function("five-independent", |b| {
        b.to_async(&runtime).iter(|| boot(false));
    });
    group.bench_function("five-chained", |b| {
        b.to_async(&runtime).iter(|| boot(true));
    });

    group.finish();
}

criterion_group!(benches, benchmark);
criterion_main!(benches);
