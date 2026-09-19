//! What the fleet is running detached, read back from the queue.
//!
//! There is no second store: a detached session is a queue job carrying the
//! kind's batch, so this walks the lifecycle prefixes and reports the ones
//! that belong to a detachable workload kind.

use serde_json::{json, Value};

use super::detached::{batch_id, workspace_of, RECORD_SCHEMA_VERSION};
use crate::cli::CmdError;
use crate::models::Job;

pub(crate) async fn list_sessions(json_output: bool) -> Result<(), CmdError> {
    let kinds = crate::cli::workload::catalog::catalog()?
        .workloads
        .iter()
        .filter(|workload| workload.detachable)
        .map(|workload| (batch_id(&workload.kind), workload.kind.clone()))
        .collect::<Vec<_>>();
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(|error| CmdError::click(format!("queue storage is unreachable: {error}")))?;
    let mut sessions = Vec::new();
    for prefix in crate::machine::JOB_PREFIXES {
        let jobs = store.list_jobs(prefix, 0).await.map_err(|error| {
            CmdError::click(format!("queue prefix {prefix} is unreadable: {error}"))
        })?;
        for job in jobs {
            let Some((_, kind)) = kinds.iter().find(|(batch, _)| *batch == job.batch_id) else {
                continue;
            };
            sessions.push(session_record(&job, kind, state_of(prefix)));
        }
    }
    sessions.sort_by(|left, right| {
        text(right, "started")
            .cmp(&text(left, "started"))
            .then_with(|| text(left, "job_id").cmp(&text(right, "job_id")))
    });
    if json_output {
        crate::cli::workload::plan::print_json(&json!({
            "schema_version": RECORD_SCHEMA_VERSION,
            "sessions": sessions,
        }));
        return Ok(());
    }
    if sessions.is_empty() {
        println!("no detached workload sessions; start one with `stado workload start <KIND>`");
        return Ok(());
    }
    for session in &sessions {
        println!(
            "{} {} {} on {}",
            text(session, "kind"),
            text(session, "job_id"),
            text(session, "state"),
            text(session, "host")
        );
        println!(
            "  workspace {} started {} holds {} core(s), {} GiB",
            text(session, "workspace"),
            text(session, "started"),
            number(session, "cpu_cores"),
            number(session, "memory_gb")
        );
        println!(
            "  follow it with `stado job watch {} --follow`",
            text(session, "job_id")
        );
    }
    Ok(())
}

fn text(session: &Value, field: &str) -> String {
    session[field]
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
        .to_string()
}

fn number(session: &Value, field: &str) -> String {
    session[field]
        .as_i64()
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

/// The lifecycle prefix, said the way an operator reads it.
fn state_of(prefix: &str) -> &str {
    if prefix == crate::queue::runs::QUEUE {
        "queued"
    } else {
        prefix
    }
}

fn session_record(job: &Job, kind: &str, state: &str) -> Value {
    let host = if job.assigned_to.is_empty() {
        job.pinned_host.clone()
    } else {
        job.assigned_to.clone()
    };
    json!({
        "kind": kind,
        "job_id": job.job_id,
        "run_id": job.run_id,
        "batch_id": job.batch_id,
        "state": state,
        "host": host,
        "pinned_host": job.pinned_host,
        "workspace": workspace_of(kind, &job.run_id),
        "started": job.created_at,
        "cpu_cores": job.cpu_cores,
        "memory_gb": job.memory_gb,
        "command": job.command,
    })
}
