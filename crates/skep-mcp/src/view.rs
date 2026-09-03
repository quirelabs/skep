//! Compact projections of engine state. Agents pay for every token, so this
//! carries what is needed to act and nothing else.

use std::collections::BTreeMap;
use std::path::Path;

use comb::{Client, Paths, Request, Response, ServiceState, ServiceStatus};
use serde::Serialize;

#[derive(Serialize)]
pub struct Report {
    pub services: Vec<Service>,
}

impl Report {
    pub fn of(statuses: &[ServiceStatus]) -> Self {
        Self {
            services: statuses.iter().map(Service::of).collect(),
        }
    }
}

#[derive(Serialize)]
pub struct Service {
    /// Carries the exact version that is running.
    pub id: String,
    pub state: &'static str,
    /// Why it failed, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The phase of a long start, such as a download or an initialisation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doing: Option<String>,
    /// Set when something outside skep holds this service's port, so it cannot
    /// start until that is dealt with. Carries the remedy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
    /// What the service announced once up, such as the public url a tunnel
    /// was given. Gone the moment the service is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub ports: BTreeMap<String, u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

impl Service {
    pub fn of(status: &ServiceStatus) -> Self {
        Self {
            id: status.id.to_string(),
            state: status.state.name(),
            reason: match &status.state {
                ServiceState::Failed { reason } => Some(reason.clone()),
                _ => None,
            },
            doing: status.activity.clone(),
            blocked: status.blocked.clone(),
            notice: status.notice.clone(),
            ports: status.ports.clone(),
            pid: status.pid,
            action: None,
        }
    }
}

#[derive(Serialize)]
pub struct Kept {
    pub snapshots: Vec<comb::Snapshot>,
}

#[derive(Serialize)]
pub struct Gone {
    pub deleted: bool,
}

#[derive(Serialize)]
pub struct Logs {
    pub id: String,
    pub lines: Vec<String>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum Entry {
    Known(Box<Service>),
    /// A line in the file we could not even resolve to a service.
    Problem {
        service: String,
        error: String,
    },
}

#[derive(Serialize)]
pub struct ProjectReport {
    pub file: String,
    pub services: Vec<Entry>,
}

/// What a project asks for, against what is running. One service failing never
/// stops the others, so the report is always complete.
pub async fn project(
    file: &Path,
    project: &comb_services::project::Project,
    running: &[ServiceStatus],
    start: bool,
    client: &mut Client,
) -> ProjectReport {
    let paths = Paths::from_env();
    let mut actions: BTreeMap<String, String> = BTreeMap::new();
    let mut problems: BTreeMap<String, String> = BTreeMap::new();

    if start {
        for (name, wanted) in &project.services {
            match comb_services::bring_up(client, name, wanted, running, &paths).await {
                Ok((id, outcome)) => {
                    actions.insert(id.to_string(), outcome.to_string());
                }
                Err(sentence) => {
                    problems.insert(name.clone(), sentence);
                }
            }
        }
    }

    // Read the state after acting, so the report describes the world as it is
    // now rather than as it was before the call.
    let latest = match client.send(&Request::Status).await {
        Ok(Response::Status { overview }) => overview.services,
        _ => running.to_vec(),
    };

    let services = project
        .services
        .iter()
        .map(|(name, wanted)| {
            if let Some(error) = problems.remove(name) {
                return Entry::Problem {
                    service: name.clone(),
                    error,
                };
            }
            let id = match comb_services::instance(name, wanted.version.as_deref(), &paths) {
                Ok(id) => id.to_string(),
                Err(error) => {
                    return Entry::Problem {
                        service: name.clone(),
                        error: error.to_string(),
                    };
                }
            };
            let mut entry = match latest.iter().find(|status| status.id.to_string() == id) {
                Some(status) => Service::of(status),
                None => Service {
                    id: id.clone(),
                    state: "stopped",
                    reason: None,
                    doing: None,
                    blocked: None,
                    notice: None,
                    ports: BTreeMap::new(),
                    pid: None,
                    action: None,
                },
            };
            entry.action = actions.remove(&id);
            Entry::Known(Box::new(entry))
        })
        .collect();

    ProjectReport {
        file: file.display().to_string(),
        services,
    }
}

/// Sites as an agent wants them: the url to actually use, not just the parts.
#[derive(Debug, Serialize)]
pub struct Sites {
    pub sites: Vec<Site>,
}

#[derive(Debug, Serialize)]
pub struct Site {
    pub url: String,
    pub host: String,
    pub port: u16,
}

impl Sites {
    pub fn of(sites: &std::collections::BTreeMap<String, u16>) -> Self {
        Self {
            sites: sites
                .iter()
                .map(|(host, port)| Site {
                    url: format!("https://{host}:{}", comb::HTTPS_PORT),
                    host: host.clone(),
                    port: *port,
                })
                .collect(),
        }
    }
}

/// What the mail catcher caught, in the shape an agent asked the question in.
#[derive(Debug, Serialize)]
pub struct Mail {
    pub unread: usize,
    pub messages: Vec<comb_services::mail::Summary>,
}

/// What sharing a target came back with.
#[derive(Serialize)]
pub struct Shared {
    pub url: String,
    pub instance: String,
}
