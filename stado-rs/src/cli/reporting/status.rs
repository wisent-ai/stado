//! `stado status [FILTER_ID] [--json]`: provider-neutral Stado queue status.

use chrono::Utc;
use serde_json::{json, Value};

use crate::deploy::fleet_claim;
use crate::models::Job;
use crate::queue::runs;
use crate::queue::storage::JobStorage;
use crate::queue::submit::default_store;

use crate::cli::CmdError;

/// Canonical lifecycle states in display and direct-lookup order, composed
/// from the prefixes the queue declares.
const STATES: &[&str] = &[
    runs::RUNNING,
    runs::QUEUE,
    runs::COMPLETED,
    runs::UPLOADED,
    runs::FAILED,
    runs::CANCELLED,
];

/// One row of the listing: a job, the lifecycle prefix it was read under,
/// and whether that reading came from the outcome its run retained after
/// the run reaper deleted the job's own documents.
struct Row {
    job: Job,
    state: String,
    reaped: bool,
}

impl Row {
    fn live(job: Job, state: &str) -> Self {
        Self {
            job,
            state: state.to_string(),
            reaped: false,
        }
    }

    /// A retained job carries the terminal prefix it ended in as its state
    /// ([`runs::retained_job`]).
    fn reaped(job: Job) -> Self {
        Self {
            state: job.state.clone(),
            job,
            reaped: true,
        }
    }

    fn json(&self) -> Value {
        let mut record = serde_json::to_value(&self.job).expect("a job serializes");
        record["state"] = json!(self.state);
        record["reaped"] = json!(self.reaped);
        record
    }
}

pub async fn run(filter_id: Option<&str>, json: bool) -> Result<(), CmdError> {
    let store = default_store(crate::config::bucket()).await?;
    // Direct read of one job: a whole id (`job-` and its hex) is read under
    // every lifecycle state at once, then from the outcome its run kept once
    // the run reaper deleted the job's own documents; the start of an id,
    // with or without `job-`, lists every job it begins — in the queue, and
    // among the reaped through the index retention writes. An id no job
    // holds is a refusal naming it. Any other text — a batch id, or a
    // substring of either — filters the listing below.
    if let Some(prefix) = filter_id.and_then(crate::queue::submit::job_id_prefix) {
        let rows = if crate::queue::submit::is_canonical_job_id(&prefix) {
            whole_id(&store, &prefix).await?
        } else {
            id_prefix(&store, &prefix).await?
        };
        if rows.is_empty() {
            return Err(CmdError::missing(format!(
                "no job with id {} in the queue or in any run's retained outcomes; `stado \
                 status` lists the jobs the queue holds",
                filter_id.expect("a prefix came from the filter")
            ))
            .machine_readable(json));
        }
        print_rows(&rows, json);
        return Ok(());
    }

    // Slow path: no filter, or filter is a batch_id — must scan all blobs.
    let mut all_jobs = store.list_all_jobs().await?;
    let mut rows = Vec::new();
    for state in STATES.iter().copied() {
        for job in all_jobs.remove(state).into_iter().flatten() {
            if let Some(filter) = filter_id {
                if !job.job_id.contains(filter) && !job.batch_id.contains(filter) {
                    continue;
                }
            }
            rows.push(Row::live(job, state));
        }
    }
    let queued_listed = rows.iter().any(|row| row.state == runs::QUEUE);
    print_rows(&rows, json);
    if json {
        return Ok(());
    }
    let count = |state: &str| rows.iter().filter(|row| row.state == state).count();
    // `completed` is a TERMINAL success state
    // ([`crate::models::job_state::is_terminal`]), and `uploaded` is the
    // separate terminal state an HuggingFace upload worker sets. This line
    // used to call the `completed` count "extracted (awaiting upload)", which
    // says those jobs are waiting for something: completed jobs get read as
    // a stalled upload queue, tied to whatever else is misbehaving on the
    // host, and offered as one defect with several faces when nothing was
    // ever awaiting an upload. A count is named after the state it
    // counts.
    println!(
        "\n{} running, {} queued, {} completed, {} uploaded, {} failed, {} cancelled",
        count(runs::RUNNING),
        count(runs::QUEUE),
        count(runs::COMPLETED),
        count(runs::UPLOADED),
        count(runs::FAILED),
        count(runs::CANCELLED),
    );

    // Why the queue is not moving, under the queue it is not moving. A row
    // that says `queue` and a count that says "1 queued" are the same
    // sentence an empty fleet and a busy one both print; a job sat here for
    // days while nothing in the product said that not one host
    // was publishing capacity. Printed only when work is queued AND nothing
    // can take it, and never as a failure: this listing is a report, so the
    // exit status stays zero either way.
    // The LISTING's queued rows, not the store's: a filtered listing that
    // shows no queued work is not the surface this verdict explains.
    if !queued_listed {
        return Ok(());
    }
    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(CmdError::from)?;
    let claim = fleet_claim::read_fleet_claim(&store, &registry, Utc::now())
        .await
        .map_err(CmdError::from)?;
    for line in claim.lines() {
        println!("{line}");
    }
    Ok(())
}

