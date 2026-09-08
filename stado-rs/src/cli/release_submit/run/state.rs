//! The durable run object: how a submission records its own progress, its
//! failures, and which run is the newest one a delivery may belong to.

use chrono::Utc;

use crate::cli::release_submit::run::source::run_state_path;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{ReleaseRun, ReleaseRunState};

pub(crate) async fn save(run: &mut ReleaseRun) -> Result<(), CmdError> {
    run.updated_at = Utc::now().to_rfc3339();
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let path = run_state_path(&run.run_id);
    let content = serde_json::to_string(run)?;
    if let Some(current) = store
        .read_text_versioned(&path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        store
            .compare_and_swap_text(&path, &current.version, &content)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        return Ok(());
    }
    if store
        .create_text_if_absent(&path, &content)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "release run state appeared concurrently: {}",
            run.run_id
        )))
    }
}
pub(crate) async fn persist_failure(run: &mut ReleaseRun, error: CmdError) -> CmdError {
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

/// The newest submitted run is the delivery fence. Delivery workers already
/// carry the queue storage identity needed to read run state, while fleet
/// targets deliberately do not carry a product publisher credential.
pub(crate) async fn latest_submitted_run(product: &str) -> Result<Option<ReleaseRun>, CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut latest: Option<(chrono::DateTime<chrono::FixedOffset>, ReleaseRun)> = None;
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
        if run.product != product {
            continue;
        }
        let created_at =
            chrono::DateTime::parse_from_rfc3339(&run.created_at).map_err(|error| {
                CmdError::click(format!(
                    "release run {} has invalid created_at: {error}",
                    run.run_id
                ))
            })?;
        let replace = match &latest {
            None => true,
            Some((latest_at, latest_run)) => {
                created_at > *latest_at
                    || (created_at == *latest_at
                        && run.run_id.as_str() > latest_run.run_id.as_str())
            }
        };
        if replace {
            latest = Some((created_at, run));
        }
    }
    Ok(latest.map(|(_, run)| run))
}
