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
    use super::to_text;

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
