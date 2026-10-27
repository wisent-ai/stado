//! The forge's REST API as `stado ci status` reads it: one client that names
//! Stado and its release, the answer reader that refuses by endpoint and
//! status, and the workflow, run and job reads built on them.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

pub(super) const API: &str = "https://api.github.com";
const ACCEPT: &str = "application/vnd.github+json";
const API_VERSION_HEADER: &str = "X-GitHub-Api-Version";
const API_VERSION: &str = "2022-11-28";
/// The forge's own conclusion for a run, a job or a step that passed.
const SUCCESS: &str = "success";
/// The forge's jobs listing filter that reads each job's latest attempt.
const LATEST_ATTEMPT: &str = "filter=latest";
/// The forge refuses a request that names no client ("All API requests must
/// include a valid User-Agent header",
/// https://docs.github.com/en/rest/using-the-rest-api/getting-started-with-the-rest-api#user-agent),
/// so every read names this program and its release.
const USER_AGENT: &str = concat!("stado/", env!("CARGO_PKG_VERSION"), " ci");

pub(super) fn client(token: &str) -> impl Fn(String) -> reqwest::RequestBuilder + '_ {
    let client = reqwest::Client::new();
    move |endpoint: String| {
        client
            .get(endpoint)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::ACCEPT, ACCEPT)
            .header(API_VERSION_HEADER, API_VERSION)
            .bearer_auth(token)
    }
}

/// One forge answer as JSON, or the refusal naming the endpoint and status.
pub(super) async fn answer(
    request: reqwest::RequestBuilder,
    what: &str,
) -> Result<Value, CmdError> {
    let response = crate::wait::request(request)
        .await
        .map_err(|error| CmdError::from(error).within(format!("cannot read {what}")))?;
    let status = response.status();
    if !status.is_success() {
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => format!("(body unreadable: {error})"),
        };
        return Err(CmdError::click(format!(
            "the forge answered HTTP {status} for {what}: {}",
            body.trim()
        ))
        .stating(FailureCode::from_upstream_status(status.as_u16())));
    }
    response.json().await.map_err(|error| {
        CmdError::click(format!(
            "the forge's answer for {what} is not JSON: {error}"
        ))
        .stating(FailureCode::InfraDown)
    })
}

/// Whether the forge concluded the item as anything but a pass.
fn not_passed(item: &Value) -> bool {
    item["conclusion"]
        .as_str()
        .is_some_and(|conclusion| conclusion != SUCCESS)
}

/// The `key` list of one forge answer about `what`, refused when the answer
/// holds none, or holds fewer entries than the `total_count` beside it says
/// the forge has: a list cut at the forge's page would read as all of it.
pub(super) fn listing<'a>(
    answer: &'a Value,
    key: &str,
    what: &str,
) -> Result<&'a [Value], CmdError> {
    let items = answer[key].as_array().ok_or_else(|| {
        CmdError::click(format!(
            "the forge's answer for {what} holds no {key} list: {answer}"
        ))
        .stating(FailureCode::InfraDown)
    })?;
    if let Some(total) = answer["total_count"].as_u64() {
        if total > items.len() as u64 {
            return Err(CmdError::click(format!(
                "the forge lists {} of the {total} {key} of {what} on its first page, so the \
                 answer would leave the rest out",
                items.len()
            ))
            .stating(FailureCode::InfraDown));
        }
    }
    Ok(items)
}

/// The jobs of `run_id` that did not succeed, each with the step that
/// failed in it.
async fn failed_jobs(
    request: &impl Fn(String) -> reqwest::RequestBuilder,
    repository: &str,
    run_id: u64,
) -> Result<Vec<Value>, CmdError> {
    let jobs = answer(
        request(format!(
            "{API}/repos/{repository}/actions/runs/{run_id}/jobs?{LATEST_ATTEMPT}"
        )),
        &format!("the jobs of run {run_id}"),
    )
    .await?;
    let mut failed = Vec::new();
    for job in listing(&jobs, "jobs", &format!("run {run_id}"))? {
        if !not_passed(job) {
            continue;
        }
        let steps = job["steps"].as_array().ok_or_else(|| {
            CmdError::click(format!(
                "the forge's job {} of run {run_id} holds no steps list",
                crate::cli::reporting::field(&job["name"])
            ))
            .stating(FailureCode::InfraDown)
        })?;
        let failed_step = steps.iter().find(|step| not_passed(step)).map(|step| {
            json!({ "name": step["name"], "conclusion": step["conclusion"], "number": step["number"] })
        });
        failed.push(json!({
            "name": job["name"],
            "conclusion": job["conclusion"],
            "url": job["html_url"],
            "failedStep": failed_step,
        }));
    }
    Ok(failed)
}

/// The newest run of one workflow, of `revision` when given: the forge
/// lists runs newest first, so the first of its page is the newest.
pub(super) async fn newest_run(
    request: &impl Fn(String) -> reqwest::RequestBuilder,
    repository: &str,
    workflow: &Value,
    revision: Option<&str>,
) -> Result<Value, CmdError> {
    let id = workflow["id"].as_u64().ok_or_else(|| {
        CmdError::click(format!(
            "a workflow of {repository} carries no id: {workflow}"
        ))
        .stating(FailureCode::InfraDown)
    })?;
    let name = &workflow["name"];
    let mut endpoint = format!("{API}/repos/{repository}/actions/workflows/{id}/runs");
    if let Some(revision) = revision {
        endpoint.push_str(&format!("?head_sha={revision}"));
    }
    let runs = answer(request(endpoint), &format!("the runs of workflow {name}")).await?;
    let Some(run) = runs["workflow_runs"]
        .as_array()
        .and_then(|runs| runs.first())
    else {
        return Ok(json!({
            "workflow": name,
            "path": workflow["path"],
            "state": workflow["state"],
            "run": Value::Null,
            "finding": match revision {
                Some(revision) => format!("no run of this workflow for revision {revision}"),
                None => String::from("this workflow has never run"),
            },
        }));
    };
    let run_id = run["id"].as_u64().ok_or_else(|| {
        CmdError::click(format!("a run of workflow {name} carries no id: {run}"))
            .stating(FailureCode::InfraDown)
    })?;
    let failed = if not_passed(run) {
        failed_jobs(request, repository, run_id).await?
    } else {
        Vec::new()
    };
    Ok(json!({
        "workflow": name,
        "path": workflow["path"],
        "state": workflow["state"],
        "run": {
            "id": run_id,
            "number": run["run_number"],
            "status": run["status"],
            "conclusion": run["conclusion"],
            "revision": run["head_sha"],
            "branch": run["head_branch"],
            "event": run["event"],
            "startedAt": run["run_started_at"],
            "updatedAt": run["updated_at"],
            "url": run["html_url"],
            "failedJobs": failed,
        },
    }))
}
