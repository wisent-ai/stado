//! Handing one failure to a detached Jeden session: the prompt it is given,
//! how the session is started, and what the job's state records about it.
//!
//! The session is started through Stado's own `workload start jeden-session`,
//! so it is placed on a host with room for it, reaches its model through
//! Brama with the fleet's grant, and is an ordinary queue job: retained
//! output, `stado job watch`, `stado cancel`. No model CLI found on this
//! host's PATH, and no credential read here, takes part.

use serde_json::{json, Value};

use super::{
    scan_new_failures, state_load, state_save, FailureRecord, FixError, ALREADY_DISPATCHED,
    DISPATCHED, REFUSED,
};
use crate::queue::JobStorage;

/// The workload kind a fix runs as.
const SESSION_KIND: &str = "jeden-session";

/// Where a fix session works: the workspace the operator names with
/// `--failure-fixer-workspace`.
#[derive(Debug, Clone)]
pub struct FixSession {
    pub workspace: String,
    /// The step bound every fix session runs under, as the host declares it.
    pub max_steps: u32,
}

/// Build the structured prompt a fix session is started with for ONE failed
/// job, with the job's whole recorded error.
pub fn format_fix_prompt(rec: &FailureRecord) -> String {
    format!(
        "You are the wisent-compute autonomous failure-fixer.\n\
         A wisent-compute job (job_id={} batch_id={}) failed at {}.\n\
         \n\
         Diagnose the root cause from the traceback below, ship the fix to \
         the appropriate repo (wisent / wisent-tools / wisent-compute), \
         publish the patched package to PyPI, then resubmit this exact \
         command via `wc submit <command> --verify <verify_command>`. The \
         fleet's local agents drift-pick-up the new PyPI release on their \
         next loop; cloud agents self-terminate on drift so a fresh VM with \
         the new version claims the resubmitted job.\n\
         \n\
         Failed command:\n  {}\n\
         \n\
         Traceback (stderr):\n\
         ---BEGIN TRACEBACK---\n{}\n---END TRACEBACK---\n\
         \n\
         Constraints: never introduce mocks, soft-defaults, or silent \
         error absorption. Diagnose root cause and patch the cause; if \
         the root is in an upstream dependency the wisent-compute team \
         cannot patch, surface that clearly instead of inventing a \
         workaround.",
        rec.job_id, rec.batch_id, rec.failed_at, rec.command, rec.error
    )
}

/// The sessions a job's state says were started for it, oldest first. A job
/// with no state yet has had none; a state that keeps no list of sessions
/// (a record written before sessions were listed, or a damaged one) is
/// refused by name, never read as "never dispatched".
fn sessions_of(job_id: &str, state: &Value) -> Result<Vec<Value>, FixError> {
    if state.as_object().is_some_and(serde_json::Map::is_empty) {
        return Ok(Vec::new());
    }
    state["sessions"].as_array().cloned().ok_or_else(|| {
        FixError::Session(format!(
            "the failure-fixer state of job {job_id} keeps no list of the sessions started for \
             it: {state}"
        ))
    })
}

/// Start the session and read back the placement record it printed.
fn start_session(session: &FixSession, prompt: &str) -> Result<Value, FixError> {
    let stado = std::env::current_exe()?;
    let proc = crate::wait::output(
        std::process::Command::new(&stado)
            .args(["workload", "start", SESSION_KIND, "--workspace"])
            .arg(&session.workspace)
            .arg("--task")
            .arg(prompt)
            .arg("--max-steps")
            .arg(session.max_steps.to_string())
            .args(["--allow-write", "--allow-command", "--json"]),
    )?;
    if !proc.status.success() {
        return Err(FixError::Session(format!(
            "`stado workload start {SESSION_KIND} --workspace {}` ended with {}: {}",
            session.workspace,
            proc.status,
            String::from_utf8_lossy(&proc.stderr).trim()
        )));
    }
    let record: Value = serde_json::from_slice(&proc.stdout)?;
    if !record["job_id"].is_string() {
        return Err(FixError::Session(format!(
            "`stado workload start {SESSION_KIND}` printed a record without a job_id: {record}"
        )));
    }
    Ok(record)
}

/// Start one fix session for ONE failed job, unless this same failure
/// already had one. Returns the dispatch record (JSON).
pub async fn dispatch_fix(
    rec: &FailureRecord,
    store: &JobStorage,
    session: &FixSession,
) -> Result<Value, FixError> {
    let state = state_load(store, &rec.job_id).await?;
    let mut sessions = sessions_of(&rec.job_id, &state)?;
    if state["failed_at"].as_str() == Some(rec.failed_at.as_str()) {
        return Ok(json!({
            "job_id": rec.job_id,
            "status": ALREADY_DISPATCHED,
            "attempts": sessions.len(),
            "failed_at": rec.failed_at,
        }));
    }
    let placed = start_session(session, &format_fix_prompt(rec))?;
    sessions.push(json!({
        "dispatched_at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "failed_at": rec.failed_at,
        "job_id": placed["job_id"],
        "target": placed["target"],
        "run_id": placed["run_id"],
    }));
    let state = json!({
        "sessions": sessions,
        "failed_at": rec.failed_at,
        "batch_id": rec.batch_id,
        "command": rec.command,
    });
    state_save(store, &rec.job_id, &state).await?;
    Ok(json!({
        "job_id": rec.job_id,
        "status": DISPATCHED,
        "attempts": sessions.len(),
        "session": placed,
    }))
}

/// Scan failed/ and start one fix session per failure not handled yet.
/// `skip_dispatched` skips every job that ever had a session, not only the
/// failure that had it, and records it as ALREADY_DISPATCHED. A job whose
/// fix cannot be started, or whose state cannot be read, is listed as
/// refused with the cause; the other jobs of the pass still get theirs.
pub async fn scan_and_dispatch(
    since_iso: Option<&str>,
    command_pattern: Option<&str>,
    session: &FixSession,
    store: &JobStorage,
    skip_dispatched: bool,
) -> Result<Vec<Value>, FixError> {
    let mut out = Vec::new();
    for rec in scan_new_failures(store, since_iso, command_pattern).await? {
        let outcome = async {
            if skip_dispatched {
                let prior = state_load(store, &rec.job_id).await?;
                let sessions = sessions_of(&rec.job_id, &prior)?;
                if !sessions.is_empty() {
                    return Ok(json!({
                        "job_id": rec.job_id,
                        "status": ALREADY_DISPATCHED,
                        "attempts": sessions.len(),
                    }));
                }
            }
            dispatch_fix(&rec, store, session).await
        }
        .await;
        out.push(match outcome {
            Ok(record) => record,
            Err(error) => json!({
                "job_id": rec.job_id,
                "status": REFUSED,
                "error": error.to_string(),
            }),
        });
    }
    Ok(out)
}
