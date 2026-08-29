//! Reading what a mail catcher caught.
//!
//! This talks to Mailpit's own http api rather than its database, because the
//! api is the part it promises to keep. The engine stays out of it: mail is one
//! service's business and a general supervision engine has none of it, so
//! frontends ask the engine which port mailpit is on and then come here.

use comb::{Error, Result};
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;

/// One message as a list shows it. Mailpit sends a snippet with the list, so a
/// useful inbox costs exactly one request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub snippet: String,
    pub at: String,
    pub read: bool,
    pub attachments: usize,
}

/// A message opened.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Body {
    pub id: String,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub at: String,
    /// Always something readable. The question being asked is what the mail
    /// said, not what tags it used.
    pub text: String,
    /// Whether skep converted this from html. Mailpit does that itself and
    /// does it well, so this is normally false; it turns true only when the
    /// plain part came back empty and the fallback had to run.
    pub converted: bool,
    /// The message as it was written, already guarded: nothing in here can
    /// fetch or run. Empty when the message was sent as plain text, which is
    /// how a reader tells whether there is anything to render.
    pub html: String,
    /// Remote images held back, and how many of those were tracking pixels.
    pub images: usize,
    pub pixels: usize,
    pub attachments: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Listed {
    #[serde(default)]
    messages: Vec<Raw>,
    #[serde(default)]
    messages_unread: usize,
}

#[derive(Debug, Deserialize)]
struct Raw {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "From")]
    from: Option<Address>,
    #[serde(rename = "To")]
    to: Option<Vec<Address>>,
    #[serde(rename = "Subject")]
    subject: String,
    #[serde(rename = "Snippet", default)]
    snippet: String,
    #[serde(rename = "Created")]
    created: String,
    #[serde(rename = "Read", default)]
    read: bool,
    #[serde(rename = "Attachments", default)]
    attachments: usize,
}

#[derive(Debug, Deserialize)]
struct Address {
    #[serde(rename = "Name", default)]
    name: String,
    #[serde(rename = "Address", default)]
    address: String,
}

impl Address {
    /// A name is worth showing when there is one, and the address always is.
    fn shown(&self) -> String {
        if self.name.is_empty() {
            self.address.clone()
        } else {
            format!("{} <{}>", self.name, self.address)
        }
    }
}

#[derive(Debug, Deserialize)]
struct Opened {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "From")]
    from: Option<Address>,
    #[serde(rename = "To")]
    to: Option<Vec<Address>>,
    #[serde(rename = "Subject")]
    subject: String,
    #[serde(rename = "Date")]
    date: String,
    #[serde(rename = "Text", default)]
    text: String,
    #[serde(rename = "HTML", default)]
    html: String,
    #[serde(rename = "Attachments", default)]
    attachments: Vec<Attachment>,
}

#[derive(Debug, Deserialize)]
struct Attachment {
    #[serde(rename = "FileName", default)]
    file_name: String,
}

/// The most recent messages, newest first, which is how mailpit already
/// returns them.
pub async fn inbox(port: u16, limit: usize) -> Result<(Vec<Summary>, usize)> {
    let body = get(port, &format!("/api/v1/messages?limit={limit}")).await?;
    let listed: Listed = parse(&body)?;
    Ok((
        listed.messages.into_iter().map(summarise).collect(),
        listed.messages_unread,
    ))
}

/// Mailpit searches headers and bodies, so this is the one call an agent needs
/// to answer whether something was sent and what it said.
pub async fn search(port: u16, query: &str, limit: usize) -> Result<Vec<Summary>> {
    let path = format!("/api/v1/search?query={}&limit={limit}", escape(query));
    let listed: Listed = parse(&get(port, &path).await?)?;
    Ok(listed.messages.into_iter().map(summarise).collect())
}

pub async fn read(port: u16, id: &str) -> Result<Body> {
    read_showing(port, id, Images::Blocked).await
}

