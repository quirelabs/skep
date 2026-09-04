//! Starting a project the way the window starts one: read the file, build a
//! spec with a port nobody wrote down, and hand it to the engine.

mod support;

use std::time::Duration;

use comb::{Engine, Paths, ServiceState};
use support::fake_service;

/// A skep.toml with the given [run] command, in a directory named after the
/// test so the project's name is its own.
fn project(name: &str, command: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("skep-run-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join(comb_services::project::FILE),
        format!("[run]\ncommand = \"{command}\"\n"),
    )
    .unwrap();
    root.join(comb_services::project::FILE)
}

async fn state(engine: &Engine, id: &comb::InstanceId) -> ServiceState {
    engine
        .status()
        .await
        .into_iter()
        .find(|status| &status.id == id)
        .map(|status| status.state)
        .expect("the project is registered")
}

#[tokio::test]
async fn a_project_that_takes_the_port_reaches_ready() {
    let file = project(
        "good",
        &format!("{} --listen {{port}}", fake_service().display()),
    );
    let paths =
        Paths::new(std::env::temp_dir().join(format!("skep-run-home-{}", std::process::id())));
    let loaded = comb_services::project::load(&file).unwrap();
    let (spec, port) = comb_services::run_spec(&file, &loaded.run.unwrap(), &paths).unwrap();
    let id = spec.id.clone();

    let engine = Engine::with_paths(paths);
    engine.upsert(spec).await.unwrap();
    engine.start(&id).await.unwrap();

    assert_eq!(state(&engine, &id).await, ServiceState::Ready);
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok(),
        "the project should be answering on the port skep chose"
    );
    engine.stop(&id).await.unwrap();
}

/// The failure this replaced took thirty seconds: the process started, listened
/// somewhere else, and skep waited out the whole startup timeout watching a
/// port nothing would ever answer on.
#[tokio::test]
async fn a_command_that_never_says_which_port_is_refused_before_it_runs() {
    let file = project(
        "deaf",
        &format!("{} --listen 3000", fake_service().display()),
    );
    let paths =
        Paths::new(std::env::temp_dir().join(format!("skep-deaf-home-{}", std::process::id())));
    let loaded = comb_services::project::load(&file).unwrap();

    let started = std::time::Instant::now();
    let refused = comb_services::run_spec(&file, &loaded.run.unwrap(), &paths).unwrap_err();

    assert!(
        refused.to_string().contains("{port}"),
        "the refusal has to say what to write: {refused}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "and it has to say it now rather than after the startup timeout"
    );
}

/// The escape hatch for a tool that takes its port from the environment
/// rather than from a flag. Without this, making the placeholder mandatory
/// would be a rule some perfectly good commands could not satisfy.
#[tokio::test]
async fn a_project_told_through_the_environment_reaches_ready() {
    let file = project(
        "env",
        &format!(
            "PORT={{port}} {} --listen-from-env",
            fake_service().display()
        ),
    );
    let paths =
        Paths::new(std::env::temp_dir().join(format!("skep-env-home-{}", std::process::id())));
    let loaded = comb_services::project::load(&file).unwrap();
    let (spec, port) = comb_services::run_spec(&file, &loaded.run.unwrap(), &paths).unwrap();
    let id = spec.id.clone();

    let engine = Engine::with_paths(paths);
    engine.upsert(spec).await.unwrap();
    engine.start(&id).await.unwrap();

    assert_eq!(state(&engine, &id).await, ServiceState::Ready);
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());
    engine.stop(&id).await.unwrap();
}

/// The command that actually happened: npm took the flag for itself, vite
/// picked its own port, and skep sat watching the port it had chosen until
/// the startup timeout ran out.
#[tokio::test]
async fn npm_keeping_the_port_for_itself_is_refused_with_the_fix() {
    let file = project("npm", "npm run dev --port {port}");
    let paths =
        Paths::new(std::env::temp_dir().join(format!("skep-npm-home-{}", std::process::id())));
    let loaded = comb_services::project::load(&file).unwrap();

    let refused = comb_services::run_spec(&file, &loaded.run.unwrap(), &paths).unwrap_err();

    assert!(
        refused.to_string().contains("npm run dev -- --port {port}"),
        "the refusal has to hand back the command that works: {refused}"
    );
}
