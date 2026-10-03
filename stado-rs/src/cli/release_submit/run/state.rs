//! The durable run objects: how a build and a submission record their own
//! progress and failures, and which run is the newest one a delivery may
//! belong to.

use chrono::Utc;

use crate::cli::release_submit::run::source::{build_state_path, run_state_path};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{
    BuildRun, BuildRunState, PlatformRunState, ReleaseRun, ReleaseRunState,
};

/// Write one run object over its current version, or create it. The
/// compare-and-swap is the guard against two coordinators writing one run.
async fn write_state(path: &str, content: &str, what: &str, id: &str) -> Result<(), CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if let Some(current) = store
        .read_text_versioned(path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        store
            .compare_and_swap_text(path, &current.version, content)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        return Ok(());
    }
    if store
        .create_text_if_absent(path, content)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "{what} state appeared concurrently: {id}"
        )))
    }
}

pub(crate) async fn save_build(build: &mut BuildRun) -> Result<(), CmdError> {
    build.updated_at = Utc::now().to_rfc3339();
    let content = serde_json::to_string(build)?;
    write_state(
        &build_state_path(&build.build_id),
        &content,
        "build",
        &build.build_id,
    )
    .await
}

pub(crate) async fn persist_build_failure(build: &mut BuildRun, error: CmdError) -> CmdError {
    build.state = BuildRunState::Failed;
    build.failure = Some(error.to_string());
    if let Err(save_error) = save_build(build).await {
        return CmdError {
            message: Some(format!(
                "{error}; failed to persist build failure: {save_error}"
            )),
            code: error.code,
            ..CmdError::default()
        };
    }
    error
}

pub(crate) async fn load_build(id: &str) -> Result<Option<BuildRun>, CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    store
        .download_text(&build_state_path(id))
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .map(|content| serde_json::from_str(&content).map_err(CmdError::from))
        .transpose()
}

pub(crate) async fn save(run: &mut ReleaseRun) -> Result<(), CmdError> {
    run.updated_at = Utc::now().to_rfc3339();
    let content = serde_json::to_string(run)?;
    write_state(
        &run_state_path(&run.run_id),
        &content,
        "release run",
        &run.run_id,
    )
    .await
}
/// Record this pass's failure on the run — unless another pass wrote the run
/// since this one last read or saved it. Two finishers (the control host's
/// release agent and `stado release resume`) walk the same run; the one that
/// lost the write race, or that judged a delivery from a stale copy, used to
/// overwrite the winner's progress with `failed`, and the delivery workers
/// then refused their own jobs ("the run is Failed, not delivering").
pub(crate) async fn persist_failure(run: &mut ReleaseRun, error: CmdError) -> CmdError {
    match load(&run.run_id).await {
        Ok(Some(stored)) if stored.updated_at != run.updated_at => {
            return CmdError {
                message: Some(format!(
                    "{error}; another pass advanced release run {} since this one read it \
                     (state {:?}), so this pass's failure was not recorded over it",
                    run.run_id, stored.state
                )),
                code: error.code,
                ..CmdError::default()
            };
        }
        Ok(_) => {}
        Err(load_error) => {
            return CmdError {
                message: Some(format!(
                    "{error}; the release run could not be read to record the failure: \
                     {load_error}"
                )),
                code: error.code,
                ..CmdError::default()
            };
        }
    }
    run.state = ReleaseRunState::Failed;
    run.failure = Some(error.to_string());
    if let Err(save_error) = save(run).await {
        return CmdError {
            message: Some(format!(
                "{}; failed to persist release failure: {}",
                error, save_error
            )),
            code: error.code,
            ..CmdError::default()
        };
    }
    error
}
pub(crate) async fn load(id: &str) -> Result<Option<ReleaseRun>, CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    store
        .download_text(&run_state_path(id))
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .map(|content| serde_json::from_str(&content).map_err(CmdError::from))
        .transpose()
}

/// The run that replaces every other published run of the product is the
/// delivery fence: a higher version, or the same version submitted later
/// ([`super::close::supersede::replaces`]). Ordered by submission time alone,
/// a release of an older version published after a newer one made every
/// delivery of the newer version refuse itself as stale.
/// Delivery workers already carry the queue storage identity needed to read
/// run state, while fleet targets deliberately do not carry a product
/// publisher credential.
///
/// A run that has published nothing fences nothing: there is no artifact of
/// it a host could receive, so no older delivery is stale relative to it.
/// With the newest run of any state as the fence, runs of an older version
/// that failed before a builder was found, created moments after a newer run,
/// make every delivery of the newer version refuse itself as superseded, on
/// every host, by runs that will never deliver a byte; an abandoned run whose
/// builds both failed does the same to the delivery before it. A run starts
/// fencing the moment it publishes a platform, which is when its deliveries
/// can begin.
pub(crate) async fn latest_submitted_run(product: &str) -> Result<Option<ReleaseRun>, CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut latest: Option<ReleaseRun> = None;
    for path in store
        .list_paths("runs/release-pipeline/", 0)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        if !path.ends_with("/run.json") {
            continue;
        }
        let Some(text) = store
            .download_text(&path)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
        else {
            continue;
        };
        let run: ReleaseRun = serde_json::from_str(&text)
            .map_err(|error| CmdError::click(format!("invalid release run {path}: {error}")))?;
        if run.product != product
            || !run
                .platforms
                .values()
                .any(|platform| platform.state == PlatformRunState::Published)
        {
            continue;
        }
        chrono::DateTime::parse_from_rfc3339(&run.created_at).map_err(|error| {
            CmdError::click(format!(
                "release run {} has invalid created_at: {error}",
                run.run_id
            ))
        })?;
        let replace = match &latest {
            None => true,
            Some(latest_run) => {
                super::close::supersede::replaces(&run, latest_run)
                    || (run.created_at == latest_run.created_at
                        && run.version == latest_run.version
                        && run.run_id.as_str() > latest_run.run_id.as_str())
            }
        };
        if replace {
            latest = Some(run);
        }
    }
    Ok(latest)
}
