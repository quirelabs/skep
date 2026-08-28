//! Reading what the mail catcher caught, without leaving the terminal.

use anyhow::{Result, bail};
use comb::{Client, Paths, Request, Response, ServiceState};

pub async fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("clear") => clear().await,
        Some("read") => match args.get(1) {
            Some(id) => read(id).await,
            None => bail!("skep mail read <id>"),
        },
        Some(query) => list(Some(query)).await,
        None => list(None).await,
    }
}

async fn list(query: Option<&str>) -> Result<()> {
    let port = port().await?;
    let (messages, unread) = match query {
        Some(query) => (
            comb_services::mail::search(port, query, 50).await?,
            usize::MAX,
        ),
        None => comb_services::mail::inbox(port, 50).await?,
    };

    if messages.is_empty() {
        match query {
            Some(query) => println!("nothing matching {query}"),
            None => println!("no mail caught yet"),
        }
        return Ok(());
    }

    for message in &messages {
        // Unread is marked in the margin, so the eye can find it without
        // reading a column that says so.
        let mark = if message.read { " " } else { "*" };
        println!(
            "{mark} {:<24} {}",
            shorten(&message.from, 24),
            message.subject
        );
        println!("    {}  {}", message.id, shorten(&message.snippet, 60));
    }
    if unread != usize::MAX && unread > 0 {
        println!("\n{unread} unread of {}", messages.len());
    }
    println!("\nread one with: skep mail read <id>");
    Ok(())
}

async fn read(id: &str) -> Result<()> {
    let body = comb_services::mail::read(port().await?, id).await?;
    println!("from     {}", body.from);
    println!("to       {}", body.to.join(", "));
    println!("subject  {}", body.subject);
    println!("at       {}", body.at);
    if !body.attachments.is_empty() {
        println!("files    {}", body.attachments.join(", "));
    }
    if body.converted {
        println!("note     this message was html, shown here as text");
    }
    println!();
    println!("{}", body.text);
    Ok(())
}

async fn clear() -> Result<()> {
    comb_services::mail::clear(port().await?).await?;
    println!("mail cleared");
    Ok(())
}

fn shorten(text: &str, width: usize) -> String {
    let text = text.replace(['\n', '\r'], " ");
    if text.chars().count() <= width {
        return text;
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

/// Asked of the engine rather than assumed, because a project is free to move
/// the port.
async fn port() -> Result<u16> {
    let mut client = match Client::connect(&Paths::from_env()).await {
        Ok(client) => client,
        Err(comb::Error::NoHost) => {
            bail!("no skep engine is running\n  start one with: skep serve")
        }
        Err(other) => return Err(other.into()),
    };

    let Response::Status { overview } = client.send(&Request::Status).await? else {
        bail!("unexpected reply");
    };
    let Some(mailpit) = overview
        .services
        .iter()
        .find(|service| service.id.service.as_str() == "mailpit")
    else {
        bail!("skep does not have mailpit");
    };
    if mailpit.state != ServiceState::Ready {
        bail!(
            "mailpit is {}, so nothing is catching mail\n  start it with: skep start mailpit",
            mailpit.state
        );
    }
    mailpit
        .ports
        .get("http")
        .copied()
        .ok_or_else(|| anyhow::anyhow!("mailpit is running but has no http port"))
}