/// One whole job id: read under every lifecycle state at once, then from
/// the outcome its run retained.
async fn whole_id(store: &JobStorage, job_id: &str) -> Result<Vec<Row>, CmdError> {
    let reads = STATES.iter().copied().map(|state| {
        let store = store.clone();
        async move { (state, store.read_job(state, job_id).await) }
    });
    let mut rows = Vec::new();
    for (state, result) in futures::future::join_all(reads).await {
        if let Some(job) = result? {
            rows.push(Row::live(job, state));
        }
    }
    if rows.is_empty() {
        if let Some(job) = runs::retained_job(store, job_id).await? {
            rows.push(Row::reaped(job));
        }
    }
    Ok(rows)
}

/// The start of a job id: every job the queue holds whose id begins with
/// it, then every reaped job the index retention writes names under it.
async fn id_prefix(store: &JobStorage, prefix: &str) -> Result<Vec<Row>, CmdError> {
    let mut all_jobs = store.list_all_jobs().await?;
    let mut rows = Vec::new();
    for state in STATES.iter().copied() {
        for job in all_jobs.remove(state).into_iter().flatten() {
            if job.job_id.starts_with(prefix) {
                rows.push(Row::live(job, state));
            }
        }
    }
    for job in runs::retained_jobs_with_prefix(store, prefix).await? {
        if !rows.iter().any(|row| row.job.job_id == job.job_id) {
            rows.push(Row::reaped(job));
        }
    }
    Ok(rows)
}

/// The rows as the operator reads them, or as one JSON array.
fn print_rows(rows: &[Row], json: bool) {
    if json {
        let printed = rows.iter().map(Row::json).collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(&printed).expect("rows serialize")
        );
        return;
    }
    println!(
        "{:<12} {:<10} {:<18} {:<22} COMMAND",
        "JOB ID", "STATE", "GPU", "SUBMITTED_BY"
    );
    println!("{}", "-".repeat(110));
    for row in rows {
        print_job_row(row);
    }
}

/// Python `_print_job_row`.
fn print_job_row(row: &Row) {
    let job = &row.job;
    // Whole values: the columns pad short ones and the command, the last
    // column, runs as long as it is.
    let cmd = job.command.split_whitespace().collect::<Vec<_>>().join(" ");
    let submitted_by = if job.submitted_by.is_empty() {
        "?"
    } else {
        job.submitted_by.as_str()
    };
    let who = format!("{submitted_by}@{}", job.submitted_from);
    let gpu = if job.gpu_type.is_empty() {
        "cpu"
    } else {
        job.gpu_type.as_str()
    };
    let state = if row.reaped {
        format!("{} (reaped)", row.state)
    } else {
        row.state.clone()
    };
    println!("{:<12} {state:<10} {gpu:<18} {who:<22} {cmd}", job.job_id);
}
