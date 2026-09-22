//! Pushed work waiting for a later shared build. Handoff starts no build.
mod source;
mod status;

use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
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
                    .ok_or_else(|| CmdError::click("pending change disappeared after admission"))?;
                let saved: Change = serde_json::from_str(&text)?;
                if saved.id != change.id
                    || saved.repository != change.repository
                    || saved.source_commit != change.source_commit
                    || saved.task_id != change.task_id
                    || saved.session_id != change.session_id
                    || saved.product != change.product
                {
                    return Err(CmdError::click("pending change identity mismatch"));
                }
                saved
            };
            let receipt = status::for_change(saved, &status::observations(&store).await?);
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
        ChangesCommand::List { task, json } => {
            let store = JobStorage::new().await.map_err(failure)?;
            let mut statuses = Vec::new();
            let observations = status::observations(&store).await?;
            for change in entries(&store).await? {
                if task.as_ref().is_some_and(|task| task != &change.task_id) {
                    continue;
                }
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

pub(super) async fn entries(store: &JobStorage) -> Result<Vec<Change>, CmdError> {
    let mut entries = Vec::new();
    for path in store.list_paths(PREFIX, 0).await.map_err(failure)? {
        if !path.ends_with(".json") {
            continue;
        }
        let text = store
            .download_text(&path)
            .await
            .map_err(failure)?
            .ok_or_else(|| CmdError::click(format!("pending change missing: {path}")))?;
        entries.push(serde_json::from_str(&text)?);
    }
    Ok(entries)
}

/// Freeze only tickets whose commits the selected build actually contains.
/// Called before queueing the build; later submissions cannot join its batch.
pub(crate) async fn bind(
    root: &std::path::Path,
    commit: &str,
    run_id: &str,
    product: &str,
) -> Result<(), CmdError> {
    let store = JobStorage::new().await.map_err(failure)?;
    let path = format!("runs/build/{run_id}/changes.json");
    if store.download_text(&path).await.map_err(failure)?.is_some() {
        return Ok(());
    }
    let candidates: Vec<_> = entries(&store)
        .await?
        .into_iter()
        .filter(|change| change.product == product)
        .collect();
    let repository = if candidates.is_empty() {
        String::new()
    } else {
        source::repository(root)?
    };
    let mut covered = Vec::new();
    let observations = status::observations(&store).await?;
    for change in candidates {
        if change.product == product
            && change.repository == repository
            && source::contains(root, &change.source_commit, commit)?
        {
            let state = status::for_change(change.clone(), &observations);
            if state.state != "passed" {
                covered.push(change);
            }
        }
    }
    covered.sort_by(|a, b| a.id.cmp(&b.id));
    // A racing coordinator may freeze first. Its immutable set is authoritative.
    store
        .create_text_if_absent(&path, &serde_json::to_string(&covered)?)
        .await
        .map_err(failure)?;
    Ok(())
}
