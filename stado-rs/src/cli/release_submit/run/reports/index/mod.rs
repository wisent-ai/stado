//! Release runs indexed by product.
//!
//! A run object's path names only its id
//! (`runs/release-pipeline/<id>/run.json`), so a question about one
//! product's runs read run objects newest first until it had enough of that
//! product's: with one old run of a product, every run object ever written,
//! one object request each, which is how `stado release status tama` ran for
//! most of an hour. Every save of a run also keeps an empty marker under
//! `runs/release-pipeline-by-product/<product>/<id>` ([`marker_path`]), and a
//! question about one product lists that prefix instead once the index is
//! complete. The walk that already reads every run body for `release newest`
//! ([`super::recorded_runs`]) writes the marker of every run it read and then
//! the completion marker, so runs saved before markers existed are indexed by
//! that one walk. Until it has run, a product question walks every run object
//! and says on stderr why.

use crate::cli::CmdError;
use crate::queue::storage::JobStorage;

use super::{RUN_STATE_LEAF, RUN_STATE_PREFIX};

/// Where each run's product marker lives.
const PRODUCT_INDEX_PREFIX: &str = "runs/release-pipeline-by-product/";
/// Present once every run object written before markers existed is indexed.
const COMPLETE: &str = "runs/release-pipeline-by-product.complete";

/// The marker that files run `id` under `product`.
pub(crate) fn marker_path(product: &str, id: &str) -> String {
    format!("{PRODUCT_INDEX_PREFIX}{product}/{id}")
}

/// Whether every run object is indexed by product.
async fn complete(store: &JobStorage) -> Result<bool, CmdError> {
    Ok(store
        .download_text(COMPLETE)
        .await
        .map_err(CmdError::from)?
        .is_some())
}

/// File every `(product, run id)` of a walk that read every run object, then
/// record the index complete; nothing is written once it is.
pub(super) async fn mark_complete(
    store: &JobStorage,
    runs: &[(String, String)],
) -> Result<(), CmdError> {
    if complete(store).await? {
        return Ok(());
    }
    for (product, id) in runs {
        store
            .upload_text(&marker_path(product, id), "")
            .await
            .map_err(CmdError::from)?;
    }
    store
        .upload_text(COMPLETE, "")
        .await
        .map_err(CmdError::from)
}

/// The run object paths a listing reads, newest first, narrowed to the runs
/// whose id starts with `run` when one is named: for one `product`, its
/// indexed runs once the index is complete; otherwise every run object.
pub(crate) async fn run_paths(
    store: &JobStorage,
    product: Option<&str>,
    run: Option<&str>,
) -> Result<Vec<String>, CmdError> {
    if let Some(product) = product {
        if complete(store).await? {
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
