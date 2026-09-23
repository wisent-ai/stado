//! Reports read back from the run objects a submission maintains: what `stado
//! release status` prints and what the operator console serves.

use futures::StreamExt;
use serde_json::Value;

use crate::cli::CmdError;
use crate::queue::storage::JobStorage;

/// One run object as raw JSON, or nothing when it is absent or unreadable.
pub(super) async fn load_run_value(
    store: &JobStorage,
    path: &str,
) -> Result<Option<Value>, CmdError> {
    let Some(text) = store
        .download_text(path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    else {
        return Ok(None);
    };
    Ok(serde_json::from_str::<Value>(&text).ok())
}

/// Where `run_state_path` puts every run object, and its leaf.
pub(super) const RUN_STATE_PREFIX: &str = "runs/release-pipeline/";
pub(super) const RUN_STATE_LEAF: &str = "/run.json";
/// How many run objects a listing reads before it stops looking: a product
/// or version filter is answered from the body of each run, one request per
/// run, and the whole history is not a bounded question. A run named by id
/// is picked from the listing before any read, so this never cuts it off.
pub(crate) const VERSION_SCAN_WINDOW: usize = 120;

/// One run recorded for a `(product, version)`: its id, its state word and
/// the commit it was cut from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordedRun {
    pub run_id: String,
    pub state: Option<crate::release_pipeline::ReleaseRunState>,
    pub source_commit: String,
}

/// Every run recorded for each `(product, version)`, newest first, read in one
/// walk of the newest `limit` run objects.
///
/// `matching_runs` answers one product at a time and joins every platform to
/// its queue job, which is what `release status` needs and what a whole
/// workspace cannot afford: `release newest` asks the same question of forty
/// checkouts at once, and asking it product by product cost one listing plus
/// up to a hundred and twenty body reads each — twenty-four minutes for a
/// plan that submits nothing. This reads each run body once and answers
/// for every product from that one pass.
pub(crate) async fn recorded_runs(
    limit: usize,
) -> Result<std::collections::BTreeMap<(String, String), Vec<RecordedRun>>, CmdError> {
    let store = JobStorage::new()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut blobs = store
        .list_blobs_with_meta(RUN_STATE_PREFIX)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .into_iter()
        .filter(|blob| blob.name.ends_with(RUN_STATE_LEAF))
        .collect::<Vec<_>>();
    blobs.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
    blobs.truncate(limit);
    let bodies = futures::stream::iter(blobs.into_iter().map(|blob| {
        let store = &store;
        async move { load_run_value(store, &blob.name).await }
    }))
    .buffered(8)
    .collect::<Vec<_>>()
    .await;
    let mut recorded = std::collections::BTreeMap::<_, Vec<RecordedRun>>::new();
    for body in bodies {
        let Some(run) = body? else { continue };
        let field = |key: &str| run[key].as_str().unwrap_or_default().to_string();
        let (product, version) = (field("product"), field("version"));
        if product.is_empty() || version.is_empty() {
            continue;
        }
        recorded
            .entry((product, version))
            .or_default()
            .push(RecordedRun {
                run_id: field("run_id"),
                state: crate::release_pipeline::ReleaseRunState::named(&field("state")),
                source_commit: field("source_commit"),
            });
    }
    Ok(recorded)
}

mod jobs;
mod listing;

pub(crate) use listing::{matching_runs, recent_runs, RunFilter};