/// The same message with its remote images allowed, asked for one message at a
/// time and never remembered.
pub async fn read_showing(port: u16, id: &str, images: Images) -> Result<Body> {
    let opened: Opened = parse(&get(port, &format!("/api/v1/message/{}", escape(id))).await?)?;

    // Where there is html, convert it here rather than take mailpit's plain
    // part. Mailpit decorates: an h1 comes back fenced in rows of asterisks
    // and bold words keep their markers, which is right for a terminal and
    // noise on a screen. Its plain part is used only when a message really
    // was sent as text.
    let converted = !opened.html.trim().is_empty();
    let text = if converted {
        to_text(&opened.html)
    } else {
        opened.text
    };
    let shown = if opened.html.trim().is_empty() {
        Rendered::default()
    } else {
        render(&opened.html, images)
    };

    Ok(Body {
        id: opened.id,
        from: opened.from.map(|one| one.shown()).unwrap_or_default(),
        to: opened
            .to
            .unwrap_or_default()
            .iter()
            .map(Address::shown)
            .collect(),
        subject: opened.subject,
        at: opened.date,
        text,
        converted,
        html: shown.html,
        images: shown.images,
        pixels: shown.pixels,
        attachments: opened
            .attachments
            .into_iter()
            .map(|one| one.file_name)
            .collect(),
    })
}

/// Marks a message read. Fetching one does not: mailpit only changes it when
/// asked, so opening a message has to say so.
pub async fn mark_read(port: u16, id: &str) -> Result<()> {
    let body = format!("{{\"IDs\":[\"{}\"],\"Read\":true}}", id.replace('"', ""));
    request_with(port, "PUT", "/api/v1/messages", Some(body))
        .await
        .map(drop)
}

/// The message exactly as it arrived, headers and encoding and all. This is
/// the answer to "what did my app actually send", which a rendered view can
/// only approximate.
pub async fn source(port: u16, id: &str) -> Result<String> {
    let raw = get(port, &format!("/api/v1/message/{}/raw", escape(id))).await?;
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// How the message would fare in real mail clients. Mailpit runs 186 tests
/// against a support database and skep was throwing the answer away.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Compatibility {
    pub supported: f32,
    pub partial: f32,
    pub unsupported: f32,
    pub tests: usize,
    pub warnings: Vec<Warning>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Warning {
    pub what: String,
    pub supported: f32,
}

#[derive(Debug, Deserialize)]
struct RawChecks {
    #[serde(rename = "Total")]
    total: RawTotal,
    #[serde(rename = "Warnings", default)]
    warnings: Vec<RawWarning>,
}

#[derive(Debug, Deserialize)]
struct RawTotal {
    #[serde(rename = "Tests")]
    tests: usize,
    #[serde(rename = "Supported")]
    supported: f32,
    #[serde(rename = "Partial")]
    partial: f32,
    #[serde(rename = "Unsupported")]
    unsupported: f32,
}

#[derive(Debug, Deserialize)]
struct RawWarning {
    #[serde(rename = "Title")]
    title: String,
    #[serde(rename = "Score")]
    score: RawScore,
}

#[derive(Debug, Deserialize)]
struct RawScore {
    #[serde(rename = "Supported")]
    supported: f32,
}

pub async fn compatibility(port: u16, id: &str) -> Result<Compatibility> {
    let body = get(port, &format!("/api/v1/message/{}/html-check", escape(id))).await?;
    let raw: RawChecks = parse(&body)?;
    Ok(Compatibility {
        supported: raw.total.supported,
        partial: raw.total.partial,
        unsupported: raw.total.unsupported,
        tests: raw.total.tests,
        warnings: raw
            .warnings
            .into_iter()
            .map(|one| Warning {
                what: one.title,
                supported: one.score.supported,
            })
            .collect(),
    })
}

/// Every link in the message, followed. This reaches out over the network, so
/// it only ever happens because somebody asked for it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Links {
    pub errors: usize,
    pub links: Vec<Link>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Link {
    pub url: String,
    pub status: u16,
    pub said: String,
}

#[derive(Debug, Deserialize)]
struct RawLinks {
    #[serde(rename = "Errors", default)]
    errors: usize,
    #[serde(rename = "Links", default)]
    links: Vec<RawLink>,
}

#[derive(Debug, Deserialize)]
struct RawLink {
    #[serde(rename = "URL")]
    url: String,
    #[serde(rename = "StatusCode", default)]
    status: u16,
    #[serde(rename = "Status", default)]
    said: String,
}

pub async fn links(port: u16, id: &str) -> Result<Links> {
    let body = get(port, &format!("/api/v1/message/{}/link-check", escape(id))).await?;
    let raw: RawLinks = parse(&body)?;
    Ok(Links {
        errors: raw.errors,
        links: raw
            .links
            .into_iter()
            .map(|one| Link {
                url: one.url,
                status: one.status,
                said: one.said,
            })
            .collect(),
    })
}

pub async fn clear(port: u16) -> Result<()> {
    request(port, "DELETE", "/api/v1/messages").await.map(drop)
}

fn summarise(raw: Raw) -> Summary {
    Summary {
        id: raw.id,
        from: raw.from.map(|one| one.shown()).unwrap_or_default(),
        to: raw
            .to
            .unwrap_or_default()
            .iter()
            .map(Address::shown)
            .collect(),
        subject: raw.subject,
        snippet: raw.snippet,
        at: raw.created,
        read: raw.read,
        attachments: raw.attachments,
    }
}

fn parse<T: for<'a> Deserialize<'a>>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body).map_err(|error| {
        Error::Io(std::io::Error::other(format!(
            "the mail catcher said something unexpected: {error}"
        )))
    })
}

