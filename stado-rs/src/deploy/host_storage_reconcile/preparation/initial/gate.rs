use super::*;

pub(in crate::deploy::host_storage_reconcile) async fn repository_runner_gate(
) -> Result<Option<Value>, DeployError> {
    if let Some(gate) = RESIDENT_RUNNER_GATE.get() {
        return Ok(Some(gate.clone()));
    }
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return Ok(None);
    }
    let required = |name: &str| {
        std::env::var(name).map_err(|_| {
            DeployError(format!(
                "{name} is required when storage reconciliation owns an Actions runner"
            ))
        })
    };
    let repository = required("GITHUB_REPOSITORY")?;
    let owner = repository
        .split_once('/')
        .map(|(owner, _)| owner)
        .filter(|owner| !owner.is_empty())
        .ok_or_else(|| DeployError("GITHUB_REPOSITORY is not owner/repository".to_string()))?;
    let current_runner = required("RUNNER_NAME")?;
    let run_id = required("GITHUB_RUN_ID")?;
    let source_sha = required("GITHUB_SHA")?;
    let token = crate::deploy::host_precheck_runner::github_credential().await?;
    let client = reqwest::Client::new();
    let request = |endpoint: String| {
        client
            .get(endpoint)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .bearer_auth(&token)
    };

    let run_endpoint = format!("https://api.github.com/repos/{repository}/actions/runs/{run_id}");
    let run_response = request(run_endpoint)
        .send()
        .await
        .map_err(|error| DeployError(format!("cannot read current workflow run: {error}")))?;
    if !run_response.status().is_success() {
        return Err(DeployError(format!(
            "current workflow run returned HTTP {}",
            run_response.status()
        )));
    }
    let run: Value = run_response
        .json()
        .await
        .map_err(|error| DeployError(format!("invalid current workflow run: {error}")))?;
    if run
        .get("id")
        .and_then(Value::as_u64)
        .map(|id| id.to_string())
        != Some(run_id.clone())
        || run.get("head_sha").and_then(Value::as_str) != Some(source_sha.as_str())
        || !matches!(
            run.get("status").and_then(Value::as_str),
            Some("in_progress" | "queued")
        )
    {
        return Err(DeployError(
            "GitHub run identity does not match this source invocation".to_string(),
        ));
    }

    let jobs_endpoint = format!(
        "https://api.github.com/repos/{repository}/actions/runs/{run_id}/jobs?filter=latest&per_page=100"
    );
    let jobs_response = request(jobs_endpoint)
        .send()
        .await
        .map_err(|error| DeployError(format!("cannot read current workflow jobs: {error}")))?;
    if !jobs_response.status().is_success() {
        return Err(DeployError(format!(
            "current workflow jobs returned HTTP {}",
            jobs_response.status()
        )));
    }
    let jobs: Value = jobs_response
        .json()
        .await
        .map_err(|error| DeployError(format!("invalid current workflow jobs: {error}")))?;
    let job_rows = jobs
        .get("jobs")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("current workflow jobs omitted jobs".to_string()))?;
    if jobs.get("total_count").and_then(Value::as_u64) != Some(job_rows.len() as u64) {
        return Err(DeployError(
            "current workflow jobs response was paginated or incomplete".to_string(),
        ));
    }
    let executing = job_rows
        .iter()
        .filter(|job| {
            job.get("runner_name").and_then(Value::as_str) == Some(current_runner.as_str())
                && job.get("status").and_then(Value::as_str) == Some("in_progress")
        })
        .collect::<Vec<_>>();
    if executing.len() != 1 {
        return Err(DeployError(format!(
            "expected one in-progress job on runner {current_runner:?}, found {}",
            executing.len()
        )));
    }
    let current_job = executing[0];
    let current_runner_id = current_job
        .get("runner_id")
        .and_then(Value::as_u64)
        .ok_or_else(|| DeployError("current workflow job omitted runner_id".to_string()))?;
    let current_job_id = current_job
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| DeployError("current workflow job omitted id".to_string()))?;

    let repositories = [
        repository.clone(),
        format!("{owner}/wisent-backend"),
        format!("{owner}/brama"),
    ];
    let mut current_online_busy = false;
    let mut other_busy = Vec::new();
    let mut inventory = Vec::new();
    for repository_name in &repositories {
        let endpoint =
            format!("https://api.github.com/repos/{repository_name}/actions/runners?per_page=100");
        let response = request(endpoint).send().await.map_err(|error| {
            DeployError(format!(
                "cannot read runners for {repository_name}: {error}"
            ))
        })?;
        if !response.status().is_success() {
            return Err(DeployError(format!(
                "runner inventory for {repository_name} returned HTTP {}",
                response.status()
            )));
        }
        let body: Value = response.json().await.map_err(|error| {
            DeployError(format!(
                "invalid runner inventory for {repository_name}: {error}"
            ))
        })?;
        let runners = body
            .get("runners")
            .and_then(Value::as_array)
            .ok_or_else(|| DeployError(format!("{repository_name} omitted runners")))?;
        if body.get("total_count").and_then(Value::as_u64) != Some(runners.len() as u64) {
            return Err(DeployError(format!(
                "runner inventory for {repository_name} was paginated or incomplete"
            )));
        }
        for runner_row in runners {
            let id = runner_row.get("id").and_then(Value::as_u64);
            let name = runner_row
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let busy = runner_row.get("busy").and_then(Value::as_bool) == Some(true);
            let online = runner_row.get("status").and_then(Value::as_str) == Some("online");
            if id == Some(current_runner_id) {
                current_online_busy |= online && busy;
            } else if busy {
                other_busy.push(json!({"repository": repository_name, "id": id, "name": name}));
            }
            inventory.push(json!({
                "repository": repository_name,
                "id": id,
                "name": name,
                "online": online,
                "busy": busy,
            }));
        }
    }
    if !current_online_busy || !other_busy.is_empty() {
        return Err(DeployError(format!(
            "fleet runner fence refused: current_online_busy={current_online_busy}, other_busy={other_busy:?}"
        )));
    }
    Ok(Some(json!({
        "repositories": repositories,
        "current_repository": repository,
        "current_run_id": run_id,
        "current_job_id": current_job_id,
        "current_job_name": current_job.get("name"),
        "current_runner": current_runner,
        "current_runner_id": current_runner_id,
        "current_online_busy": true,
        "other_busy": other_busy,
        "inventory": inventory,
        "source_sha": source_sha,
        "checked_at": Utc::now().timestamp(),
    })))
}
