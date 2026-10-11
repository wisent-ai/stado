//! What GitHub says about one runner registration at one scope, read with
//! the lifecycle's GitHub identity.

use serde_json::Value;

use crate::deploy::host_precheck_runner::verdict::scope::RunnerScope;
use crate::deploy::DeployError;

use super::github::{github_credential, github_json};

/// What GitHub says about one runner name at one scope.
///
/// `Unreadable` is its own answer on purpose. The fleet's credential is a
/// repository administrator, not an organization administrator, so the
/// organization list answers 403 while every repository list it owns answers
/// 200. A refused read is a fact about the credential; it is not evidence
/// that a runner is missing, and reporting it as missing is how five separate
/// diagnoses concluded that runners could not be managed from here.
#[derive(Debug, Clone)]
pub(crate) enum RunnerRecord {
    Present { status: String },
    Absent { listed: Vec<String> },
    Unreadable { detail: String },
}

pub(crate) async fn github_runner(scope: &RunnerScope, runner_name: &str) -> RunnerRecord {
    let credential = match github_credential().await {
        Ok(credential) => credential,
        Err(DeployError {
            message: detail, ..
        }) => return RunnerRecord::Unreadable { detail },
    };
    let mut listed = Vec::new();
    let mut page = 1;
    loop {
        let endpoint = format!("{}&page={page}", scope.runners_endpoint());
        let document = match github_json(reqwest::Method::GET, &endpoint, &credential, None).await {
            Ok(document) => document,
            Err(DeployError {
                message: detail, ..
            }) => return RunnerRecord::Unreadable { detail },
        };
        let Some(runners) = document.get("runners").and_then(Value::as_array) else {
            return RunnerRecord::Unreadable {
                detail: format!("{endpoint} answered no runners array"),
            };
        };
        if let Some(runner) = runners
            .iter()
            .find(|runner| runner.get("name").and_then(Value::as_str) == Some(runner_name))
        {
            return match runner.get("status").and_then(Value::as_str) {
                Some(status @ ("online" | "offline")) => RunnerRecord::Present {
                    status: status.to_string(),
                },
                status => RunnerRecord::Unreadable {
                    detail: format!("GitHub runner {runner_name} has invalid status {status:?}"),
                },
            };
        }
        listed.extend(runners.iter().filter_map(|runner| {
            runner
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        }));
        if runners.len() < 100 {
            return RunnerRecord::Absent { listed };
        }
        page += 1;
    }
}

/// Whether GitHub reports this registration online, asked at the scope that
/// made it. An unreadable list is not an offline runner: cycling a service on
/// a refused read is a production action taken on no evidence.
pub(crate) async fn github_runner_is_online(
    scope: &RunnerScope,
    runner_name: &str,
) -> Result<bool, DeployError> {
    match github_runner(scope, runner_name).await {
        RunnerRecord::Present { status } if status == "online" => Ok(true),
        RunnerRecord::Present { status } if status == "offline" => Ok(false),
        RunnerRecord::Present { status } => Err(DeployError::unreachable(format!(
            "GitHub runner {runner_name} has unknown status {status:?}"
        ))),
        RunnerRecord::Absent { listed } => Err(DeployError(format!(
            "GitHub has no runner named {runner_name} under {}; it lists {}",
            scope.label(),
            if listed.is_empty() {
                "none".to_string()
            } else {
                listed.join(", ")
            }
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound)),
        RunnerRecord::Unreadable { detail } => Err(DeployError::unreachable(format!(
            "GitHub's runner list for {} could not be read, so this runner's state is unknown: {detail}",
            scope.label()
        ))),
    }
}
