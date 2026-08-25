//! Boots the real Mailpit binary through the engine.

mod support;

use std::net::TcpStream;

use comb::ServiceState;
use comb_services::Mailpit;
use support::{curl, history, registered, start};

#[tokio::test]
async fn boots_and_answers_on_both_ports() {
    let (engine, id, ports) = registered(&Mailpit, "serves", &["http", "smtp"]).await;
    let (http, smtp) = (ports[0], ports[1]);

    start(&engine, &id).await;

    let status = engine.status_of(&id).await.unwrap();
    assert_eq!(status.ports["http"], http);
    assert_eq!(status.ports["smtp"], smtp);

    // Ready only means the probe answered. Check the real API and the SMTP
    // listener too, so a probe that passed for the wrong reason is caught.
    assert!(
        curl(&format!("http://127.0.0.1:{http}/api/v1/info")).contains("Version"),
        "the HTTP API should be serving"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", smtp)).is_ok(),
        "the SMTP port should be listening"
    );

    engine.stop(&id).await.unwrap();
    assert_eq!(
        engine.status_of(&id).await.unwrap().state,
        ServiceState::Stopped
    );
}

#[tokio::test]
async fn stopping_hands_the_ports_back() {
    let (engine, id, ports) = registered(&Mailpit, "releases", &["http", "smtp"]).await;
    start(&engine, &id).await;
    engine.stop(&id).await.unwrap();

    assert!(
        TcpStream::connect(("127.0.0.1", ports[0])).is_err(),
        "the port should be free once the service is stopped"
    );
}

#[tokio::test]
async fn its_logs_reach_the_engine() {
    let (engine, id, ports) = registered(&Mailpit, "logs", &["http", "smtp"]).await;
    start(&engine, &id).await;

    // The banner names the interface it bound, so this also proves the spec
    // kept it off every interface but loopback.
    let text = history(&engine, &id).await;
    assert!(
        text.contains(&format!("[http] starting on 127.0.0.1:{}", ports[0])),
        "expected the http banner, got {text}"
    );
    assert!(
        text.contains(&format!("[smtpd] starting on 127.0.0.1:{}", ports[1])),
        "expected the smtp banner, got {text}"
    );

    engine.stop(&id).await.unwrap();
}
