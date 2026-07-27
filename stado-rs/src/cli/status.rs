//! `stado status [FILTER_ID]` — port of the `status` command in
//! `stado/cli.py`: API mode when COMPUTE_API_KEY is set, queue-storage
//! scan otherwise. The table format matches cli.py's inline renderer
//! (the Python `dashboard_summary/status_view.py` web renderer is a
//! different surface and is not involved here).

use serde_json::Value;

use crate::models::Job;
use crate::queue::submit::default_store;

use super::CmdError;

/// Python `_STATES` — display + scan order.
const STATES: [&str; 5] = ["running", "queue", "completed", "uploaded", "failed"];

pub async fn run(filter_id: Option<&str>) -> Result<(), CmdError> {
    if !super::api_key().is_empty() {
        status_api(filter_id).await
    } else {
        status_gcs(filter_id).await
    }
}

/// Python `_api_get`.
async fn api_get(path: &str) -> Result<Value, CmdError> {
    let response = reqwest::Client::new()
        .get(format!("{}{path}", crate::config::compute_api()))
        .header("X-API-Key", super::api_key())
        .send()
        .await?;
    Ok(response.json::<Value>().await?)
}

/// API mode (Python `_status_api`).
async fn status_api(filter_id: Option<&str>) -> Result<(), CmdError> {
    let instances = api_get("/api/v1/instances").await?;
    let instances = instances.as_array().cloned().unwrap_or_default();
    println!("{:<38} {:<12} {:<30} COST", "ID", "STATUS", "IMAGE");
    println!("{}", "-".repeat(95));
    for inst in &instances {
        let iid = inst.get("id").and_then(Value::as_str).unwrap_or("");
        let iid: String = iid.chars().take(36).collect();
        let st = inst.get("status").and_then(Value::as_str).unwrap_or("");
        let img = inst
            .get("docker_image")
            .and_then(Value::as_str)
            .unwrap_or("");
        let img: String = img.chars().take(28).collect();
        let cost = inst
            .get("total_cost_cents")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            / 100.0;
        if let Some(filter) = filter_id {
            if !iid.contains(filter) {
                continue;
            }
        }
        println!("{iid:<38} {st:<12} {img:<30} ${cost:.2}");
    }
    println!("\n{} instance(s)", instances.len());
    Ok(())
}

/// Python `_print_job_row`.
fn print_job_row(job: &Job, state: &str) {
    let cmd_one_line = job.command.split_whitespace().collect::<Vec<_>>().join(" ");
    let cmd: String = if cmd_one_line.chars().count() > 42 {
        format!("{}...", cmd_one_line.chars().take(42).collect::<String>())
    } else {
        cmd_one_line
    };
    let submitted_by = if job.submitted_by.is_empty() {
        "?"
    } else {
        job.submitted_by.as_str()
    };
    let submitted_from: String = job.submitted_from.chars().take(12).collect();
    let who: String = format!("{submitted_by}@{submitted_from}")
        .chars()
        .take(22)
        .collect();
    let gpu = if job.gpu_type.is_empty() {
        "cpu"
    } else {
        job.gpu_type.as_str()
    };
    println!("{:<12} {state:<10} {gpu:<18} {who:<22} {cmd}", job.job_id);
}

/// Queue-storage scan mode (Python `_status_gcs`).
async fn status_gcs(filter_id: Option<&str>) -> Result<(), CmdError> {
    let store = default_store(crate::config::bucket()).await?;
    println!(
        "{:<12} {:<10} {:<18} {:<22} COMMAND",
        "JOB ID", "STATE", "GPU", "SUBMITTED_BY"
    );
    println!("{}", "-".repeat(110));

    // Fast path: filter looks like a job_id — 5 parallel direct reads, no listing.
    let job_id_re = regex::Regex::new(r"^[0-9a-f]{8}$").expect("static regex compiles");
    if let Some(filter) = filter_id.filter(|f| job_id_re.is_match(f)) {
        let reads = STATES.map(|state| {
            let store = store.clone();
            async move { (state, store.read_job(state, filter).await) }
        });
        let results = futures::future::join_all(reads).await;
        let mut found = false;
        for (state, result) in results {
            if let Some(job) = result? {
                print_job_row(&job, state);
                found = true;
            }
        }
        if !found {
            println!("(no job with id {filter})");
        }
        return Ok(());
    }

    // Slow path: no filter, or filter is a batch_id — must scan all blobs.
    let all_jobs = store.list_all_jobs().await?;
    for state in STATES {
        for job in &all_jobs[state] {
            if let Some(filter) = filter_id {
                if !job.job_id.contains(filter) && !job.batch_id.contains(filter) {
                    continue;
                }
            }
            print_job_row(job, state);
        }
    }
    println!(
        "\n{} running, {} queued, {} extracted (awaiting upload), {} uploaded, {} failed",
        all_jobs["running"].len(),
        all_jobs["queue"].len(),
        all_jobs["completed"].len(),
        all_jobs["uploaded"].len(),
        all_jobs["failed"].len(),
    );
    Ok(())
}