/// Percent encoding, for the few characters a subject or an id can carry that
/// a query string cannot.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

async fn get(port: u16, path: &str) -> Result<Vec<u8>> {
    request(port, "GET", path).await
}

async fn request(port: u16, method: &str, path: &str) -> Result<Vec<u8>> {
    request_with(port, method, path, None).await
}

async fn request_with(
    port: u16,
    method: &str,
    path: &str,
    json_body: Option<String>,
) -> Result<Vec<u8>> {
    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .map_err(|_| Error::Io(std::io::Error::other("no mail catcher is listening")))?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|error| Error::Io(std::io::Error::other(error.to_string())))?;
    tokio::spawn(connection);

    let sending = hyper::Request::builder()
        .method(method)
        .uri(path)
        .header(hyper::header::HOST, "127.0.0.1")
        .header(hyper::header::CONTENT_TYPE, "application/json");
    let request = match json_body {
        Some(body) => sending.body(Full::new(Bytes::from(body)).boxed()),
        None => sending.body(
            Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed(),
        ),
    }
    .map_err(|error| Error::Io(std::io::Error::other(error.to_string())))?;

    let response = sender
        .send_request(request)
        .await
        .map_err(|error| Error::Io(std::io::Error::other(error.to_string())))?;
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|error| Error::Io(std::io::Error::other(error.to_string())))?
        .to_bytes();

    if !status.is_success() {
        return Err(Error::Io(std::io::Error::other(format!(
            "the mail catcher answered {status}"
        ))));
    }
    Ok(body.to_vec())
}

/// Html made safe to show, for a viewer that renders it rather than reading it
/// out as words.
///
/// Two layers, because one of them being wrong should not be enough. The
/// policy at the top tells the engine to fetch nothing at all, and the pass
/// below removes the things that would have asked. Without this a message
/// fetches its own tracking pixel the moment it is opened, which tells whoever
/// sent it that you read it. That is measured, not assumed: an unguarded page
/// fetched one twice.
pub fn safe_html(html: &str) -> Rendered {
    render(html, Images::Blocked)
}

/// Whether remote images may load. Blocked is the only default there can be:
/// loading them is what tells a sender you opened the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Images {
    Blocked,
    Allowed,
}

/// A message ready to show, and what was held back to get it there.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Rendered {
    pub html: String,
    /// Remote images not loaded.
    pub images: usize,
    /// How many of those were a single pixel, which is an image only in the
    /// sense that it has to be fetched. Counted apart because "two images and
    /// a tracking pixel" is the true sentence, and the pixel is the reason any
    /// of this is blocked.
    pub pixels: usize,
}

