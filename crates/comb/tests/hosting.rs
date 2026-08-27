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

/// Everything worth knowing when a lock refuses to come free. flock is held per
/// open file description, so a process can block itself through a second
/// descriptor. The question this has to answer is whether the holder is us.
fn who_holds(path: &std::path::Path) -> String {
    let mine = std::process::id();
    let recorded: Option<u32> = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse().ok());
    let inode = std::fs::metadata(path)
        .map(|meta| std::os::unix::fs::MetadataExt::ino(&meta))
        .ok();
    // Every process holding the file, us included, so a self-conflict is
    // visible rather than inferred.
    let open_now = std::process::Command::new("lsof")
        .arg("--")
        .arg(path)
        .output()
        .map(|out| {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if text.is_empty() {
                "nobody has it open".to_string()
            } else {
                text
            }
        })
        .unwrap_or_else(|error| format!("lsof did not run: {error}"));

    format!(
        "\n  lock file: {path}\n  inode: {inode:?}\n  pid written in the file: {recorded:?}\n  \
         this process: {mine}\n  the file names this process: {same}\n  \
         processes holding it now:\n{open_now}",
        path = path.display(),
        same = recorded == Some(mine),
    )
}

#[tokio::test]
async fn a_lock_from_a_dead_host_does_not_block_the_machine() {
    let home = TestHome::new();
    let paths = Paths::new(home.path());

    // The kernel drops an flock when the file description closes, which is
    // what happens when a process dies. Dropping the guard is the same event.
    //
    // The refused attempt in the middle is not decoration. Release is only
    // ever late after a refusal, and that lateness is what Lock::acquire now
    // waits out.
    let first = Lock::acquire(&paths).unwrap();
    assert!(matches!(
        Lock::acquire(&paths),
        Err(Error::AlreadyHosted { .. })
    ));
    drop(first);

    if let Err(error) = Lock::acquire(&paths) {
        panic!(
            "a released lock should be available again, got {error:?}{}",
            who_holds(&paths.lock_file())
        );
    }

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
    let Response::Status { overview } = client.send(&Request::Status).await.unwrap() else {
        panic!("expected a status")
    };
    assert_eq!(overview.services.len(), 1);
    assert_eq!(overview.services[0].state, ServiceState::Ready);

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

#[tokio::test]
async fn a_new_host_takes_over_by_asking_rather_than_by_force() {
    let home = TestHome::new();
    let (leaving, paths) = engine_for(&home);
    let spec = fake_spec(&home, "valkey@8", &[]);
    let id = spec.id.clone();
    leaving.register(spec).await.unwrap();
    leaving.start(&id).await.unwrap();
    let pid = leaving.status_of(&id).await.unwrap().pid.unwrap();

    let host = Host::claim(leaving.clone()).await.unwrap();
    let serving = tokio::spawn(async move { host.serve(std::future::pending()).await });

    // The arriving host asks; it never touches the lock file itself.
    let arriving = Engine::with_paths(paths.clone());
    let taken = Host::take_over(arriving.clone(), Duration::from_secs(10))
        .await
        .expect("the machine should change hands");

    // The host that left took its services with it, as the handover requires.
    let _ = tokio::time::timeout(Duration::from_secs(10), serving).await;
    assert_eq!(
        leaving.status_of(&id).await.unwrap().state,
        ServiceState::Stopped
    );
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(
        !alive.success(),
        "the old host's child outlived the handover"
    );

    // And the new host really holds the machine now.
    let (stop, serving) = {
        let (stop, stopped) = oneshot::channel();
        let serving = tokio::spawn(async move {
            let _ = taken
                .serve(async {
                    let _ = stopped.await;
                })
                .await;
        });
        (stop, serving)
    };
    Client::connect(&paths).await.expect("the new host answers");
    assert!(matches!(
        Host::claim(Engine::with_paths(paths)).await,
        Err(Error::AlreadyHosted { .. })
    ));

    let _ = stop.send(());
    serving.await.unwrap();
}

#[tokio::test]
async fn taking_over_an_empty_machine_is_just_claiming_it() {
    let home = TestHome::new();
    let (engine, paths) = engine_for(&home);

    let host = Host::take_over(engine, Duration::from_secs(5))
        .await
        .expect("nothing to take over from");
    let (stop, stopped) = oneshot::channel();
    let serving = tokio::spawn(async move {
        let _ = host
            .serve(async {
                let _ = stopped.await;
            })
            .await;
    });

    Client::connect(&paths).await.expect("it is serving");
    let _ = stop.send(());
    serving.await.unwrap();
}
