//! The handed-off tickets in the fleet store: listed, read, the products
//! still queued, and the tickets a build binds to itself.

use futures::StreamExt;

use crate::cli::CmdError;
use crate::queue::storage::JobStorage;

use super::{source, status, Change, PREFIX};

/// Every ticket object under [`PREFIX`].
pub(super) async fn ticket_paths(store: &JobStorage) -> Result<Vec<String>, CmdError> {
    Ok(store
        .list_paths(PREFIX, 0)
        .await
        .map_err(CmdError::from)?
        .into_iter()
        .filter(|path| path.ends_with(".json"))
        .collect())
}

/// The change id a ticket object is named after (`<PREFIX><id>.json`).
pub(super) fn ticket_id(path: &str) -> Option<&str> {
    path.strip_prefix(PREFIX)?.strip_suffix(".json")
}

/// Download and parse the named tickets, fanned out like every other bulk
/// object read.
pub(super) async fn download(
    store: &JobStorage,
    paths: &[String],
) -> Result<Vec<Change>, CmdError> {
    let texts = futures::stream::iter(paths)
        .map(|path| async move { (path, store.download_text(path).await) })
        .buffered(crate::queue::copy::default_concurrency())
        .collect::<Vec<_>>()
        .await;
    let mut entries = Vec::with_capacity(texts.len());
    for (path, text) in texts {
        let text = text.map_err(CmdError::from)?.ok_or_else(|| {
            CmdError::click(format!("pending change missing: {path}"))
                .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
        entries.push(serde_json::from_str(&text)?);
    }
    Ok(entries)
}

async fn entries(store: &JobStorage) -> Result<Vec<Change>, CmdError> {
    download(store, &ticket_paths(store).await?).await
}

/// The products whose handed-off work no build has taken yet: every ticket
/// still `queued`. `stado build newest --queued` builds exactly these, which
/// is the daily batch the handoff promises.
pub(crate) async fn queued_products() -> Result<std::collections::BTreeSet<String>, CmdError> {
    let store = JobStorage::new().await.map_err(CmdError::from)?;
    let paths = ticket_paths(&store).await?;
    let wanted: std::collections::HashSet<String> = paths
        .iter()
        .filter_map(|path| ticket_id(path))
        .map(str::to_owned)
        .collect();
    let observed = status::observations(&store, &wanted).await?;
    // A ticket some build observed is not queued, so only the others are read.
    let unobserved: Vec<String> = paths
        .into_iter()
        .filter(|path| ticket_id(path).is_none_or(|id| !observed.by_change.contains_key(id)))
        .collect();
    Ok(download(&store, &unobserved)
        .await?
        .into_iter()
        .map(|change| change.product)
        .collect())
}

/// Freeze only tickets whose commits the selected build actually contains.
/// Called before queueing the build.
///
/// The first freeze is the build's batch (`changes.json`) and never changes.
/// A ticket handed off after it, whose commit that same build contains, is
/// bound in an additional batch (`changes-<digest>.json`) beside it: a build
/// is identified by its commit, so a later build of that commit is the same
/// build, and a handoff made after the build froze would otherwise stay
/// `queued` for good while every `build newest --queued` picked the product
/// again and answered with that same passed build.
pub(crate) async fn bind(
    root: &std::path::Path,
    commit: &str,
    run_id: &str,
    product: &str,
) -> Result<(), CmdError> {
    let store = JobStorage::new().await.map_err(CmdError::from)?;
    let path = format!("runs/build/{run_id}/changes.json");
    let frozen = store
        .download_text(&path)
        .await
        .map_err(CmdError::from)?
        .is_some();
    let candidates: Vec<_> = entries(&store)
        .await?
        .into_iter()
        .filter(|change| change.product == product)
        .collect();
    if candidates.is_empty() && frozen {
        return Ok(());
    }
    let repository = if candidates.is_empty() {
        String::new()
    } else {
        source::repository(root)?
    };
    let mut covered = Vec::new();
    let wanted = candidates.iter().map(|change| change.id.clone()).collect();
    let observations = status::observations(&store, &wanted).await?.by_change;
    for change in candidates {
        if change.product != product || change.repository != repository {
            continue;
        }
        if !source::holds(root, &change.source_commit)? {
            eprintln!(
                "release of {product}: pending change {} ({}) names commit {}, which {} no longer holds, so this release cannot cover it; record that work's repair again on a commit main carries",
                change.id, change.task_id, change.source_commit, repository
            );
            continue;
        }
        if source::contains(root, &change.source_commit, commit)? {
            let unbound = !observations.contains_key(&change.id);
            let state = status::for_change(change.clone(), &observations);
            if (!frozen && state.state != "passed") || (frozen && unbound) {
                covered.push(change);
            }
        }
    }
    covered.sort_by(|a, b| a.id.cmp(&b.id));
    let target = if frozen {
        if covered.is_empty() {
            return Ok(());
        }
        let ids: Vec<&str> = covered.iter().map(|change| change.id.as_str()).collect();
        format!(
            "runs/build/{run_id}/changes-{}.json",
            crate::release_control::sha256_bytes(ids.join("\n").as_bytes())
        )
    } else {
        path
    };
    // A racing coordinator may freeze first. Its immutable set is authoritative.
    store
        .create_text_if_absent(&target, &serde_json::to_string(&covered)?)
        .await
        .map_err(CmdError::from)?;
    Ok(())
}
