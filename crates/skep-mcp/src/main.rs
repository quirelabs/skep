//! The MCP server. A pure client of the engine, like the CLI: nothing here
//! hosts services, and nothing here crashes because none are running.

mod view;

use comb::{Client, Paths, Request, Response};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::transport::stdio;
use rmcp::{ErrorData, ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};

/// The macro keeps the router in a static, so the server itself is stateless
/// and every call connects afresh.
#[derive(Clone)]
struct Skep;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct Service {
    /// Service name, for example "postgres". May carry a version: "postgres@16".
    service: String,
    /// Major like "17" or an exact version. Defaults to the newest pinned one.
    version: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct Mail {
    /// Words to look for in senders, subjects and bodies. Omit to list the
    /// most recent messages.
    query: Option<String>,
    /// The id of one message, to read it in full instead of listing.
    id: Option<String>,
    /// How many to return. Defaults to 20.
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct Named {
    /// Service name, optionally with a version.
    service: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct Tail {
    /// Service name, optionally with a version.
    service: String,
    /// How many of the most recent lines to return. Defaults to 50.
    lines: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct Keep {
    /// Service name, optionally with a version or a branch label.
    service: String,
    /// What to call the copy. Lowercase letters, digits, - and _.
    name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct Sprout {
    /// Service to branch from, optionally with a version.
    service: String,
    /// What to call the branch. Lowercase letters, digits, - and _.
    label: String,
    /// Branch from this snapshot instead of from the service as it stands.
    from: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct Project {
    /// Directory to search from. Defaults to the working directory. skep.toml
    /// is looked for here and in every parent.
    path: Option<String>,
    /// Start anything the file asks for that is not already running.
    start: Option<bool>,
}

#[tool_router]
impl Skep {
    fn new() -> Self {
        Self
    }

    #[tool(
        name = "skep_status",
        description = "Every local dev service skep knows about, and exactly what it is doing. \
                       Returns {\"services\":[{\"id\":\"postgres@17.6.0\",\"state\":\"ready\",\
                       \"ports\":{\"postgres\":5432},\"pid\":123}]}. state is one of stopped, \
                       starting, ready, stopping, failed, restarting. A failed service carries \
                       \"reason\"; one that is still coming up carries \"doing\" with the phase \
                       it is in, such as a download or a database being initialised. The id \
                       always names the exact version that is running. This is the one call \
                       needed to know what is wrong."
    )]
    async fn status(&self) -> Result<CallToolResult, ErrorData> {
        let mut client = match connect().await {
            Ok(client) => client,
            Err(problem) => return Ok(*problem),
        };
        match ask(&mut client, Request::Status).await {
            Ok(Response::Status { overview }) => json(&view::Report::of(&overview.services)),
            Ok(other) => Ok(confused(other)),
            Err(problem) => Ok(*problem),
        }
    }

    #[tool(
        name = "skep_start",
        description = "Start a service, installing the pinned release first if needed. Returns \
                       the service's status in the same shape as skep_status. Fails with a \
                       sentence naming the cause, such as a port already held by another \
                       process and the command that frees it."
    )]
    async fn start(
        &self,
        Parameters(Service { service, version }): Parameters<Service>,
    ) -> Result<CallToolResult, ErrorData> {
        let spec =
            match comb_services::spec_default(&service, version.as_deref(), &Paths::from_env()) {
                Ok(spec) => spec,
                Err(error) => return Ok(sentence(error.to_string())),
            };
        let instance = spec.id.clone();

        let mut client = match connect().await {
            Ok(client) => client,
            Err(problem) => return Ok(*problem),
        };
        for request in [
            Request::Register {
                spec: Box::new(spec),
            },
            Request::Start {
                instance: instance.clone(),
            },
        ] {
            match ask(&mut client, request).await {
                Ok(Response::Done) => {}
                Ok(other) => return Ok(confused(other)),
                Err(problem) => return Ok(*problem),
            }
        }
        self.report_on(&mut client, &instance.to_string()).await
    }

    #[tool(
        name = "skep_mail",
        description = "What the local mail catcher has caught: the mail an application sent \
                       while it was running. With no arguments, the most recent messages as \
                       {\"unread\":1,\"messages\":[{\"id\":\"abc\",\"from\":\"hello@myapp.test\",\
                       \"subject\":\"Reset your password\",\"snippet\":\"Click here...\",\
                       \"at\":\"2026-08-28T10:34:17-04:00\",\"read\":false}]}. With query, only \
                       messages matching it. With id, that one message in full, including its \
                       body as text, which is where a confirmation link or a code will be. \
                       This is the one call needed to answer whether something was sent and \
                       what it said. Needs mailpit running."
    )]
    async fn mail(
        &self,
        Parameters(Mail { query, id, limit }): Parameters<Mail>,
    ) -> Result<CallToolResult, ErrorData> {
        let port = match mail_port().await {
            Ok(port) => port,
            Err(problem) => return Ok(*problem),
        };
        let limit = limit.unwrap_or(20);

        if let Some(id) = id {
            return match comb_services::mail::read(port, &id).await {
                Ok(body) => json(&body),
                Err(error) => Ok(sentence(error.to_string())),
            };
        }

        let caught = match query {
            Some(query) => comb_services::mail::search(port, &query, limit)
                .await
                .map(|messages| (messages, 0)),
            None => comb_services::mail::inbox(port, limit).await,
        };
        match caught {
            Ok((messages, unread)) => json(&view::Mail { unread, messages }),
            Err(error) => Ok(sentence(error.to_string())),
        }
    }

