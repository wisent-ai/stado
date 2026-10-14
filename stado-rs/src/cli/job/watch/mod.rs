//! `stado job watch` — the log read, and the follow that keeps reading it.

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

/// `stado job watch JOB_ID [--follow] [--json]` — print the log from the
/// start, then (with `--follow`) hold on the store's change watch and print
/// what is written until the job reaches a terminal prefix, and report the
/// outcome. A store without change notification refuses `--follow` with its
/// own reason.
pub(super) async fn watch(job_id: &str, follow: bool, json: bool) -> Result<(), CmdError> {
    let facade = MachineFacade::new().await.map_err(cmd_error)?;
    let mut cursor = 0;
    let mut buffered = String::new();
    // Armed before the first read, so a change between that read and the
    // wait still wakes it.
    let mut watch = if follow {
        Some(facade.watch_job(job_id).map_err(cmd_error)?)
    } else {
        None
    };

    let job = loop {
        // State first, log second. A job observed in a terminal prefix has
        // already stopped writing, so the drain below cannot miss its last
        // bytes. The other order would read the log, watch the job go
        // terminal, and exit having dropped whatever landed in between.
        let job = facade.lookup_job(job_id).await.map_err(cmd_error)?;
        let terminal = job_state::is_terminal(&job.state);
        drain(&facade, job_id, &mut cursor, &mut buffered, json).await?;
        let Some(armed) = watch.take().filter(|_| !terminal) else {
            break job;
        };
        let woken = tokio::task::spawn_blocking(move || {
            let mut armed = armed;
            armed.next().map(|()| armed)
        })
        .await
        .map_err(|error| {
            CmdError::click(format!("the change watch on {job_id} stopped: {error}"))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        watch = Some(woken.map_err(|error| {
            CmdError::click(format!("the change watch on {job_id} failed: {error}"))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?);
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
        // An absent value is named in its cell: a blank STARTED, ENDED or
        // ERROR read the same for a job not yet started, one still running
        // and one that failed without recording why.
        let named = |value: Option<String>, absent: &str| match value {
            Some(value) => value,
            None => absent.to_string(),
        };
        let ended = job.completed_at.clone().or_else(|| job.failed_at.clone());
        let row = vec![
            job.job_id.clone(),
            job.state.clone(),
            cursor.to_string(),
            named(job.started_at.clone(), "(not started)"),
            named(ended, "(not ended)"),
            named(job.error.clone(), "(no error recorded)"),
        ];
        table::print(
            &["JOB", "STATE", "LOG BYTES", "STARTED", "ENDED", "ERROR"],
            std::slice::from_ref(&row),
        );
    }
    outcome(&job, terminal)
}
