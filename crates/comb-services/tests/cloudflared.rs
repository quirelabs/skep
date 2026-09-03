//! Boots the real cloudflared binary through the engine and watches the
//! public url arrive on the event stream. Needs the network twice: once for
//! the release and once for the edge.

mod support;

use comb::{Engine, EventKind, Label, Paths, ServiceState, Version};
use comb_services::{Cloudflared, Origin, ServiceAdapter, TUNNEL_SUFFIX, install, share_spec};
use support::{heavy, shared_home};

#[tokio::test]
async fn a_quick_tunnel_announces_its_url_and_withdraws_it_on_stop() {
    if !heavy("cloudflared") {
        return;
    }
    let paths = Paths::new(shared_home());
    let version = Version::new(Cloudflared.default_version()).unwrap();
    install(&Cloudflared, &version, &paths).await.unwrap();

    let engine = Engine::with_paths(paths.clone());
    // Nothing needs to answer behind it for the edge to hand out a url.
    let spec = share_spec(&Label::new("nothing").unwrap(), Origin::service(1), &paths).unwrap();
    let id = spec.id.clone();
    assert!(id.is_target());
    engine.register(spec).await.unwrap();
    let mut events = engine.subscribe_events();

    engine.start(&id).await.unwrap();

    let said = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let event = events.recv().await.expect("the stream stays open");
            if let EventKind::Notice { text: Some(text) } = event.kind {
                return text;
            }
        }
    })
    .await
    .expect("the edge should hand out a url");
    assert!(
        said.starts_with("https://") && said.ends_with(TUNNEL_SUFFIX),
        "{said}"
    );
    assert_eq!(engine.status_of(&id).await.unwrap().notice, Some(said));

    engine.stop(&id).await.unwrap();
    let status = engine.status_of(&id).await.unwrap();
    assert_eq!(status.state, ServiceState::Stopped);
    assert_eq!(status.notice, None);
}
