//! The GitHub identity this lifecycle uses, and the API calls it makes with it.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

use super::credentials::admin_credential;
use super::scope::RunnerScope;
use crate::deploy::DeployError;

pub const GITHUB_ORGANIZATION: &str = "wisent-ai";
pub const GITHUB_CREDENTIAL_ITEM: &str = "GITHUB_TOKEN";

pub(crate) async fn github_credential() -> Result<String, DeployError> {
    admin_credential(GITHUB_CREDENTIAL_ITEM, "value").await
}

pub(crate) async fn github_runner_token(
    scope: &RunnerScope,
    kind: &str,
) -> Result<String, DeployError> {
    let credential = github_credential().await?;
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
            match scope {
                RunnerScope::Organization => format!(
                    ". Stado read this credential from Skarbiec item {GITHUB_CREDENTIAL_ITEM:?} \
                     field \"value\"; that identity may not manage {GITHUB_ORGANIZATION} runners, \
                     which is what an organization-wide runner needs. Either store a credential \
                     with the organization's self-hosted-runner write permission in that item, or \
                     register this host against one repository with --repository <NAME>, which \
                     the same credential is allowed to do"
                ),
                RunnerScope::Repository(repository) => format!(
                    ". Stado read this credential from Skarbiec item {GITHUB_CREDENTIAL_ITEM:?} \
                     field \"value\"; that identity is not an administrator of \
                     {GITHUB_ORGANIZATION}/{repository}, so it cannot register a runner there"
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
    let mut request = reqwest::Client::new()
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

pub(crate) async fn github_runner_is_online(runner_name: &str) -> Result<bool, DeployError> {
    let credential = github_credential().await?;
    let endpoint =
        format!("https://api.github.com/orgs/{GITHUB_ORGANIZATION}/actions/runners?per_page=100");
    let document = github_json(reqwest::Method::GET, &endpoint, &credential, None).await?;
    let runners = document
        .get("runners")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("GitHub runner response has no runners array".to_string()))?;
    let runner = runners
        .iter()
        .find(|runner| runner.get("name").and_then(Value::as_str) == Some(runner_name))
        .ok_or_else(|| {
            DeployError(format!(
                "GitHub has no registered runner named {runner_name}; refusing to cycle a locally registered runner"
            ))
        })?;
    match runner.get("status").and_then(Value::as_str) {
        Some("online") => Ok(true),
        Some("offline") => Ok(false),
        Some(status) => Err(DeployError(format!(
            "GitHub runner {runner_name} has unknown status {status:?}"
        ))),
        None => Err(DeployError(format!(
            "GitHub runner {runner_name} has no status"
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
