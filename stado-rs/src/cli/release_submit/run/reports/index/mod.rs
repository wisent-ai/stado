//! Release runs indexed by product.
//!
//! A run object's path names only its id
//! (`runs/release-pipeline/<id>/run.json`), so without an index a question
//! about one product's runs reads run objects newest first until it has
//! enough of that product's: with one old run of a product, every run object
//! ever written, one object request each. Every save of a run also keeps an
//! empty marker under
//! `runs/release-pipeline-by-product/<product>/<id>` ([`marker_path`]), and a
//! question about one product lists that prefix instead once the index is
//! complete. The walk that already reads every run body for `release newest`
//! ([`super::recorded_runs`]) writes the marker of every run it read and then
//! the completion marker, so runs saved before markers existed are indexed by
//! that one walk. Until it has run, a product question walks every run object
//! and says on stderr why.
//!
//! The fleet's hosts upgrade one at a time, so a run saved by a Stado that
//! writes no marker can land after the index is complete. The completion
//! marker therefore holds the newest run object time it covers, and every
//! product question first lists the run objects (one listing) and files each
//! saved after that time by reading its body, then moves the covered time
//! forward: the index stays whole without trusting every writer. A
//! completion marker written before it held a time covers what was saved
//! before the marker itself was written.

use chrono::{DateTime, Utc};

use crate::cli::CmdError;
use crate::queue::storage::JobStorage;

use super::{load_run_value, RUN_STATE_LEAF, RUN_STATE_PREFIX};

/// Where each run's product marker lives.
const PRODUCT_INDEX_PREFIX: &str = "runs/release-pipeline-by-product/";
/// Present once every run object written before markers existed is indexed;
/// it holds the newest run object time the index covers.
const COMPLETE: &str = "runs/release-pipeline-by-product.complete";

/// The marker that files run `id` under `product`.
pub(crate) fn marker_path(product: &str, id: &str) -> String {
    format!("{PRODUCT_INDEX_PREFIX}{product}/{id}")
}

/// The run object time the index covers, none while it is not complete.
async fn covered(store: &JobStorage) -> Result<Option<DateTime<Utc>>, CmdError> {
    let Some(text) = store
        .download_text(COMPLETE)
        .await
        .map_err(CmdError::from)?
    else {
        return Ok(None);
    };
    if !text.trim().is_empty() {
        return DateTime::parse_from_rfc3339(text.trim())
            .map(|at| Some(at.with_timezone(&Utc)))
            .map_err(|error| {
                CmdError::refused(format!(
                    "the release run index marker {COMPLETE} holds {text:?}: {error}"
                ))
            });
    }
    // A marker written before it held a time: the walk that wrote it had
    // filed every run saved before it.
    let marker = store
        .list_blobs_with_meta(COMPLETE)
        .await
        .map_err(CmdError::from)?;
    marker
        .into_iter()
        .find(|blob| blob.name == COMPLETE)
        .and_then(|blob| blob.updated)
        .map(Some)
        .ok_or_else(|| {
            CmdError::refused(format!(
                "the release run index marker {COMPLETE} carries no time it was written"
            ))
        })
}

/// Record that every run object up to `newest` is filed.
async fn cover(store: &JobStorage, newest: Option<DateTime<Utc>>) -> Result<(), CmdError> {
    let text = match newest {
        Some(newest) => newest.to_rfc3339(),
        None => String::new(),
    };
    store
        .upload_text(COMPLETE, &text)
        .await
        .map_err(CmdError::from)
}

/// File every `(product, run id)` of a walk that read every run object up to
/// `newest`, then record the index complete; nothing is written once it is.
pub(super) async fn mark_complete(
    store: &JobStorage,
    runs: &[(String, String)],
    newest: Option<DateTime<Utc>>,
) -> Result<(), CmdError> {
    if covered(store).await?.is_some() {
        return Ok(());
    }
    for (product, id) in runs {
        store
            .upload_text(&marker_path(product, id), "")
            .await
            .map_err(CmdError::from)?;
    }
    cover(store, newest).await
}

/// File every run object saved after the index's covered time (a save by a
/// Stado that writes no marker), and move the covered time to the newest.
async fn catch_up(store: &JobStorage, since: DateTime<Utc>) -> Result<(), CmdError> {
    let later: Vec<_> = store
        .list_blobs_with_meta(RUN_STATE_PREFIX)
        .await
        .map_err(CmdError::from)?
        .into_iter()
        .filter(|blob| {
            blob.name.ends_with(RUN_STATE_LEAF) && blob.updated.is_some_and(|at| at > since)
        })
        .collect();
    let Some(newest) = later.iter().filter_map(|blob| blob.updated).max() else {
        return Ok(());
    };
    for blob in &later {
        let Some(run) = load_run_value(store, &blob.name).await? else {
            continue;
        };
        let (Some(product), Some(id)) = (run["product"].as_str(), run["run_id"].as_str()) else {
            return Err(CmdError::refused(format!(
                "release run object {} names no product or run_id, so it cannot be indexed: {run}",
                blob.name
            )));
        };
        store
            .upload_text(&marker_path(product, id), "")
            .await
            .map_err(CmdError::from)?;
    }
    cover(store, Some(newest)).await
}

/// The run object paths a listing reads, newest first, narrowed to the runs
/// whose id starts with `run` when one is named: for one `product`, its
/// indexed runs once the index is complete (after filing runs saved since
/// its covered time); otherwise every run object.
pub(crate) async fn run_paths(
    store: &JobStorage,
    product: Option<&str>,
    run: Option<&str>,
) -> Result<Vec<String>, CmdError> {
    if let Some(product) = product {
        if let Some(since) = covered(store).await? {
            catch_up(store, since).await?;
            let prefix = format!("{PRODUCT_INDEX_PREFIX}{product}/");
            let mut markers = store
                .list_blobs_with_meta(&prefix)
                .await
                .map_err(CmdError::from)?;
            markers.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
            return Ok(markers
                .into_iter()
                .filter_map(|blob| blob.name.strip_prefix(prefix.as_str()).map(str::to_owned))
                .filter(|id| run.is_none_or(|wanted| id.starts_with(wanted)))
                .map(|id| format!("{RUN_STATE_PREFIX}{id}{RUN_STATE_LEAF}"))
                .collect());
        }
        eprintln!(
            "[release runs] runs are not indexed by product yet, so every run object is read to find \
             {product}'s; the next `stado release newest` reads every run once and indexes them"
        );
    }
    let mut blobs = store
        .list_blobs_with_meta(RUN_STATE_PREFIX)
        .await
        .map_err(CmdError::from)?
        .into_iter()
        .filter(|blob| blob.name.ends_with(RUN_STATE_LEAF))
        // The run id is the path segment after the prefix, so a run named by
        // id is found from the listing alone.
        .filter(|blob| {
            run.is_none_or(|wanted| {
                blob.name
                    .strip_prefix(RUN_STATE_PREFIX)
                    .is_some_and(|rest| rest.starts_with(wanted))
            })
        })
        .collect::<Vec<_>>();
    blobs.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
    Ok(blobs.into_iter().map(|blob| blob.name).collect())
}
