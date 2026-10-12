//! `stado ci status <PRODUCT> [--revision SHA] [--json]`: what the forge's
//! continuous integration last did for a catalogued product, read from the
//! forge's own API with the identity Stado's GitHub declaration names in
//! Skarbiec.
//!
//! The product's repository is its catalog record's `owner_repository`; its
//! workflows are the ones that repository declares under `.github/workflows`
//! as the forge lists them. For each workflow the newest run — of one
//! revision when `--revision` names it — is read with its conclusion, and a
//! run that did not succeed carries every job that failed with the step
//! that failed in it, so the answer says what broke, not only that something
//! did. A product whose repository declares no workflow is answered as such,
//! which is a finding about the product, not a refusal. Skarbiec, the
//! declaration and the forge each refuse by name when they do not answer;
//! nothing is guessed in their place. The text form prints each field as
//! the forge gave it, `null` where the forge gave none.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

const API: &str = "https://api.github.com";
const ACCEPT: &str = "application/vnd.github+json";
const API_VERSION_HEADER: &str = "X-GitHub-Api-Version";
const API_VERSION: &str = "2022-11-28";
/// The forge's own conclusion for a run, a job or a step that passed.
const SUCCESS: &str = "success";
/// The forge's jobs listing filter that reads each job's latest attempt.
const LATEST_ATTEMPT: &str = "filter=latest";

fn client(token: &str) -> impl Fn(String) -> reqwest::RequestBuilder + '_ {
    let client = reqwest::Client::new();
    move |endpoint: String| {
        client
            .get(endpoint)
            .header(reqwest::header::ACCEPT, ACCEPT)
            .header(API_VERSION_HEADER, API_VERSION)
            .bearer_auth(token)
    }
}

/// One forge answer as JSON, or the refusal naming the endpoint and status.
async fn answer(request: reqwest::RequestBuilder, what: &str) -> Result<Value, CmdError> {
    let response = crate::wait::request(request)
        .await
        .map_err(|error| CmdError::from(error).within(&format!("cannot read {what}")))?;
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

/// The catalogued product's repository, `owner/name`.
fn repository(product: &str) -> Result<String, CmdError> {
    let catalog = stado_product::catalog::embedded().map_err(|error| {
        CmdError::click(format!(
            "the compiled product catalog is invalid: {error:#}"
        ))
    })?;
    let record = stado_product::catalog::product(&catalog, product).map_err(|error| {
        CmdError::usage(format!(
            "{error:#}; `stado product catalog` lists the products"
        ))
    })?;
    stado_product::catalog::text(record, "owner_repository")
        .map(str::to_string)
        .map_err(|error| CmdError::click(format!("{product}: {error:#}")))
}

/// Whether the forge concluded the item as anything but a pass.
fn not_passed(item: &Value) -> bool {
    item["conclusion"]
        .as_str()
        .is_some_and(|conclusion| conclusion != SUCCESS)
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
    Ok(jobs["jobs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|job| not_passed(job))
        .map(|job| {
            let failed_step = job["steps"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|step| not_passed(step))
                .map(|step| json!({ "name": step["name"], "conclusion": step["conclusion"], "number": step["number"] }));
            json!({
                "name": job["name"],
                "conclusion": job["conclusion"],
                "url": job["html_url"],
                "failedStep": failed_step,
            })
        })
        .collect())
}

/// The newest run of one workflow, of `revision` when given: the forge
/// lists runs newest first, so the first of its page is the newest.
async fn newest_run(
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

/// `stado ci status`: the answer document.
pub async fn status_document(product: &str, revision: Option<&str>) -> Result<Value, CmdError> {
    let repository = repository(product)?;
    let token = crate::github_identity::credential()
        .await
        .map_err(|error| {
            CmdError::from(error).within("the forge identity Stado acts as is not readable")
        })?;
    let request = client(&token);
    let listed = answer(
        request(format!("{API}/repos/{repository}/actions/workflows")),
        &format!("the workflows of {repository}"),
    )
    .await?;
    let workflows: Vec<&Value> = listed["workflows"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let mut rows = Vec::with_capacity(workflows.len());
    for workflow in &workflows {
        rows.push(newest_run(&request, &repository, workflow, revision).await?);
    }
    let finding = workflows
        .is_empty()
        .then(|| format!("{repository} declares no workflow under .github/workflows, so nothing runs for it on the forge"));
    Ok(json!({
        "product": product,
        "repository": repository,
        "revision": revision,
        "workflows": rows,
        "finding": finding,
    }))
}

/// `stado ci status <PRODUCT> [--revision SHA] [--json]`.
pub async fn status(product: &str, revision: Option<&str>, json: bool) -> Result<(), CmdError> {
    let document = status_document(product, revision).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&document)
                .map_err(|error| CmdError::click(error.to_string()))?
        );
        return Ok(());
    }
    println!("{product} ({})", document["repository"]);
    if let Some(finding) = document["finding"].as_str() {
        println!("  {finding}");
    }
    for row in document["workflows"].as_array().into_iter().flatten() {
        let workflow = &row["workflow"];
        match row["run"].as_object() {
            None => println!("  {workflow}: {}", row["finding"]),
            Some(run) => {
                println!(
                    "  {workflow}: concluded {} (status {}) run {} on {} at {} {}",
                    run["conclusion"],
                    run["status"],
                    run["number"],
                    run["revision"],
                    run["updatedAt"],
                    run["url"],
                );
                for job in run["failedJobs"].as_array().into_iter().flatten() {
                    println!(
                        "    failed job {}: step {} ({})",
                        job["name"], job["failedStep"]["name"], job["url"]
                    );
                }
            }
        }
    }
    Ok(())
}
