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

/// Sends a message the way an application would, so the reader is tested
/// against mail that actually went through smtp rather than a fixture.
fn post(smtp: u16, subject: &str, body: &str, html: bool) {
    use std::io::{BufRead, BufReader, Write};

    let stream = TcpStream::connect(("127.0.0.1", smtp)).expect("smtp is listening");
    let mut reading = BufReader::new(stream.try_clone().unwrap());
    let mut writing = stream;
    let mut line = String::new();
    let expect = |reading: &mut BufReader<TcpStream>, line: &mut String| {
        line.clear();
        reading.read_line(line).expect("the server answers");
    };

    expect(&mut reading, &mut line);
    let kind = if html { "text/html" } else { "text/plain" };
    for step in [
        "HELO skep\r\n".to_string(),
        "MAIL FROM:<hello@myapp.test>\r\n".to_string(),
        "RCPT TO:<you@example.test>\r\n".to_string(),
        "DATA\r\n".to_string(),
    ] {
        writing.write_all(step.as_bytes()).unwrap();
        expect(&mut reading, &mut line);
    }
    let message = format!(
        "From: hello@myapp.test\r\nTo: you@example.test\r\nSubject: {subject}\r\n\
         Content-Type: {kind}; charset=utf-8\r\n\r\n{body}\r\n.\r\n"
    );
    writing.write_all(message.as_bytes()).unwrap();
    expect(&mut reading, &mut line);
    writing.write_all(b"QUIT\r\n").unwrap();
}

#[tokio::test]
async fn what_was_caught_can_be_listed_read_and_searched() {
    let (engine, id, ports) = registered(&Mailpit, "reads", &["http", "smtp"]).await;
    let (http, smtp) = (ports[0], ports[1]);
    start(&engine, &id).await;

    // The data directory outlives a run, so anything an earlier one caught is
    // still in there. Counting messages means starting from none.
    comb_services::mail::clear(http).await.unwrap();

    post(smtp, "Welcome aboard", "Your account is ready.", false);
    post(
        smtp,
        "Reset your password",
        "<html><body><p>Click <a href=\"https://myapp.test/r?t=42\">here</a>.</p></body></html>",
        true,
    );

    let (inbox, unread) = comb_services::mail::inbox(http, 20).await.unwrap();
    assert_eq!(inbox.len(), 2, "both messages should be caught");
    assert_eq!(unread, 2);
    assert!(inbox.iter().any(|one| one.subject == "Welcome aboard"));
    assert!(inbox.iter().all(|one| one.from.contains("myapp.test")));

    // The html message has no plain part, so reading it has to produce words
    // rather than tags, and keep the link that is the point of it. Mailpit
    // converts html itself and does it well, so this usually passes without
    // our own converter running at all; that one is the fallback for when the
    // plain part comes back empty anyway.
    let html_one = inbox
        .iter()
        .find(|one| one.subject == "Reset your password")
        .expect("the html message is there");
    let opened = comb_services::mail::read(http, &html_one.id).await.unwrap();
    assert!(!opened.text.contains('<'), "read as words: {}", opened.text);
    assert!(
        opened.text.contains("https://myapp.test/r?t=42"),
        "the link is the point of the message: {}",
        opened.text
    );

    let found = comb_services::mail::search(http, "password", 10)
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].subject, "Reset your password");

    comb_services::mail::clear(http).await.unwrap();
    let (empty, _) = comb_services::mail::inbox(http, 20).await.unwrap();
    assert!(empty.is_empty(), "clearing should leave nothing behind");

    engine.stop(&id).await.unwrap();
}
