//! `stado job watch` — the log read, and the tail that keeps reading it.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::cli::{reporting::table, CmdError};
use crate::machine::{normalize_job, MachineFacade};
use crate::models::{job_state, Job};
use crate::primitives::constants::POLL_INTERVAL_S;

use super::cmd_error;

mod drain;
mod outcome;

use self::drain::drain;
use self::outcome::outcome;

/// Poll cadence for `--follow`. The bytes being tailed are written by an
/// agent that polls the same store at [`POLL_INTERVAL_S`]; reading faster
/// than the writer writes only multiplies storage round-trips, so the tail
/// rides the fleet's own cadence.
const WATCH_POLL_INTERVAL: Duration = Duration::from_secs(POLL_INTERVAL_S);
/// A successful submit can become visible through the object-backed queue a
/// few polls after its ID is returned. `--follow` waits through that bounded
/// publication window instead of turning normal propagation into NOT_FOUND.
const WATCH_APPEARANCE_TIMEOUT: Duration = Duration::from_secs(60);

/// `stado job watch JOB_ID [--follow] [--json]` — print the log from the
/// start, then (with `--follow`) tail it until the job reaches a terminal
/// prefix and report the outcome.
pub(super) async fn watch(job_id: &str, follow: bool, json: bool) -> Result<(), CmdError> {
    let facade = MachineFacade::new().await.map_err(cmd_error)?;
    let mut cursor = i64::default();
    let mut buffered = String::new();

    let job = loop {
        // State first, log second. A job observed in a terminal prefix has
        // already stopped writing, so the drain below cannot miss its last
        // bytes. The other order would read the log, watch the job go
        // terminal, and exit having dropped whatever landed in between.
        let job = lookup_visible_job(&facade, job_id, follow && cursor == i64::default()).await?;
        let terminal = job_state::is_terminal(&job.state);
        drain(&facade, job_id, &mut cursor, &mut buffered, json).await?;
        if terminal || !follow {
            break job;
        }
        tokio::time::sleep(WATCH_POLL_INTERVAL).await;
    };

    let terminal = job_state::is_terminal(&job.state);
    if json {
        let payload = json!({
            "job": normalize_job(&job),
            "terminal": terminal,
            "log_bytes": cursor,
            "log": buffered,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        let ended = job.completed_at.clone().or_else(|| job.failed_at.clone());
        let row = vec![
            job.job_id.clone(),
            job.state.clone(),
            cursor.to_string(),
            job.started_at.clone().unwrap_or_default(),
            ended.unwrap_or_default(),
            job.error.clone().unwrap_or_default(),
        ];
        table::print(
            &["JOB", "STATE", "LOG BYTES", "STARTED", "ENDED", "ERROR"],
            std::slice::from_ref(&row),
        );
    }
    outcome(&job, terminal)
}

async fn lookup_visible_job(
    facade: &MachineFacade,
    job_id: &str,
    wait_for_publication: bool,
) -> Result<Job, CmdError> {
    let deadline = Instant::now() + WATCH_APPEARANCE_TIMEOUT;
    loop {
        match facade.lookup_job(job_id).await {
            Ok(job) => return Ok(job),
            Err(exc)
                if wait_for_publication && exc.code == "NOT_FOUND" && Instant::now() < deadline =>
            {
                tokio::time::sleep(WATCH_POLL_INTERVAL).await;
            }
            Err(exc) => return Err(cmd_error(exc)),
        }
    }
}