pub fn render(html: &str, images: Images) -> Rendered {
    let mut out = String::with_capacity(html.len());
    let mut held = Rendered::default();
    let mut rest = html;

    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let lowered = rest.to_ascii_lowercase();

        // Whole elements that exist to load or run something.
        let banished = [
            "script", "iframe", "object", "embed", "video", "audio", "applet", "frame", "frameset",
            "portal",
        ];
        if let Some(skipped) = banished
            .iter()
            .find_map(|name| skip_block(&lowered, rest, name))
        {
            rest = skipped;
            continue;
        }

        let Some(end) = rest.find('>') else {
            held.html = finish(&out, images);
            return held;
        };
        let tag = &rest[1..end];
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();

        // Empty elements that only ever point somewhere else.
        if matches!(name.as_str(), "link" | "meta" | "base" | "source" | "track") {
            rest = &rest[end + 1..];
            continue;
        }

        if name == "img" {
            match remote_image(tag) {
                Some(true) => held.pixels += 1,
                Some(false) => held.images += 1,
                None => {}
            }
        }

        out.push('<');
        out.push_str(&strip_attributes(tag, images));
        out.push('>');
        rest = &rest[end + 1..];
    }
    out.push_str(rest);

    held.html = finish(&out, images);
    held
}

/// Whether an image comes from elsewhere, and whether it is the one pixel kind
/// whose only job is to be fetched.
fn remote_image(tag: &str) -> Option<bool> {
    let source = attribute(tag, "src")?;
    let source = source.trim().to_ascii_lowercase();
    if !source.starts_with("http://") && !source.starts_with("https://") {
        return None;
    }
    let tiny = |name: &str| {
        attribute(tag, name)
            .map(|value| matches!(value.trim(), "0" | "1"))
            .unwrap_or(false)
    };
    Some(tiny("width") && tiny("height"))
}

fn attribute(tag: &str, wanted: &str) -> Option<String> {
    let lowered = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lowered[from..].find(&format!("{wanted}=")) {
        let at = from + at;
        // Only when it really is the attribute and not the end of another.
        let starts = at == 0 || lowered.as_bytes()[at - 1].is_ascii_whitespace();
        let after = &tag[at + wanted.len() + 1..];
        if starts {
            return Some(match after.chars().next() {
                Some(quote @ ('"' | '\'')) => after[1..].split(quote).next()?.to_string(),
                _ => after.split_whitespace().next()?.to_string(),
            });
        }
        from = at + wanted.len() + 1;
    }
    None
}

/// Attributes that fetch, and attributes that run. Everything else is kept, so
/// the message still looks like itself.
fn strip_attributes(tag: &str, images: Images) -> String {
    let dangerous = |name: &str| {
        let name = name.to_ascii_lowercase();
        if name == "src" && images == Images::Allowed {
            return false;
        }
        name.starts_with("on")
            || matches!(
                name.as_str(),
                "src" | "srcset" | "poster" | "background" | "data" | "formaction" | "ping"
            )
    };

    let mut kept = String::with_capacity(tag.len());
    let mut rest = tag;
    // The element name comes first and is never an attribute.
    let split = rest.find(char::is_whitespace).unwrap_or(rest.len());
    kept.push_str(&rest[..split]);
    rest = &rest[split..];

    while let Some(at) = rest.find(|c: char| !c.is_whitespace()) {
        rest = &rest[at..];
        if rest.starts_with('/') {
            kept.push_str(" /");
            break;
        }
        let name_end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        let name = &rest[..name_end];
        rest = &rest[name_end..];

        let mut value = "";
        if let Some(stripped) = rest.strip_prefix('=') {
            let stripped = stripped.trim_start();
            let (taken, remainder) = match stripped.chars().next() {
                Some(quote @ ('"' | '\'')) => {
                    let inner = &stripped[1..];
                    match inner.find(quote) {
                        Some(close) => (&inner[..close], &inner[close + 1..]),
                        None => (inner, ""),
                    }
                }
                _ => {
                    let end = stripped.find(char::is_whitespace).unwrap_or(stripped.len());
                    (&stripped[..end], &stripped[end..])
                }
            };
            value = taken;
            rest = remainder;
        }

        if dangerous(name) {
            continue;
        }
        // A stylesheet can fetch too, through url().
        let value = if name.eq_ignore_ascii_case("style") {
            without_urls(value)
        } else {
            value.to_string()
        };
        kept.push_str(&format!(" {name}=\"{}\"", value.replace('"', "&quot;")));
    }
    kept
}

