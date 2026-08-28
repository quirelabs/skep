//! A DNS server that only ever gives one answer: everything under the managed
//! suffix is this machine. That is small enough to write out, and a real DNS
//! library would be far more surface than a server with one opinion needs.
//!
//! macOS reaches this through `/etc/resolver/<suffix>`, which is why it can
//! listen on an unprivileged port and still answer for a whole domain.

use std::net::Ipv4Addr;

use tokio::net::UdpSocket;

use crate::error::Result;

/// Fixed, because `/etc/resolver/<suffix>` records the port and a port that
/// moved every run would leave that file pointing at nothing.
pub const PORT: u16 = 15353;

/// IETF reserves `.test` for exactly this. `.dev` is Google's and HSTS
/// preloaded, and `.local` belongs to mDNS.
pub const SUFFIX: &str = "test";

const HEADER: usize = 12;
const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;
const CLASS_IN: u16 = 1;

/// Short, because a person adding a site should not wait out a cache.
const TTL: u32 = 60;

const HERE: Ipv4Addr = Ipv4Addr::LOCALHOST;
const HERE6: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];

/// Whether the system will actually send names here. A file that exists is not
/// the same as a file that points at us, and the difference is silent: names
/// simply fail to resolve and skep looks broken.
#[derive(Debug, PartialEq, Eq)]
pub enum Routing {
    /// Nothing routes the suffix anywhere yet.
    Missing,
    /// Something routes it, but not to this server. macOS reads an omitted
    /// port as 53, which skep cannot bind without root, so a file written for
    /// another tool lands here rather than looking fine.
    Elsewhere {
        says: String,
    },
    Ours,
}

/// Reads what the resolver file currently says.
pub fn routing(suffix: &str) -> Routing {
    let path = crate::platform::resolver_file(suffix);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Routing::Missing;
    };

    let mut nameserver = None;
    let mut port = 53;
    for line in text.lines() {
        let mut word = line.split_whitespace();
        match (word.next(), word.next()) {
            (Some("nameserver"), Some(value)) => nameserver = Some(value.to_string()),
            (Some("port"), Some(value)) => port = value.parse().unwrap_or(0),
            _ => {}
        }
    }

    if nameserver.as_deref() == Some("127.0.0.1") && port == PORT {
        Routing::Ours
    } else {
        Routing::Elsewhere {
            says: text.split_whitespace().collect::<Vec<_>>().join(" "),
        }
    }
}

/// Answers for `suffix` and everything under it until `shutdown` completes.
pub async fn serve(
    socket: UdpSocket,
    suffix: String,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<()> {
    // A query that does not fit in 512 bytes is not one we can answer anyway.
    let mut buffer = [0u8; 512];
    let shutdown = std::pin::pin!(shutdown);
    let mut shutdown = shutdown;
    loop {
        let received = tokio::select! {
            _ = &mut shutdown => return Ok(()),
            received = socket.recv_from(&mut buffer) => received,
        };
        let Ok((size, from)) = received else { continue };
        if let Some(answer) = reply(&buffer[..size], &suffix) {
            let _ = socket.send_to(&answer, from).await;
        }
    }
}

/// The whole protocol, as far as skep needs it. `None` means the datagram was
/// not a question worth answering, and silence is the right reply to that.
pub fn reply(query: &[u8], suffix: &str) -> Option<Vec<u8>> {
    if query.len() < HEADER {
        return None;
    }
    let id = u16::from_be_bytes([query[0], query[1]]);
    let flags = u16::from_be_bytes([query[2], query[3]]);

    // Answers are not ours to answer, and only standard queries are handled.
    if flags & 0x8000 != 0 || (flags >> 11) & 0xF != 0 {
        return None;
    }
    if u16::from_be_bytes([query[4], query[5]]) != 1 {
        return None;
    }

    let (name, after) = read_name(query, HEADER)?;
    let kind = u16::from_be_bytes([*query.get(after)?, *query.get(after + 1)?]);
    let class = u16::from_be_bytes([*query.get(after + 2)?, *query.get(after + 3)?]);
    let question = query.get(HEADER..after + 4)?;

    let suffix = suffix.trim_start_matches('.').to_ascii_lowercase();
    let ours = name == suffix || name.ends_with(&format!(".{suffix}"));

    let records = match (ours, class, kind) {
        (true, CLASS_IN, TYPE_A) => vec![record(TYPE_A, &HERE.octets())],
        (true, CLASS_IN, TYPE_AAAA) => vec![record(TYPE_AAAA, &HERE6)],
        // A name we own but a type we have nothing for is an empty answer, not
        // a denial: saying the name does not exist would be a lie that stops
        // the resolver asking for the type we do serve.
        _ => Vec::new(),
    };
    let code = if ours { 0 } else { 3 };

    // Authoritative, never recursive, echoing whether recursion was wanted.
    let answer_flags = 0x8000 | 0x0400 | (flags & 0x0100) | code;

    let mut out = Vec::with_capacity(HEADER + question.len() + records.len() * 16);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&answer_flags.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(records.len() as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(question);
    for record in records {
        out.extend_from_slice(&record);
    }
    Some(out)
}

fn record(kind: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + data.len());
    // Points back at the name in the question rather than repeating it.
    out.extend_from_slice(&0xC00Cu16.to_be_bytes());
    out.extend_from_slice(&kind.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    out.extend_from_slice(&TTL.to_be_bytes());
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
    out
}

/// Reads a question's name. Compression pointers are refused rather than
/// followed: they have no place in a question, and following offsets from an
/// unknown sender is how a parser is talked into a loop.
fn read_name(bytes: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    loop {
        let length = *bytes.get(at)? as usize;
        at += 1;
        if length == 0 {
            break;
        }
        if length & 0xC0 != 0 || length > 63 {
            return None;
        }
        let end = at.checked_add(length)?;
        let label = bytes.get(at..end)?;
        labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
        at = end;
        // Longer than a legal name means something is wrong with the sender.
        if labels.len() > 127 {
            return None;
        }
    }
    Some((labels.join("."), at))
}
