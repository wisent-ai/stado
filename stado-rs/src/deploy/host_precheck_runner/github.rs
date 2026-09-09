//! The GitHub identity this lifecycle uses, and the API calls it makes with it.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

use super::scope::RunnerScope;
use crate::deploy::DeployError;

pub const GITHUB_ORGANIZATION: &str = "wisent-ai";

pub(crate) async fn github_credential() -> Result<String, DeployError> {
    crate::github_identity::credential()
        .await
        .map_err(DeployError)
}

pub(crate) async fn github_runner_token(
    scope: &RunnerScope,
    kind: &str,
) -> Result<String, DeployError> {
    let resolved = crate::github_identity::resolve()
        .await
        .map_err(DeployError)?;
    let credential = crate::github_identity::read(&resolved)
        .await
        .map_err(DeployError)?;
    let response = reqwest::Client::new()
        .post(scope.token_endpoint(kind))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header(reqwest::header::USER_AGENT, "wisent-stado-precheck-runner")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .bearer_auth(&credential)
        .send()
        .await
        .map_err(|error| DeployError(format!("GitHub runner token request failed: {error}")))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| DeployError(format!("GitHub runner token response failed: {error}")))?;
    if !status.is_success() {
        let detail = String::from_utf8_lossy(&bytes).replace(&credential, "[REDACTED]");
        // Which credential was used, what it must be allowed to do, and the
        // other door. A bare 403 sends the reader to GitHub's documentation
        // and has cost this fleet five separate re-diagnoses.
        let remedy = if status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::UNAUTHORIZED
        {
            let coordinate = format!("{}.{}", resolved.item, resolved.field);
            match scope {
                RunnerScope::Organization => format!(
                    ". Skarbiec route {:?} resolved to {coordinate}. GitHub refused organization \
                     runner administration. Repository registration is a separate operation: \
                     use --repository <NAME> when a runner for one repository is intended",
                    resolved.route
                ),
                RunnerScope::Repository(repository) => format!(
                    ". Skarbiec route {:?} resolved to {coordinate}. GitHub refused runner \
                     administration for {GITHUB_ORGANIZATION}/{repository}; the response above \
                     is the permission verdict for that repository",
                    resolved.route
                ),
            }
        } else {
            String::new()
        };
        return Err(DeployError(format!(
            "GitHub runner {kind} token request for {} returned HTTP {}: {}{remedy}",
            scope.label(),
            status.as_u16(),
            detail.trim()
        )));
    }
    serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| DeployError(format!("GitHub runner token response is invalid: {error}")))?
        .get("token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| DeployError("GitHub runner token response has no token".to_string()))
}

pub(crate) async fn github_json(
    method: reqwest::Method,
    endpoint: &str,
    credential: &str,
    body: Option<&Value>,
) -> Result<Value, DeployError> {
    let mut request = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| DeployError(format!("GitHub client could not start: {error}")))?
        .request(method, endpoint)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header(reqwest::header::USER_AGENT, "wisent-stado-precheck-runner")
        .bearer_auth(credential);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| DeployError(format!("GitHub request failed for {endpoint}: {error}")))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| DeployError(format!("GitHub response failed for {endpoint}: {error}")))?;
    if !status.is_success() {
        let detail = String::from_utf8_lossy(&bytes).replace(credential, "[REDACTED]");
        return Err(DeployError(format!(
            "GitHub request to {endpoint} returned HTTP {}: {}",
            status.as_u16(),
            detail.trim()
        )));
    }
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        DeployError(format!(
            "GitHub response from {endpoint} is invalid: {error}"
        ))
    })
}

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
        Err(DeployError(detail)) => return RunnerRecord::Unreadable { detail },
    };
    let mut listed = Vec::new();
    let mut page = 1;
    loop {
        let endpoint = format!("{}&page={page}", scope.runners_endpoint());
        let document = match github_json(reqwest::Method::GET, &endpoint, &credential, None).await {
            Ok(document) => document,
            Err(DeployError(detail)) => return RunnerRecord::Unreadable { detail },
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
        RunnerRecord::Present { status } => Err(DeployError(format!(
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
        ))),
        RunnerRecord::Unreadable { detail } => Err(DeployError(format!(
            "GitHub's runner list for {} could not be read, so this runner's state is unknown: {detail}",
            scope.label()
        ))),
    }
}

pub(crate) fn repository_name(repository: &str) -> Result<&str, DeployError> {
    let repository = repository.trim();
    if repository.is_empty()
        || repository.contains('/')
        || !repository
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(DeployError(
            "repository must be one name inside wisent-ai".to_string(),
        ));
    }
    Ok(repository)
}

pub(crate) fn set_repository_secret(
    repository: &str,
    name: &str,
    value: &str,
    github_token: &str,
) -> Result<(), DeployError> {
    let mut child = Command::new("gh")
        .arg("secret")
        .arg("set")
        .arg(name)
        .arg("--repo")
        .arg(format!("{GITHUB_ORGANIZATION}/{repository}"))
        .env("GH_TOKEN", github_token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| DeployError(format!("could not start gh secret set: {error}")))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| DeployError("gh secret set stdin is unavailable".to_string()))?
        .write_all(value.as_bytes())
        .map_err(|error| DeployError(format!("could not write gh secret set stdin: {error}")))?;
    let output = child
        .wait_with_output()
        .map_err(|error| DeployError(format!("gh secret set failed: {error}")))?;
    if !output.status.success() {
        return Err(DeployError(format!(
            "GitHub repository secret {name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .replace(github_token, "[REDACTED]")
                .trim()
        )));
    }
    Ok(())
}