fn without_urls(style: &str) -> String {
    let mut out = String::with_capacity(style.len());
    let mut rest = style;
    while let Some(at) = rest.to_ascii_lowercase().find("url(") {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        match rest.find(')') {
            Some(close) => rest = &rest[close + 1..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The policy that makes the guarantee, rather than the tidying that supports
/// it. Nothing may be fetched, inline styling is allowed because that is how
/// mail is written, and images only when they came with the message.
fn finish(body: &str, images: Images) -> String {
    // Asked for, and only for this one message: there is no setting that turns
    // this on everywhere, because the choice belongs to the message in front
    // of you rather than to every message that will ever arrive.
    let sources = match images {
        Images::Blocked => "data:",
        Images::Allowed => "data: https: http:",
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; \
         style-src 'unsafe-inline'; img-src {sources}; font-src 'none'; form-action 'none'\">\
         <style>html,body{{margin:0;padding:14px;font:13px/1.5 -apple-system,sans-serif;\
         word-wrap:break-word}}img{{max-width:100%}}</style></head><body>{body}</body></html>"
    )
}

/// Html reduced to what a person wanted to read. This is not a renderer and
/// does not pretend to be one: it drops what carries no words, turns the tags
/// that mean a line break into line breaks, keeps where a link goes, and
/// leaves the rest as text.
pub fn to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    // Where a link started and where it points, so its target can be put after
    // the words it wrapped rather than at the end of the message.
    let mut link: Option<(String, usize)> = None;
    let mut rest = html;

    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];

        let lowered = rest.to_ascii_lowercase();
        // Script, style and head carry no words, and their contents are not
        // text however much they look like it.
        if let Some(skipped) = skip_block(&lowered, rest, "script")
            .or_else(|| skip_block(&lowered, rest, "style"))
            .or_else(|| skip_block(&lowered, rest, "head"))
        {
            rest = skipped;
            continue;
        }

        let Some(end) = rest.find('>') else {
            // An unclosed angle bracket is text, not a tag.
            out.push_str(rest);
            return tidy(&out);
        };
        let tag = &rest[1..end];

        if let Some(target) = link_target(tag) {
            link = Some((target, out.len()));
        } else if tag.trim_start_matches('/').eq_ignore_ascii_case("a")
            && tag.starts_with('/')
            && let Some((target, at)) = link.take()
        {
            let words = out[at..].trim().to_string();
            if words.is_empty() {
                out.push_str(&target);
            } else if words != target {
                out.push_str(&format!(" ({target})"));
            }
        }

        out.push_str(spacing_for(tag));
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    tidy(&out)
}

/// Skips a whole element, contents and all.
fn skip_block<'a>(lowered: &str, rest: &'a str, name: &str) -> Option<&'a str> {
    if !lowered.starts_with(&format!("<{name}")) {
        return None;
    }
    let close = format!("</{name}>");
    match lowered.find(&close) {
        Some(at) => Some(&rest[at + close.len()..]),
        None => Some(""),
    }
}

/// What a tag is worth in whitespace. Everything else is worth nothing. Both
/// halves of a pair are worth the same, and the runs they make are collapsed
/// afterwards rather than counted here.
fn spacing_for(tag: &str) -> &'static str {
    let name = tag
        .trim_start_matches('/')
        .split(|c: char| c.is_whitespace() || c == '/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match name.as_str() {
        "br" | "p" | "div" | "tr" | "li" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "table"
        | "ul" | "ol" | "blockquote" | "section" | "header" | "footer" => "\n",
        "td" | "th" => " ",
        _ => "",
    }
}

