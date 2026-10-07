//! Pushed work waiting for a later shared build. Handoff starts no build.
mod source;
mod status;
mod ticket;

use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use clap::{Args, Subcommand};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
pub(crate) use source::contains;
use std::path::PathBuf;

const PREFIX: &str = "runs/release-changes/";

#[derive(Args)]
pub struct ChangesArgs {
    #[command(subcommand)]
    command: ChangesCommand,
}

#[derive(Subcommand)]
enum ChangesCommand {
    /// Record a pushed commit and its task without compiling or spending build budget.
    Submit {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        commit: String,
        #[arg(long)]
        task: String,
        #[arg(long)]
        session: String,
        #[arg(long)]
        json: bool,
    },
    /// Read queued work and the actual build qualification covering it.
    List {
        #[arg(long)]
        task: Option<String>,
        /// Only these change ids (repeatable). A caller that follows a few
        /// tickets reads those, not every ticket the fleet ever queued.
        #[arg(long = "id")]
        ids: Vec<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub id: String,
    pub product: String,
    pub task_id: String,
    pub session_id: String,
    pub source_commit: String,
    pub repository: String,
    pub submitted_at: String,
}

#[derive(Serialize)]
pub struct ChangeStatus {
    #[serde(flatten)]
    pub change: Change,
    pub state: String,
    pub run_id: Option<String>,
    pub failure: Option<String>,
    pub evidence: Vec<crate::release_pipeline::BuildReceipt>,
}

pub async fn dispatch(args: &ChangesArgs) -> Result<(), CmdError> {
    match &args.command {
        ChangesCommand::Submit {
            source,
            commit,
            task,
            session,
            json,
        } => {
            let change = source::prepare(source, commit, task, session)?;
            // A commit that fails its product's formatting or lock gate fails
            // the batch build that takes it, and with it every other session's
            // work in that batch; refusing the handoff names it to the session
            // that pushed it, while that session can still repair it.
            crate::cli::quality::check_revision(
                source,
                &change.source_commit,
                crate::cli::quality::Report::Stderr,
            )
            .map_err(|refusal| {
                CmdError::refused(format!(
                    "{} is not handed off: {refusal}. Commit and push the repair, then hand \
                     off that commit",
                    change.source_commit
                ))
            })?;
            let store = JobStorage::new().await.map_err(failure)?;
            let path = format!("{PREFIX}{}.json", change.id);
            let encoded = serde_json::to_string(&change)?;
            let created = store
                .create_text_if_absent(&path, &encoded)
                .await
                .map_err(failure)?;
            let saved = if created {
                change
            } else {
                let text = store
                    .download_text(&path)
                    .await
                    .map_err(failure)?
                    .ok_or_else(|| {
                        CmdError::click("pending change disappeared after admission")
                            .stating(crate::primitives::failure::FailureCode::InfraDown)
                    })?;
                let saved: Change = serde_json::from_str(&text)?;
                if let Some(differing) = ticket::disagreement(&saved, &change) {
                    return Err(CmdError::refused(format!(
                        "pending change {} at {path} disagrees with this handoff: {differing}",
                        change.id
                    )));
                }
                saved
            };
            // A ticket written just now is in no frozen batch: a build freezes
            // the tickets that exist when it is queued, and the next `list`
            // reads the batch that covers this one. Reading every build ever
            // made to answer that is what made a handoff take minutes.
            let receipt = if created {
                status::for_change(saved, &Default::default())
            } else {
                let wanted = std::iter::once(saved.id.clone()).collect();
                status::for_change(
                    saved,
                    &status::observations(&store, &wanted).await?.by_change,
                )
            };
            if *json {
                println!("{}", serde_json::to_string(&receipt)?);
            } else {
                println!(
                    "{}: {} {} for {}; no build started",
                    receipt.change.id,
                    receipt.state,
                    receipt.change.source_commit,
                    receipt.change.task_id
                );
            }
            Ok(())
        }
        ChangesCommand::List { task, ids, json } => {
            let store = JobStorage::new().await.map_err(failure)?;
            let mut statuses = Vec::new();
            // A ticket's id is its object name, so the wanted set comes from
            // the listing alone; every ticket a build batch froze arrives with
            // that batch, and only the rest are downloaded one by one.
            let paths: Vec<String> = ticket_paths(&store)
                .await?
                .into_iter()
                .filter(|path| {
                    ids.is_empty()
                        || ticket_id(path).is_some_and(|id| ids.iter().any(|wanted| wanted == id))
                })
                .collect();
            let wanted: std::collections::HashSet<String> = paths
                .iter()
                .filter_map(|path| ticket_id(path))
                .map(str::to_owned)
                .collect();
            let mut observed = status::observations(&store, &wanted).await?;
            let unfrozen: Vec<String> = paths
                .into_iter()
                .filter(|path| ticket_id(path).is_none_or(|id| !observed.frozen.contains_key(id)))
                .collect();
            let mut listed: Vec<Change> = download(&store, &unfrozen).await?;
            listed.extend(wanted.iter().filter_map(|id| observed.frozen.remove(id)));
            listed.retain(|change| task.as_ref().is_none_or(|task| task == &change.task_id));
            listed.sort_by(|left, right| left.id.cmp(&right.id));
            let observations = observed.by_change;
            for change in listed {
                statuses.push(status::for_change(change, &observations));
            }
            if *json {
                println!("{}", serde_json::to_string(&statuses)?);
            } else {
                for row in statuses {
                    println!(
                        "{} {} {} {} {}",
                        row.change.id,
                        row.change.product,
                        row.change.task_id,
                        row.state,
                        row.failure.unwrap_or_default()
                    );
                }
            }
            Ok(())
        }
    }
}

pub(super) fn failure(error: impl std::fmt::Display) -> CmdError {
    CmdError::click(error.to_string())
}

/// Every ticket object under [`PREFIX`].
async fn ticket_paths(store: &JobStorage) -> Result<Vec<String>, CmdError> {
    Ok(store
        .list_paths(PREFIX, 0)
        .await
        .map_err(failure)?
        .into_iter()
        .filter(|path| path.ends_with(".json"))
        .collect())
}

/// The change id a ticket object is named after (`<PREFIX><id>.json`).
fn ticket_id(path: &str) -> Option<&str> {
    path.strip_prefix(PREFIX)?.strip_suffix(".json")
}

/// Download and parse the named tickets, fanned out like every other bulk
/// object read.
async fn download(store: &JobStorage, paths: &[String]) -> Result<Vec<Change>, CmdError> {
    let texts = futures::stream::iter(paths)
        .map(|path| async move { (path, store.download_text(path).await) })
        .buffered(crate::queue::copy::default_concurrency())
        .collect::<Vec<_>>()
        .await;
    let mut entries = Vec::with_capacity(texts.len());
    for (path, text) in texts {
        let text = text.map_err(failure)?.ok_or_else(|| {
            CmdError::click(format!("pending change missing: {path}"))
                .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
        entries.push(serde_json::from_str(&text)?);
    }
    Ok(entries)
}

pub(super) async fn entries(store: &JobStorage) -> Result<Vec<Change>, CmdError> {
    download(store, &ticket_paths(store).await?).await
}

/// The products whose handed-off work no build has taken yet: every ticket
/// still `queued`. `stado build newest --queued` builds exactly these, which
/// is the daily batch the handoff promises.
pub(crate) async fn queued_products() -> Result<std::collections::BTreeSet<String>, CmdError> {
    let store = JobStorage::new().await.map_err(failure)?;
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
    let store = JobStorage::new().await.map_err(failure)?;
    let path = format!("runs/build/{run_id}/changes.json");
    let frozen = store.download_text(&path).await.map_err(failure)?.is_some();
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
        .map_err(failure)?;
    Ok(())
}