    #[tool(
        name = "skep_sites",
        description = "Every hostname this machine serves over https, and the port each one \
                       points at. Returns {\"sites\":[{\"url\":\"https://myapp.test:8443\",\
                       \"host\":\"myapp.test\",\"port\":3000}]}. Skep does not run the app \
                       behind a site, it puts a name and a certificate browsers trust in \
                       front of a port something is already listening on. An empty list \
                       means none are configured, which is not an error."
    )]
    async fn sites(&self) -> Result<CallToolResult, ErrorData> {
        let mut client = match connect().await {
            Ok(client) => client,
            Err(problem) => return Ok(*problem),
        };
        match ask(&mut client, Request::Sites).await {
            Ok(Response::Sites { sites }) => json(&view::Sites::of(&sites)),
            Ok(other) => Ok(confused(other)),
            Err(problem) => Ok(*problem),
        }
    }

    #[tool(
        name = "skep_stop",
        description = "Stop a running service. Returns its status afterwards."
    )]
    async fn stop(
        &self,
        Parameters(Named { service }): Parameters<Named>,
    ) -> Result<CallToolResult, ErrorData> {
        self.lifecycle(&service, |instance| Request::Stop { instance })
            .await
    }

    #[tool(
        name = "skep_restart",
        description = "Restart a service, stopping it first if it is running. Returns its \
                       status afterwards."
    )]
    async fn restart(
        &self,
        Parameters(Named { service }): Parameters<Named>,
    ) -> Result<CallToolResult, ErrorData> {
        self.lifecycle(&service, |instance| Request::Restart { instance })
            .await
    }

    #[tool(
        name = "skep_logs",
        description = "The most recent output of a service, oldest first. Returns \
                       {\"id\":\"...\",\"lines\":[\"...\"]}. This is a bounded tail of what the \
                       service has written since it started, not a live stream."
    )]
    async fn logs(
        &self,
        Parameters(Tail { service, lines }): Parameters<Tail>,
    ) -> Result<CallToolResult, ErrorData> {
        let instance = match comb_services::instance(&service, None) {
            Ok(instance) => instance,
            Err(error) => return Ok(sentence(error.to_string())),
        };
        let mut client = match connect().await {
            Ok(client) => client,
            Err(problem) => return Ok(*problem),
        };
        let request = Request::Logs {
            instance: instance.clone(),
            lines: lines.unwrap_or(50),
        };
        match ask(&mut client, request).await {
            Ok(Response::Logs { lines }) => json(&view::Logs {
                id: instance.to_string(),
                lines: lines.into_iter().map(|line| line.text).collect(),
            }),
            Ok(other) => Ok(confused(other)),
            Err(problem) => Ok(*problem),
        }
    }

    #[tool(
        name = "skep_snapshot",
        description = "Keep a copy of a service's data under a name. The service is stopped for \
                       the copy and started again afterwards, which is what makes the copy \
                       consistent, so expect a pause rather than an instant answer. Returns the \
                       service's status afterwards, in the same shape as skep_status."
    )]
    async fn snapshot(
        &self,
        Parameters(Keep { service, name }): Parameters<Keep>,
    ) -> Result<CallToolResult, ErrorData> {
        let instance = match comb_services::instance(&service, None) {
            Ok(instance) => instance,
            Err(error) => return Ok(sentence(error.to_string())),
        };
        let mut client = match connect().await {
            Ok(client) => client,
            Err(problem) => return Ok(*problem),
        };
        let request = Request::Snapshot {
            instance: instance.clone(),
            name,
        };
        match ask(&mut client, request).await {
            Ok(Response::Done) => self.report_on(&mut client, &instance.to_string()).await,
            Ok(other) => Ok(confused(other)),
            Err(problem) => Ok(*problem),
        }
    }

    #[tool(
        name = "skep_snapshots",
        description = "The copies kept for a service, newest name last. Returns \
                       {\"snapshots\":[{\"name\":\"...\",\"taken\":1700000000000}]}. Use a name \
                       from here as skep_branch's from argument."
    )]
    async fn snapshots(
        &self,
        Parameters(Named { service }): Parameters<Named>,
    ) -> Result<CallToolResult, ErrorData> {
        let instance = match comb_services::instance(&service, None) {
            Ok(instance) => instance,
            Err(error) => return Ok(sentence(error.to_string())),
        };
        let mut client = match connect().await {
            Ok(client) => client,
            Err(problem) => return Ok(*problem),
        };
        match ask(&mut client, Request::Snapshots { instance }).await {
            Ok(Response::Snapshots { snapshots }) => json(&view::Kept { snapshots }),
            Ok(other) => Ok(confused(other)),
            Err(problem) => Ok(*problem),
        }
    }

    #[tool(
        name = "skep_branch",
        description = "Run a second copy of a service on its own data and its own port, started \
                       and ready to connect to. Branch from a snapshot with from, or omit it to \
                       branch from the service as it stands, which stops it briefly to copy. \
                       A branch is a sibling, not a child: it belongs to the service and \
                       version, so branching a branch gives another sibling rather than a \
                       nested one. Returns the branch's status, including the port it took."
    )]
    async fn branch(
        &self,
        Parameters(Sprout {
            service,
            label,
            from,
        }): Parameters<Sprout>,
    ) -> Result<CallToolResult, ErrorData> {
        let (parent, label) = match (
            comb_services::instance(&service, None),
            comb::Label::new(label),
        ) {
            (Ok(parent), Ok(label)) => (parent, label),
            (Err(error), _) | (_, Err(error)) => return Ok(sentence(error.to_string())),
        };
        let spec = match comb_services::branch_spec(&parent, &label, &Paths::from_env()) {
            Ok(spec) => spec,
            Err(error) => return Ok(sentence(error.to_string())),
        };
        let instance = spec.id.clone();

        let mut client = match connect().await {
            Ok(client) => client,
            Err(problem) => return Ok(*problem),
        };
        for request in [
            Request::Branch {
                from: parent,
                spec: Box::new(spec),
                snapshot: from,
            },
            Request::Start {
                instance: instance.clone(),
            },
        ] {
            match ask(&mut client, request).await {
                Ok(Response::Done) => {}
                Ok(other) => return Ok(confused(other)),
                Err(problem) => return Ok(*problem),
            }
        }
        self.report_on(&mut client, &instance.to_string()).await
    }

    #[tool(
        name = "skep_delete_branch",
        description = "Remove a branch and the data it was using. The branch must be stopped \
                       first, and the error says so if it is not. Name it the way it prints: \
                       postgres:experiment."
    )]
    async fn delete_branch(
        &self,
        Parameters(Named { service }): Parameters<Named>,
    ) -> Result<CallToolResult, ErrorData> {
        let instance = match comb_services::instance(&service, None) {
            Ok(instance) => instance,
            Err(error) => return Ok(sentence(error.to_string())),
        };
        let mut client = match connect().await {
            Ok(client) => client,
            Err(problem) => return Ok(*problem),
        };
        match ask(&mut client, Request::RemoveBranch { instance }).await {
            Ok(Response::Done) => json(&view::Gone { deleted: true }),
            Ok(other) => Ok(confused(other)),
            Err(problem) => Ok(*problem),
        }
    }

    #[tool(
        name = "skep_project",
        description = "Read a repository's skep.toml and report what it needs against what is \
                       running. Returns {\"file\":\"...\",\"services\":[{\"id\":\"...\",\
                       \"state\":\"...\"}]}, with \"action\" per service when start is true. \
                       Set start to boot whatever is missing; one service failing never stops \
                       the others."
    )]
    async fn project(
        &self,
        Parameters(Project { path, start }): Parameters<Project>,
    ) -> Result<CallToolResult, ErrorData> {
        let from = match path {
            Some(path) => std::path::PathBuf::from(path),
            None => match std::env::current_dir() {
                Ok(directory) => directory,
                Err(error) => return Ok(sentence(error.to_string())),
            },
        };
        let Some(file) = comb_services::project::find(&from) else {
            return Ok(sentence(format!(
                "no skep.toml here or in any parent of {}",
                from.display()
            )));
        };
        let project = match comb_services::project::load(&file) {
            Ok(project) => project,
            Err(error) => return Ok(sentence(error.to_string())),
        };

        let mut client = match connect().await {
            Ok(client) => client,
            Err(problem) => return Ok(*problem),
        };
        let running = match ask(&mut client, Request::Status).await {
            Ok(Response::Status { overview }) => overview.services,
            Ok(other) => return Ok(confused(other)),
            Err(problem) => return Ok(*problem),
        };

        let report = view::project(
            &file,
            &project,
            &running,
            start.unwrap_or(false),
            &mut client,
        )
        .await;
        json(&report)
    }

    async fn lifecycle(
        &self,
        service: &str,
        request: impl FnOnce(comb::InstanceId) -> Request,
    ) -> Result<CallToolResult, ErrorData> {
        let instance = match comb_services::instance(service, None) {
            Ok(instance) => instance,
            Err(error) => return Ok(sentence(error.to_string())),
        };
        let mut client = match connect().await {
            Ok(client) => client,
            Err(problem) => return Ok(*problem),
        };
        match ask(&mut client, request(instance.clone())).await {
            Ok(Response::Done) => self.report_on(&mut client, &instance.to_string()).await,
            Ok(other) => Ok(confused(other)),
            Err(problem) => Ok(*problem),
        }
    }

    /// Every lifecycle call answers with the resulting state, so an agent never
    /// has to follow up with skep_status to learn what happened.
    async fn report_on(&self, client: &mut Client, id: &str) -> Result<CallToolResult, ErrorData> {
        match ask(client, Request::Status).await {
            Ok(Response::Status { overview }) => {
                match overview
                    .services
                    .iter()
                    .find(|status| status.id.to_string() == id)
                {
                    Some(status) => json(&view::Service::of(status)),
                    None => Ok(sentence(format!("{id} is not registered"))),
                }
            }
            Ok(other) => Ok(confused(other)),
            Err(problem) => Ok(*problem),
        }
    }
}

