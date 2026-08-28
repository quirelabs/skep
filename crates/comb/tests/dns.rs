//! The resolver, as bytes in and bytes out, plus one round trip through a real
//! socket and dig so something other than our own parser agrees.

use std::process::Command;

use comb::dns_reply;
use tokio::net::UdpSocket;

const SUFFIX: &str = "test";
const A: u16 = 1;
const AAAA: u16 = 28;
const MX: u16 = 15;

fn question(name: &str, kind: u16) -> Vec<u8> {
    // id, recursion desired, one question and nothing else.
    let mut out = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in name.split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&kind.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out
}

fn code(answer: &[u8]) -> u8 {
    answer[3] & 0x0F
}

fn answers(answer: &[u8]) -> u16 {
    u16::from_be_bytes([answer[6], answer[7]])
}

/// The address in the single record we ever send, read by walking the message
/// rather than by counting backwards from the end.
fn address(answer: &[u8]) -> &[u8] {
    let mut at = 12;
    while answer[at] != 0 {
        at += 1 + answer[at] as usize;
    }
    // The terminating zero, then qtype and qclass, then the record's pointer,
    // type, class and ttl.
    at += 1 + 4 + 10;
    let length = u16::from_be_bytes([answer[at], answer[at + 1]]) as usize;
    &answer[at + 2..at + 2 + length]
}

#[test]
fn a_name_under_the_suffix_is_this_machine() {
    let answer = dns_reply(&question("myapp.test", A), SUFFIX).unwrap();

    assert_eq!(code(&answer), 0);
    assert_eq!(answers(&answer), 1);
    assert_eq!(address(&answer), [127, 0, 0, 1]);
    // Authoritative, and the id came back.
    assert_eq!(&answer[0..2], &[0x12, 0x34]);
    assert_ne!(answer[2] & 0x04, 0, "the answer should be authoritative");
}

#[test]
fn the_suffix_itself_is_answered_too() {
    let answer = dns_reply(&question("test", A), SUFFIX).unwrap();
    assert_eq!(code(&answer), 0);
    assert_eq!(answers(&answer), 1);
}

#[test]
fn a_deep_name_is_still_ours() {
    let answer = dns_reply(&question("api.staging.myapp.test", A), SUFFIX).unwrap();
    assert_eq!(answers(&answer), 1);
    assert_eq!(address(&answer), [127, 0, 0, 1]);
}

#[test]
fn an_ipv6_question_gets_loopback() {
    let answer = dns_reply(&question("myapp.test", AAAA), SUFFIX).unwrap();
    assert_eq!(answers(&answer), 1);
    assert_eq!(
        address(&answer),
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]
    );
}

#[test]
fn a_name_outside_the_suffix_does_not_exist() {
    let answer = dns_reply(&question("example.com", A), SUFFIX).unwrap();
    assert_eq!(code(&answer), 3, "anything else is not ours to claim");
    assert_eq!(answers(&answer), 0);
}

#[test]
fn a_name_that_merely_ends_in_the_letters_is_not_ours() {
    // "notatest" ends with "test" as text but is a different domain.
    let answer = dns_reply(&question("notatest", A), SUFFIX).unwrap();
    assert_eq!(code(&answer), 3);
}

#[test]
fn a_type_we_do_not_serve_is_an_empty_answer_not_a_denial() {
    let answer = dns_reply(&question("myapp.test", MX), SUFFIX).unwrap();
    // Saying the name does not exist would stop the resolver ever asking for
    // the type we do serve.
    assert_eq!(code(&answer), 0);
    assert_eq!(answers(&answer), 0);
}

#[test]
fn rubbish_gets_silence() {
    // Too short to be a header.
    assert!(dns_reply(&[0, 1, 2], SUFFIX).is_none());

    // An answer, not a question.
    let mut reply_shaped = question("myapp.test", A);
    reply_shaped[2] |= 0x80;
    assert!(dns_reply(&reply_shaped, SUFFIX).is_none());

    // A compression pointer where a question's name should be.
    let mut pointer = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
    pointer.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1]);
    assert!(dns_reply(&pointer, SUFFIX).is_none());

    // A name that runs off the end of the datagram.
    let mut truncated = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
    truncated.push(40);
    truncated.extend_from_slice(b"short");
    assert!(dns_reply(&truncated, SUFFIX).is_none());
}

#[tokio::test]
async fn dig_gets_an_answer_it_understands() {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = comb::serve_dns(socket, SUFFIX.to_string(), async {
            let _ = stopped.await;
        })
        .await;
    });

    // macOS ships dig, and a missing one means this check is not happening.
    let output = tokio::task::spawn_blocking(move || {
        Command::new("dig")
            .args([
                "@127.0.0.1",
                "-p",
                &port.to_string(),
                "+short",
                "+timeout=2",
                "myapp.test",
            ])
            .output()
            .expect("macOS ships dig, so a missing one is a real failure")
    })
    .await
    .unwrap();

    let said = String::from_utf8_lossy(&output.stdout);
    assert_eq!(said.trim(), "127.0.0.1", "dig said: {said}");
    let _ = stop.send(());
}
