//! What a service announces rides the event stream, lasts as long as the
//! service does, and is withdrawn on the same stream when it ends.

mod support;

use comb::{Engine, EventKind, Mirror, Notice, ServiceState};
use support::{TestHome, fake_spec, wait_for_event};

#[tokio::test]
async fn a_notice_is_kept_while_the_service_is_up_and_withdrawn_when_it_ends() {
    let home = TestHome::new();
    let engine = Engine::new();
    // The fake announces its own pid; that line is the notice.
    let spec = fake_spec(&home, "valkey@8", &[]).with_notice(Notice::new("pid="));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut events = engine.subscribe_events();

    engine.start(&id).await.unwrap();
    let said = wait_for_event(&mut events, |kind| {
        matches!(kind, EventKind::Notice { text: Some(_) })
    })
    .await;
    let status = engine.status_of(&id).await.unwrap();
    let expected = format!("pid={}", status.pid.unwrap());
    assert_eq!(
        said.kind,
        EventKind::Notice {
            text: Some(expected.clone())
        }
    );
    assert_eq!(status.notice.as_deref(), Some(expected.as_str()));

    engine.stop(&id).await.unwrap();
    wait_for_event(&mut events, |kind| {
        matches!(kind, EventKind::Notice { text: None })
    })
    .await;
    let status = engine.status_of(&id).await.unwrap();
    assert_eq!(status.state, ServiceState::Stopped);
    assert_eq!(status.notice, None, "a notice must not outlive its service");
}

#[tokio::test]
async fn a_replica_carries_the_notice_and_drops_it_in_lockstep() {
    let home = TestHome::new();
    let engine = Engine::new();
    let spec = fake_spec(&home, "valkey@8", &[]).with_notice(Notice::new("pid="));
    let id = spec.id.clone();
    engine.register(spec).await.unwrap();
    let mut mirror = Mirror::new();
    mirror.reset(engine.overview().await);
    let mut events = engine.subscribe_events();
    let mut waiting = engine.subscribe_events();

    engine.start(&id).await.unwrap();
    // The announcement lands on its own beat after the start returns.
    wait_for_event(&mut waiting, |kind| {
        matches!(kind, EventKind::Notice { text: Some(_) })
    })
    .await;
    engine.stop(&id).await.unwrap();

    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        mirror.apply(&event);
        if let EventKind::Notice { text } = &event.kind {
            seen.push(text.is_some());
        }
    }
    assert_eq!(seen, [true, false], "announced once, withdrawn once");
    assert_eq!(mirror.get(&id).unwrap().notice, None);
}
