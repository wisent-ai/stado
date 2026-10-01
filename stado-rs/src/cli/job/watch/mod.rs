//! `stado job watch` — the log read, and the tail that keeps reading it.

use std::time::Duration;

use serde_json::json;

use crate::cli::{reporting::table, CmdError};
use crate::machine::normalize_job;
use crate::machine::MachineFacade;
use crate::models::job_state;

use super::cmd_error;

mod drain;
mod outcome;

use self::drain::drain;
use self::outcome::outcome;

/// `stado job watch JOB_ID [--follow-seconds N] [--json]` — print the log
/// from the start, then (with `--follow-seconds`) re-read it every N seconds
/// until the job reaches a terminal prefix and report the outcome.
pub(super) async fn watch(
    job_id: &str,
    follow: Option<Duration>,
    json: bool,
) -> Result<(), CmdError> {
    let facade = MachineFacade::new().await.map_err(cmd_error)?;
    let mut cursor = i64::default();
    let mut buffered = String::new();

    let job = loop {
        // State first, log second. A job observed in a terminal prefix has
        // already stopped writing, so the drain below cannot miss its last
        // bytes. The other order would read the log, watch the job go
        // terminal, and exit having dropped whatever landed in between.
        let job = facade.lookup_job(job_id).await.map_err(cmd_error)?;
        let terminal = job_state::is_terminal(&job.state);
        drain(&facade, job_id, &mut cursor, &mut buffered, json).await?;
        match follow {
            Some(period) if !terminal => tokio::time::sleep(period).await,
            _ => break job,
        }
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