#[tool_handler]
impl ServerHandler for Skep {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::LATEST)
            .with_server_info(Implementation::new("skep", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "skep manages local development services: databases, caches and a mail \
                 catcher, running natively. Call skep_status once to learn the state of \
                 everything, including why anything failed. Errors are written to be relayed \
                 to the user as they are: they name the cause and the command that fixes it.",
            )
    }
}

/// Connects, or hands back the same sentence a person would see. Nothing here
/// panics: an agent calling with no engine running is ordinary, not an error
/// worth taking the server down for.
async fn connect() -> Result<Client, Box<CallToolResult>> {
    Client::connect(&Paths::from_env())
        .await
        .map_err(|error| Box::new(sentence(error.to_string())))
}

async fn ask(client: &mut Client, request: Request) -> Result<Response, Box<CallToolResult>> {
    match client.send(&request).await {
        Ok(Response::Failed { message }) => Err(Box::new(sentence(message))),
        Ok(response) => Ok(response),
        Err(error) => Err(Box::new(sentence(error.to_string()))),
    }
}

/// Which port the mail catcher is on, asked of the engine rather than assumed,
/// because a project is free to move it.
async fn mail_port() -> Result<u16, Box<CallToolResult>> {
    let mut client = connect().await?;
    let services = match ask(&mut client, Request::Status).await {
        Ok(Response::Status { overview }) => overview.services,
        Ok(other) => return Err(Box::new(confused(other))),
        Err(problem) => return Err(problem),
    };

    let Some(mailpit) = services
        .iter()
        .find(|service| service.id.service.as_str() == "mailpit")
    else {
        return Err(Box::new(sentence(
            "skep does not have mailpit. Start it with skep_start.",
        )));
    };
    if mailpit.state != comb::ServiceState::Ready {
        return Err(Box::new(sentence(format!(
            "mailpit is {}, so there is nothing catching mail. Start it with skep_start.",
            mailpit.state
        ))));
    }
    mailpit
        .ports
        .get("http")
        .copied()
        .ok_or_else(|| Box::new(sentence("mailpit is running but has no http port")))
}

fn sentence(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

fn confused(response: Response) -> CallToolResult {
    sentence(format!(
        "the engine answered something unexpected: {response:?}"
    ))
}

fn json(value: &impl Serialize) -> Result<CallToolResult, ErrorData> {
    serde_json::to_string(value)
        .map(|text| CallToolResult::success(vec![ContentBlock::text(text)]))
        .map_err(|error| ErrorData::internal_error(error.to_string(), None))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let running = Skep::new().serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}
