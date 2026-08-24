//! One engine owns the machine. These tests cover the ways that ownership can
//! go wrong: two hosts, a crashed host, a stale socket, a mismatched client.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use comb::{Client, Engine, Error, Host, Lock, Paths, Request, Response, ServiceState};
use support::{TestHome, fake_spec};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::oneshot;

fn engine_for(home: &TestHome) -> (Engine, Paths) {
    let paths = Paths::new(home.path());
    (Engine::with_paths(paths.clone()), paths)
}

/// Claims the machine and serves in the background until the returned sender
/// is used.
async fn hosted(engine: Engine) -> (oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let host = Host::claim(engine).await.expect("claims the machine");
    let (stop, stopped) = oneshot::channel();
    let serving = tokio::spawn(async move {
        let _ = host
            .serve(async {
                let _ = stopped.await;
            })
            .await;
    });
    (stop, serving)
}

#[tokio::test]
async fn only_one_process_can_host() {
    let home = TestHome::new();
    let (engine, paths) = engine_for(&home);
    let (stop, serving) = hosted(engine).await;

    let second = Host::claim(Engine::with_paths(paths)).await;

    match second {
        Err(Error::AlreadyHosted { pid }) => {
            assert_eq!(
                pid,
                Some(std::process::id()),
                "the pid should name the holder"
            )
        }
        other => panic!("expected a refusal, got {other:?}", other = other.err()),
    }

    let _ = stop.send(());
    serving.await.unwrap();
}

#[tokio::test]
async fn a_lock_from_a_dead_host_does_not_block_the_machine() {
    let home = TestHome::new();
    let paths = Paths::new(home.path());

    // The kernel drops an flock when the file description closes, which is
    // what happens when a process dies. Dropping the guard is the same event.
    let first = Lock::acquire(&paths).unwrap();
    assert!(matches!(
        Lock::acquire(&paths),
        Err(Error::AlreadyHosted { .. })
    ));
    drop(first);

    Lock::acquire(&paths).expect("a released lock is available again");
    // The file is still on disk. Existence is deliberately not the test.
    assert!(paths.lock_file().exists());
}

#[tokio::test]
async fn a_socket_left_by_a_crashed_host_is_replaced() {
    let home = TestHome::new();
    let (engine, paths) = engine_for(&home);
    std::fs::create_dir_all(paths.run_dir()).unwrap();
    std::fs::write(paths.socket(), b"a corpse").unwrap();

    let (stop, serving) = hosted(engine).await;
    Client::connect(&paths).await.expect("connects anyway");

    let _ = stop.send(());
    serving.await.unwrap();
}

#[tokio::test]
async fn the_socket_is_reachable_only_by_its_owner() {
    let home = TestHome::new();
    let (engine, paths) = engine_for(&home);
    let (stop, serving) = hosted(engine).await;

    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&paths.socket()), 0o600);
    assert_eq!(mode(&paths.run_dir()), 0o700);

    let _ = stop.send(());
    serving.await.unwrap();
}

#[tokio::test]
async fn a_client_drives_the_hosted_engine() {
    let home = TestHome::new();
    let (engine, paths) = engine_for(&home);
    let spec = fake_spec(&home, "valkey@8", &[]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let (stop, serving) = hosted(engine.clone()).await;

    let mut client = Client::connect(&paths).await.unwrap();
    let started = client
        .send(&Request::Start {
            instance: id.clone(),
        })
        .await
        .unwrap();
    assert!(matches!(started, Response::Done), "{started:?}");

    // The host's engine is the one that changed, not a second copy.
    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Ready
    );
    let Response::Status { services } = client.send(&Request::Status).await.unwrap() else {
        panic!("expected a status")
    };
    assert_eq!(services.len(), 1);
    assert_eq!(services[0].state, ServiceState::Ready);

    let _ = stop.send(());
    serving.await.unwrap();
}

#[tokio::test]
async fn an_unknown_instance_comes_back_as_a_message_not_a_hang() {
    let home = TestHome::new();
    let (engine, paths) = engine_for(&home);
    let (stop, serving) = hosted(engine).await;

    let mut client = Client::connect(&paths).await.unwrap();
    let answer = client
        .send(&Request::Stop {
            instance: "ghost@1".parse().unwrap(),
        })
        .await
        .unwrap();

    match answer {
        Response::Failed { message } => assert!(message.contains("ghost@1"), "{message}"),
        other => panic!("expected a failure, got {other:?}"),
    }

    let _ = stop.send(());
    serving.await.unwrap();
}

#[tokio::test]
async fn a_client_from_another_version_is_told_what_to_do() {
    let home = TestHome::new();
    let (engine, paths) = engine_for(&home);
    let (stop, serving) = hosted(engine).await;

    // Speak the handshake by hand, as a client from a different release would.
    let stream = UnixStream::connect(paths.socket()).await.unwrap();
    let mut stream = BufReader::new(stream);
    stream
        .get_mut()
        .write_all(b"{\"protocol\":9999}\n")
        .await
        .unwrap();
    let mut reply = String::new();
    stream.read_line(&mut reply).await.unwrap();

    assert!(reply.contains("protocol 9999"), "{reply}");
    assert!(
        reply.contains("same version"),
        "the refusal should say what to do: {reply}"
    );

    let _ = stop.send(());
    serving.await.unwrap();
}

#[tokio::test]
async fn services_stop_with_their_host() {
    let home = TestHome::new();
    let (engine, _) = engine_for(&home);
    let spec = fake_spec(&home, "valkey@8", &[]);
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    engine.start(&id).await.unwrap();
    let pid = engine.status_of(&id).await.unwrap().pid.unwrap();

    let (stop, serving) = hosted(engine.clone()).await;
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), serving)
        .await
        .expect("the host winds down")
        .unwrap();

    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Stopped
    );
    // Gone for real, not merely marked stopped.
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!alive.success(), "the child outlived its host");
}
