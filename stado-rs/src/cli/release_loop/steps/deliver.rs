//! One pass of the delivery loop over a workspace's product checkouts.
//!
//! Each checkout is read the way `stado release newest` reads it — its
//! commit, the version that commit declares and what the recorded runs did
//! with it — and then by what Oko owes it: the open defects whose recorded
//! repair the commit carries. A commit carrying none owes no release. One
//! carrying some is released and the release recorded on its defects; a
//! commit already published only needs that record. A commit the loop
//! already delivered or saw fail is not taken again: its outcome stands in
//! the register and on its defects, and the next commit of the product is
//! the next attempt.

use std::path::Path;

use chrono::Utc;

use super::super::register::{self, Entry, State, Step};
use super::carried::{carried, note_failure, record_released};
use super::version::commit_next;
use crate::cli::release_newest::{plan, Standing};
use crate::cli::release_submit::{submit, ReleaseSubmitArgs, SubmitChannel};
use crate::cli::CmdError;

/// What one checkout owes, read from its plan.
enum Owed {
    /// Release `commit` under `version`, after committing `version` first
    /// when `bump` says the declared one is taken by another commit's run.
    Release {
        commit: String,
        version: String,
        bump: bool,
    },
    /// Published already: only the record in Oko is owed.
    Record { commit: String, version: String },
    /// A run is releasing `commit` now; the next pass reads its outcome.
    InFlight { commit: String, version: String },
    /// The plan could not be read, or the version cannot advance.
    Refused {
        commit: Option<String>,
        step: Step,
        error: String,
    },
    /// The product declares it does not release.
    Nothing,
}

fn owed(standing: Standing) -> Owed {
    match standing {
        Standing::Releasable {
            commit, version, ..
        } => Owed::Release {
            commit,
            version,
            bump: false,
        },
        Standing::VersionTaken {
            commit,
            version,
            next,
            unadvanceable,
            ..
        } => match (next, unadvanceable) {
            (Some(next), _) => Owed::Release {
                commit,
                version: next,
                bump: true,
            },
            (None, Some(why)) => Owed::Refused {
                commit: Some(commit),
                step: Step::Version,
                error: format!(
                    "{version} is taken by another commit's release and the versioning rule gives no next version: {why}"
                ),
            },
            (None, None) => Owed::Refused {
                commit: Some(commit),
                step: Step::Version,
                error: format!(
                    "{version} is taken by another commit's release and the plan named neither the next version nor why there is none"
                ),
            },
        },
        Standing::Published {
            commit, version, ..
        } => Owed::Record { commit, version },
        Standing::InFlight {
            commit, version, ..
        } => Owed::InFlight { commit, version },
        Standing::Unreadable { refusal } => Owed::Refused {
            commit: None,
            step: Step::Plan,
            error: refusal,
        },
        Standing::DeclaresNoReleases { .. } => Owed::Nothing,
    }
}

fn entry(product: &str, commit: Option<&str>, step: Step) -> Entry {
    let now = Utc::now();
    Entry {
        product: product.to_string(),
        commit: commit.map(str::to_string),
        state: State::Running,
        step,
        version: None,
        version_commit: None,
        carried: Vec::new(),
        recorded: Vec::new(),
        error: None,
        started_at: now,
        updated_at: now,
    }
}

/// Keep `entry` failed at its step with `error`, and note the failure on
/// every defect it carries.
async fn failed(mut entry: Entry, error: CmdError) -> Result<Entry, CmdError> {
    let error = error.to_string();
    if let Some(commit) = &entry.commit {
        note_failure(
            &entry.carried,
            &entry.product,
            commit,
            entry.step.name(),
            &error,
        )
        .await;
    }
    entry.state = State::Failed;
    entry.error = Some(error);
    entry.updated_at = Utc::now();
    register::keep(&entry)?;
    Ok(entry)
}

/// Move `entry` to `step` and keep it.
fn at(entry: &mut Entry, step: Step) -> Result<(), CmdError> {
    entry.step = step;
    entry.updated_at = Utc::now();
    register::keep(entry)
}

/// Deliver one checkout's commit; the entry it ends with.
async fn deliver(
    product: &str,
    checkout: &Path,
    owed: Owed,
    channel: SubmitChannel,
) -> Result<Option<Entry>, CmdError> {
    let (commit, version, bump, published) = match owed {
        Owed::Nothing => return Ok(None),
        Owed::Refused {
            commit,
            step,
            error,
        } => {
            let refused = entry(product, commit.as_deref(), step);
            return failed(refused, CmdError::click(error)).await.map(Some);
        }
        Owed::InFlight { commit, version } => {
            let mut running = entry(product, Some(&commit), Step::Release);
            running.version = Some(version);
            register::keep(&running)?;
            return Ok(Some(running));
        }
        Owed::Release {
            commit,
            version,
            bump,
        } => (commit, version, bump, false),
        Owed::Record { commit, version } => (commit, version, false, true),
    };
    if let Some(kept) = register::kept(product, &commit)? {
        if matches!(kept.state, State::Delivered | State::Failed) {
            return Ok(Some(kept));
        }
    }
    let mut current = entry(product, Some(&commit), Step::Carried);
    current.version = Some(version.clone());
    register::keep(&current)?;
    current.carried = match carried(product, checkout, &commit).await {
        Ok(carried) => carried,
        Err(error) => return failed(current, error).await.map(Some),
    };
    if current.carried.is_empty() {
        current.state = State::NothingCarried;
        current.updated_at = Utc::now();
        register::keep(&current)?;
        return Ok(Some(current));
    }
    let mut released_commit = commit.clone();
    if bump {
        at(&mut current, Step::Version)?;
        match commit_next(checkout, product, &commit, &version).await {
            Ok(written) => {
                current.version_commit = Some(written.clone());
                released_commit = written;
            }
            Err(error) => return failed(current, error).await.map(Some),
        }
    }
    if !published {
        at(&mut current, Step::Release)?;
        let args = ReleaseSubmitArgs::for_checkout(checkout, &released_commit, &version, channel);
        if let Err(error) = submit(&args).await {
            return failed(current, error).await.map(Some);
        }
    }
    at(&mut current, Step::Record)?;
    current.recorded = match record_released(product).await {
        Ok(recorded) => recorded,
        Err(error) => return failed(current, error).await.map(Some),
    };
    current.state = State::Delivered;
    current.updated_at = Utc::now();
    register::keep(&current)?;
    Ok(Some(current))
}

/// One pass over `root`'s checkouts (`products` narrows it): the entry each
/// releasing product ended with, in product order.
pub async fn pass(
    root: &Path,
    products: &[String],
    channel: SubmitChannel,
) -> Result<Vec<Entry>, CmdError> {
    let mut entries = Vec::new();
    for planned in plan(root, products).await? {
        let checkout = root.join(&planned.checkout);
        let owed = owed(planned.standing);
        if let Some(entry) = deliver(&planned.product, &checkout, owed, channel).await? {
            entries.push(entry);
        }
    }
    Ok(entries)
}
