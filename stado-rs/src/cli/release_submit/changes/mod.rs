//! Pushed work waiting for a later shared build. Handoff starts no build.
mod source;
mod status;
mod store;
mod ticket;

use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
pub(crate) use source::contains;
use std::path::PathBuf;
pub(crate) use store::{bind, queued_products};
use store::{download, ticket_id, ticket_paths};

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
    pub qualification_policy: &'static str,
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
                crate::cli::quality::Selection::Formatting,
            )
            .await
            .map_err(|refusal| {
                CmdError::refused(format!(
                    "{} is not handed off: {refusal}. Commit and push the repair, then hand \
                     off that commit",
                    change.source_commit
                ))
            })?;
            // Every request below is a job-store request: an object API
            // request or a local-store lock, and each says what it waits on
            // and where before it waits (`crate::wait`).
            let store = JobStorage::new().await.map_err(CmdError::from)?;
            let path = format!("{PREFIX}{}.json", change.id);
            let encoded = serde_json::to_string(&change)?;
            let created = store
                .create_text_if_absent(&path, &encoded)
                .await
                .map_err(CmdError::from)?;
            let saved = if created {
                change
            } else {
                let text = store
                    .download_text(&path)
                    .await
                    .map_err(CmdError::from)?
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
            let store = JobStorage::new().await.map_err(CmdError::from)?;
            // A ticket's id is its object name, so the wanted set comes from
            // the listing alone; every ticket a build batch froze arrives with
            // that batch, and only the rest are downloaded one by one.
            let listed_paths = ticket_paths(&store).await?;
            let paths: Vec<String> = listed_paths
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
            let mut statuses = Vec::with_capacity(listed.len());
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
