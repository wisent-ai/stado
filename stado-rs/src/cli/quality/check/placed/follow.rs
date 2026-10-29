//! Following a placed check to its end, from wherever it was placed from.
//!
//! The queue store wakes a reader on change only where the store is a
//! directory on the reader's own disk. On that host the job is held with the
//! store's own change watch; from any other machine the hold runs on the
//! store's authority host (`stado job watch --follow` there), whose answer
//! is the job's log as it ended. Nothing here waits on a timer.

use serde_json::Value;

use crate::cli::CmdError;
use crate::machine::MachineFacade;

/// How the job ended: its terminal state and error, and the log the hold
/// returned when it ran on the authority host.
pub(super) struct Ending {
    pub state: String,
    pub error: Option<String>,
    pub log: Option<String>,
}

pub(super) async fn follow(job_id: &str) -> Result<Ending, CmdError> {
    let facade = MachineFacade::new().await.map_err(|error| {
        CmdError::click(format!(
            "the queue cannot be opened to follow {job_id}: {error}"
        ))
    })?;
    let log = if crate::targets::store_is_local_filesystem() {
        facade
            .status_until_terminal(job_id)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "job {job_id} could not be held to its end: {error}"
                ))
            })?;
        None
    } else {
        Some(on_authority(job_id).await?)
    };
    let job = facade.lookup_job(job_id).await.map_err(|error| {
        CmdError::click(format!(
            "job {job_id} could not be read after it ended: {error}"
        ))
    })?;
    Ok(Ending {
        state: job.state,
        error: job.error,
        log,
    })
}

/// Hold the job on the host whose store is local: the registry's directory
/// authority, which serves the store this machine reads.
async fn on_authority(job_id: &str) -> Result<String, CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let authority = document["service_directory"]["authority"]["target"]
        .as_str()
        .ok_or_else(|| {
            CmdError::declaration(
                "the registry names no service_directory.authority.target, so no host holds the \
                 store a placed check could be followed on"
                    .to_string(),
            )
        })?
        .to_string();
    let registry = crate::targets::load_registry_from_value(&document)
        .map_err(|error| CmdError::declaration(format!("the registry does not load: {error}")))?;
    let target = crate::deploy::host_channel::resolve_target(&registry, &authority)?;
    let script = format!(
        "set -eu\n\"$HOME/.stado/bin/stado\" job watch {} --follow\n",
        crate::deploy::shlex_quote(job_id)
    );
    let output = crate::deploy::host_channel::run_script(
        target,
        &script,
        &crate::deploy::production_runner(),
    )
    .await?;
    if !output.succeeded() {
        return Err(CmdError::click(format!(
            "`stado job watch {job_id} --follow` on {authority}, which holds the fleet store, did \
             not follow it to its end: {}",
            output.detail().trim()
        )));
    }
    Ok(output.stdout)
}

/// The ending as the placement record carries it.
pub(super) fn to_json(ending: &Ending) -> Value {
    serde_json::json!({"state": ending.state, "error": ending.error})
}
