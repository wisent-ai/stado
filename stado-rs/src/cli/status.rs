//! `stado status [FILTER_ID]`: provider-neutral Stado queue status.

use crate::models::Job;
use crate::queue::submit::default_store;

use super::CmdError;

/// Canonical lifecycle states in display and direct-lookup order.
const STATES: &[&str] = &[
    "running",
    "queue",
    "completed",
    "uploaded",
    "failed",
    "cancelled",
];

pub async fn run(filter_id: Option<&str>) -> Result<(), CmdError> {
    status_queue(filter_id).await
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

/// Provider-neutral queue-storage scan.
async fn status_queue(filter_id: Option<&str>) -> Result<(), CmdError> {
    let store = default_store(crate::config::bucket()).await?;
    println!(
        "{:<12} {:<10} {:<18} {:<22} COMMAND",
        "JOB ID", "STATE", "GPU", "SUBMITTED_BY"
    );
    println!("{}", "-".repeat(110));

    // Fast path: direct parallel reads across every canonical lifecycle state.
    let job_id_re = regex::Regex::new(r"^[0-9a-f]{8}$").expect("static regex compiles");
    if let Some(filter) = filter_id.filter(|f| job_id_re.is_match(f)) {
        let reads = STATES.iter().copied().map(|state| {
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
    for state in STATES.iter().copied() {
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
        "\n{} running, {} queued, {} extracted (awaiting upload), {} uploaded, {} failed, {} cancelled",
        all_jobs["running"].len(),
        all_jobs["queue"].len(),
        all_jobs["completed"].len(),
        all_jobs["uploaded"].len(),
        all_jobs["failed"].len(),
        all_jobs["cancelled"].len(),
    );
    Ok(())
}
