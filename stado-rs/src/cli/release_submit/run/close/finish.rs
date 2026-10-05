//! The pass that turns a queued submission into a published release without
//! anyone waiting in a terminal.
//!
//! `stado release submit` ends when the platform builds are queued. What used
//! to follow inside that same client process - waiting for every builder,
//! signing, publishing, delivering - is this pass, run by the control host's
//! release agent on every tick. It picks only runs whose builds have reached
//! a terminal queue state, so it never blocks the agent on a builder, and it
//! walks each one through the same `finish_run` that `stado release resume`
//! uses by hand.

use crate::cli::release_submit::builds::jobs::terminal::terminal;
use crate::cli::release_submit::run::resume::finish_run;
use crate::cli::release_submit::run::source::UNFINISHED_RUN_PREFIX;
use crate::cli::release_submit::run::state::load;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::ReleaseRunState;

/// Runs whose builds are all terminal and that are not themselves terminal:
/// the ones one tick may finish. Every unfinished run is considered, read
/// from the index `state::save` keeps, however long ago it was submitted. A
/// run last written before that index existed has no marker until its next
/// write; `stado release resume` reaches it by hand. Only the control host
/// finishes anything - it owns the object store and the signing grant - and
/// it says so once when it is not.
pub async fn finish_ready_runs() -> Result<Vec<String>, String> {
    if !crate::config::stado_api_url().is_empty() {
        return Ok(Vec::new());
    }
    let store = JobStorage::new().await.map_err(|error| error.to_string())?;
    let mut finished = Vec::new();
    let markers = store
        .list_blobs_with_meta(UNFINISHED_RUN_PREFIX)
        .await
        .map_err(|error| error.to_string())?;
    for marker in markers {
        let id = &marker.name[UNFINISHED_RUN_PREFIX.len().min(marker.name.len())..];
        if id.is_empty() {
            continue;
        }
        let Some(stored) = load(id).await.map_err(|error| error.to_string())? else {
            eprintln!(
                "stado release agent run={id} is indexed as unfinished but has no run object"
            );
            continue;
        };
        if !matches!(
            stored.state,
            ReleaseRunState::Waiting | ReleaseRunState::Publishing | ReleaseRunState::Delivering
        ) {
            continue;
        }
        let run = serde_json::to_value(&stored).map_err(|error| error.to_string())?;
        if !builds_terminal(&store, &run).await? {
            continue;
        }
        match finish_run(id, false).await {
            Ok(()) => finished.push(id.to_string()),
            // The failure is already persisted on the run object by
            // `finish_run`; the agent's log names it once and moves on.
            Err(error) => eprintln!("stado release agent run={id} finish failed: {error}"),
        }
    }
    Ok(finished)
}

/// Every platform this run submitted has a job that ended: a record the
/// queue calls terminal, or — once the run reaper has retired that record on
/// its own cadence — the receipt the worker wrote, which is what publishing
/// verifies. A platform still failed from an earlier attempt has nothing to
/// wait for. A job still queued or running answers an error from
/// [`terminal`], which here means "not yet".
async fn builds_terminal(store: &JobStorage, run: &serde_json::Value) -> Result<bool, String> {
    let Some(platforms) = run["platforms"].as_object() else {
        return Ok(false);
    };
    for platform in platforms.values() {
        if platform["state"].as_str() == Some("failed") {
            continue;
        }
        let Some(job_id) = platform["job_id"].as_str() else {
            return Ok(false);
        };
        if terminal(store, job_id).await.is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}
