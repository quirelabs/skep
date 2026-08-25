//! Readiness checks. Each one answers "is this service answering", never "has
//! enough time passed", which is what keeps boot times honest.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::spec::Probe;

const LOOPBACK: &str = "127.0.0.1";
const PROTOCOL_3: i32 = 196_608;

/// Runs one check. The error is the sentence a user ends up reading.
pub(crate) async fn check(probe: &Probe, limit: Duration) -> Result<(), String> {
    match probe {
        Probe::None => Ok(()),
        Probe::Tcp { port } => connect(*port, limit).await.map(drop),
        Probe::Resp { port } => {
            let reply = exchange(*port, b"PING\r\n", limit).await?;
            starts_with(&reply, b"+PONG", "a RESP PONG")
        }
        Probe::Http { port, path, expect } => {
            let request = format!(
                "GET {path} HTTP/1.1\r\nHost: {LOOPBACK}:{port}\r\nConnection: close\r\n\r\n"
            );
            let reply = exchange(*port, request.as_bytes(), limit).await?;
            let status = http_status(&reply)?;
            if status == *expect {
                Ok(())
            } else {
                Err(format!("answered {status}, expected {expect}"))
            }
        }
        Probe::Mysql { port } => {
            let greeting = listen_first(*port, limit).await?;
            // Byte four is the first payload byte: the protocol version, or
            // 0xff when the server is refusing rather than serving.
            match greeting.get(4) {
                Some(0x0a) => Ok(()),
                Some(0xff) => Err(mysql_error(&greeting)),
                Some(other) => Err(format!("answered with an unexpected packet {other:#04x}")),
                None => Err("closed the connection without a greeting".to_string()),
            }
        }
        Probe::Postgres {
            port,
            user,
            database,
        } => {
            let reply = exchange(*port, &startup_message(user, database), limit).await?;
            match reply.first() {
                // An authentication request means the postmaster is serving.
                Some(b'R') => Ok(()),
                Some(b'E') => Err(postgres_error(&reply)),
                Some(other) => Err(format!("unexpected reply {:?}", *other as char)),
                None => Err("closed the connection without answering".to_string()),
            }
        }
    }
}

async fn connect(port: u16, limit: Duration) -> Result<TcpStream, String> {
    match timeout(limit, TcpStream::connect((LOOPBACK, port))).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(error)) => Err(format!("port {port} refused the connection: {error}")),
        Err(_) => Err(format!("port {port} did not answer within {limit:?}")),
    }
}

async fn exchange(port: u16, request: &[u8], limit: Duration) -> Result<Vec<u8>, String> {
    let mut stream = connect(port, limit).await?;
    let talk = async {
        stream.write_all(request).await?;
        stream.flush().await?;
        let mut reply = vec![0u8; 512];
        let read = stream.read(&mut reply).await?;
        reply.truncate(read);
        Ok::<_, std::io::Error>(reply)
    };
    match timeout(limit, talk).await {
        Ok(Ok(reply)) => Ok(reply),
        Ok(Err(error)) => Err(format!("port {port} broke off: {error}")),
        Err(_) => Err(format!("port {port} did not reply within {limit:?}")),
    }
}

/// For protocols where the server speaks first.
async fn listen_first(port: u16, limit: Duration) -> Result<Vec<u8>, String> {
    let mut stream = connect(port, limit).await?;
    let listen = async {
        let mut reply = vec![0u8; 512];
        let read = stream.read(&mut reply).await?;
        reply.truncate(read);
        Ok::<_, std::io::Error>(reply)
    };
    match timeout(limit, listen).await {
        Ok(Ok(reply)) => Ok(reply),
        Ok(Err(error)) => Err(format!("port {port} broke off: {error}")),
        Err(_) => Err(format!("port {port} did not greet within {limit:?}")),
    }
}

/// A MySQL error packet: header, marker, a two byte code, an optional SQL
/// state marked with #, then the message.
fn mysql_error(packet: &[u8]) -> String {
    let body = &packet[packet.len().min(7)..];
    let body = match body.first() {
        Some(b'#') => &body[body.len().min(6)..],
        _ => body,
    };
    let message = String::from_utf8_lossy(body).trim().to_string();
    if message.is_empty() {
        "refused the connection".to_string()
    } else {
        message
    }
}

fn starts_with(reply: &[u8], prefix: &[u8], expected: &str) -> Result<(), String> {
    if reply.starts_with(prefix) {
        Ok(())
    } else {
        Err(format!(
            "expected {expected}, got {:?}",
            String::from_utf8_lossy(&reply[..reply.len().min(32)])
        ))
    }
}

fn http_status(reply: &[u8]) -> Result<u16, String> {
    let head = String::from_utf8_lossy(&reply[..reply.len().min(64)]);
    head.split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| format!("no HTTP status in {head:?}"))
}

/// The protocol 3 startup packet: length, version, then null terminated pairs.
fn startup_message(user: &str, database: &str) -> Vec<u8> {
    let mut body = Vec::new();
    for (key, value) in [("user", user), ("database", database)] {
        body.extend_from_slice(key.as_bytes());
        body.push(0);
        body.extend_from_slice(value.as_bytes());
        body.push(0);
    }
    body.push(0);

    let length = (8 + body.len()) as i32;
    let mut message = Vec::with_capacity(length as usize);
    message.extend_from_slice(&length.to_be_bytes());
    message.extend_from_slice(&PROTOCOL_3.to_be_bytes());
    message.extend_from_slice(&body);
    message
}

/// Postgres error fields are null terminated and tagged; M is the message.
fn postgres_error(reply: &[u8]) -> String {
    reply
        .split(|byte| *byte == 0)
        .find(|field| field.first() == Some(&b'M'))
        .map(|field| String::from_utf8_lossy(&field[1..]).to_string())
        .unwrap_or_else(|| "refused the startup packet".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_startup_packet_matches_the_protocol() {
        let message = startup_message("skep", "app");

        // 8 bytes of header plus a 24 byte body, and the length counts itself.
        assert_eq!(message.len(), 32);
        assert_eq!(i32::from_be_bytes(message[0..4].try_into().unwrap()), 32);
        assert_eq!(
            i32::from_be_bytes(message[4..8].try_into().unwrap()),
            PROTOCOL_3
        );
        assert_eq!(&message[8..], b"user\0skep\0database\0app\0\0");
    }

    #[test]
    fn a_postgres_error_reads_as_its_message() {
        let mut reply = vec![b'E', 0, 0, 0, 20];
        reply.extend_from_slice(b"SFATAL\0Mthe database system is starting up\0\0");

        assert_eq!(postgres_error(&reply), "the database system is starting up");
    }

    #[test]
    fn a_mysql_error_packet_reads_as_its_message() {
        let mut packet = vec![0x17, 0, 0, 0, 0xff, 0x69, 0x04];
        packet.extend_from_slice(b"#08S01Host is not allowed to connect");

        assert_eq!(
            mysql_error(&packet),
            "Host is not allowed to connect",
            "the SQL state marker should not reach the user"
        );
    }

    #[test]
    fn http_replies_are_read_for_their_status() {
        assert_eq!(http_status(b"HTTP/1.1 200 OK\r\n\r\n").unwrap(), 200);
        assert_eq!(http_status(b"HTTP/1.1 503 Nope\r\n\r\n").unwrap(), 503);
        assert!(http_status(b"garbage").is_err());
    }
}