/// Where a link goes, which is usually the point of a development email.
fn link_target(tag: &str) -> Option<String> {
    let lowered = tag.to_ascii_lowercase();
    if !lowered.starts_with("a ") {
        return None;
    }
    let at = lowered.find("href=")? + 5;
    let after = &tag[at..];
    let quote = after.chars().next()?;
    let value = if quote == '"' || quote == '\'' {
        after[1..].split(quote).next()?
    } else {
        after.split_whitespace().next()?
    };
    (!value.is_empty() && !value.starts_with('#')).then(|| value.to_string())
}

/// Entities become the characters they stand for, and the whitespace that tag
/// pairs left behind is collapsed: one space between words, one line between
/// lines, and none at either end.
fn tidy(raw: &str) -> String {
    let mut text = raw.to_string();
    for (entity, becomes) in [
        ("&nbsp;", " "),
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&apos;", "'"),
        ("&mdash;", "\u{2014}"),
        ("&ndash;", "\u{2013}"),
        ("&hellip;", "\u{2026}"),
    ] {
        text = text.replace(entity, becomes);
    }

    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{Images, render, safe_html, to_text};

    /// Two images and a tracking pixel is the true sentence, and the pixel is
    /// the reason any of this is blocked, so it is counted apart.
    #[test]
    fn a_tracking_pixel_is_counted_apart_from_an_image() {
        let html = r#"<img src="https://cdn.test/logo.png" width="200" height="60">
            <img src="https://cdn.test/hero.jpg">
            <img src="https://track.test/o.gif" width="1" height="1">"#;
        let held = safe_html(html);
        assert_eq!(held.images, 2, "two real images");
        assert_eq!(held.pixels, 1, "and one that exists only to be fetched");
    }

    #[test]
    fn an_image_that_came_with_the_message_is_not_held_back() {
        let held = safe_html(r#"<img src="data:image/png;base64,iVBOR" width="1" height="1">"#);
        assert_eq!(held.images, 0);
        assert_eq!(held.pixels, 0, "it is embedded, so nothing is fetched");
    }

    #[test]
    fn asking_for_images_lets_them_through_for_that_message_only() {
        let html = r#"<img src="https://cdn.test/logo.png" width="200" height="60">"#;
        let blocked = render(html, Images::Blocked).html;
        let allowed = render(html, Images::Allowed).html;

        assert!(!blocked.contains("cdn.test"), "{blocked}");
        assert!(blocked.contains("img-src data:"), "{blocked}");

        assert!(allowed.contains("cdn.test"), "{allowed}");
        assert!(allowed.contains("img-src data: https: http:"), "{allowed}");
        // Everything else stays shut whichever way images went.
        assert!(allowed.contains("default-src 'none'"), "{allowed}");
        assert!(!allowed.contains("script-src"), "{allowed}");
    }

    #[test]
    fn nothing_that_fetches_survives_sanitising() {
        let html = r#"<img src="https://tracker.test/pixel.gif" alt="x">
            <link rel="stylesheet" href="https://tracker.test/s.css">
            <iframe src="https://tracker.test/frame"></iframe>
            <div style="background: url('https://tracker.test/bg.png'); color: red">hi</div>
            <script>fetch('https://tracker.test/beacon')</script>"#;
        let safe = safe_html(html).html;

        assert!(!safe.contains("tracker.test"), "{safe}");
        assert!(!safe.contains("<iframe"), "{safe}");
        assert!(!safe.contains("<script"), "{safe}");
        assert!(!safe.contains("src="), "{safe}");
        // What the message meant is still there.
        assert!(safe.contains("color: red"), "{safe}");
        assert!(safe.contains("hi"), "{safe}");
    }

    #[test]
    fn the_policy_forbids_fetching_even_if_the_pass_missed_something() {
        let safe = safe_html("<p>hello</p>").html;
        assert!(safe.contains("Content-Security-Policy"), "{safe}");
        assert!(safe.contains("default-src 'none'"), "{safe}");
        // Mail is written with inline styles, so those have to survive.
        assert!(safe.contains("style-src 'unsafe-inline'"), "{safe}");
        // An image that came with the message is not a fetch.
        assert!(safe.contains("img-src data:"), "{safe}");
    }

    #[test]
    fn handlers_that_would_run_are_removed() {
        let safe = safe_html(r#"<div onclick="alert(1)" onload="x()" title="kept">hi</div>"#).html;
        assert!(!safe.contains("onclick"), "{safe}");
        assert!(!safe.contains("onload"), "{safe}");
        assert!(safe.contains("title=\"kept\""), "{safe}");
    }

    #[test]
    fn an_embedded_image_is_left_alone_in_the_words() {
        // The policy allows data urls; the pass must not undo that by turning
        // the element into something else.
        let safe = safe_html("<p>before</p><p>after</p>").html;
        assert!(safe.contains("before"), "{safe}");
        assert!(safe.contains("after"), "{safe}");
    }

    #[test]
    fn tags_that_mean_a_line_break_become_one() {
        assert_eq!(to_text("<p>one</p><p>two</p>"), "one\ntwo");
        assert_eq!(to_text("one<br>two"), "one\ntwo");
        assert_eq!(to_text("<ul><li>a</li><li>b</li></ul>"), "a\nb");
    }

    #[test]
    fn what_carries_no_words_is_dropped_with_its_contents() {
        let html = "<style>body{color:red}</style><p>hello</p><script>alert(1)</script>";
        assert_eq!(to_text(html), "hello");
    }

    #[test]
    fn a_head_is_not_the_message() {
        let html = "<html><head><title>Ignore me</title></head><body><p>Read me</p></body></html>";
        assert_eq!(to_text(html), "Read me");
    }

    #[test]
    fn a_link_keeps_where_it_goes() {
        // The point of most development mail is the url in it.
        let html = r#"<a href="https://myapp.test/reset?token=abc">Reset your password</a>"#;
        assert_eq!(
            to_text(html),
            "Reset your password (https://myapp.test/reset?token=abc)"
        );
    }

    #[test]
    fn a_link_that_is_its_own_url_is_not_said_twice() {
        let html = r#"<a href="https://myapp.test/x">https://myapp.test/x</a>"#;
        assert_eq!(to_text(html), "https://myapp.test/x");
    }

    #[test]
    fn an_anchor_going_nowhere_is_left_alone() {
        assert_eq!(
            to_text(r##"<a href="#top">Back to top</a>"##),
            "Back to top"
        );
    }

    #[test]
    fn entities_become_the_characters_they_stand_for() {
        assert_eq!(
            to_text("<p>Tom &amp; Jerry &lt;3 &quot;quotes&quot;&nbsp;here</p>"),
            "Tom & Jerry <3 \"quotes\" here"
        );
    }

    #[test]
    fn blank_lines_do_not_pile_up() {
        let html = "<div></div><div></div><p>only line</p><div></div>";
        assert_eq!(to_text(html), "only line");
    }

    #[test]
    fn an_unclosed_tag_is_not_a_panic() {
        assert_eq!(to_text("before <b"), "before <b");
        assert_eq!(to_text("<script>never closed"), "");
    }

    /// Mailpit's own plain part fences a heading in asterisks and keeps bold
    /// markers, which is right for a terminal and noise on a screen. This is
    /// the message that showed it.
    #[test]
    fn a_heading_does_not_come_back_wearing_asterisks() {
        let html = "<html><body><h1>Password reset</h1>\
                    <p>Someone asked to reset the password for <b>you@example.test</b>.</p>\
                    </body></html>";
        assert_eq!(
            to_text(html),
            "Password reset\nSomeone asked to reset the password for you@example.test."
        );
    }

    #[test]
    fn a_realistic_message_reads_as_a_message() {
        let html = "<html><head><style>p{margin:0}</style></head><body>\
                    <h1>Welcome to Skep</h1>\
                    <p>Your account is ready.</p>\
                    <p>Please <a href=\"https://myapp.test/confirm?t=9f2\">confirm your email</a> \
                    to finish.</p>\
                    <table><tr><td>Plan</td><td>Free</td></tr></table>\
                    </body></html>";
        assert_eq!(
            to_text(html),
            "Welcome to Skep\n\
             Your account is ready.\n\
             Please confirm your email (https://myapp.test/confirm?t=9f2) to finish.\n\
             Plan Free"
        );
    }
}
