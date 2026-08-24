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
            ports: status.ports.clone(),
            pid: status.pid,
            action: None,
        }
    }
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
        Ok(Response::Status { services }) => services,
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
            let id = match comb_services::instance(name, wanted.version.as_deref()) {
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
