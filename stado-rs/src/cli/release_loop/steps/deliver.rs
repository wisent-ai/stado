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
use super::carried::{carried, note_failure, open_repairs, record_released, repository_of, Repair};
use super::version::{commit_next, declares_format_gate};
use crate::cli::quality::{check_revision, check_source, Report, Selection};
use crate::cli::release_newest::{Planned, Standing};
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
    /// The declared version is taken and cannot advance: `error` says why.
    Unadvanceable { commit: String, error: String },
    /// The product declares it does not release, or its checkout could not
    /// be read; `release newest` names either in its own listing.
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
            (None, Some(why)) => Owed::Unadvanceable {
                commit,
                error: format!(
                    "{version} is taken by another commit's release and the versioning rule gives no next version: {why}"
                ),
            },
            (None, None) => Owed::Unadvanceable {
                commit,
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
        Standing::Unreadable { .. } | Standing::DeclaresNoReleases { .. } => Owed::Nothing,
    }
}

fn entry(product: &str, commit: &str, step: Step) -> Entry {
    let now = Utc::now();
    Entry {
        product: product.to_string(),
        commit: commit.to_string(),
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
    note_failure(
        &entry.carried,
        &entry.product,
        &entry.commit,
        entry.step.name(),
        &error,
    )
    .await;
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

/// What a pass does with a commit that owes a release.
#[derive(Clone, Copy)]
pub enum Mode {
    /// Keep it queued with the steps it needs, carried defects named; no
    /// version is committed and nothing is built. What the serve role and
    /// `stado release newest --plan` do: a build waits for the gate.
    Queue,
    /// Run the queued steps on `channel`: what `stado release newest` does,
    /// the release command the build gate admits or refuses.
    Deliver(SubmitChannel),
}

/// Whether `kept` stands for good: delivered, or failed at a step that
/// acted (committed a version, submitted, recorded). A failure before any
/// action — an unreadable checkout, Oko not answering — is read again.
fn settled(kept: &Entry) -> bool {
    match kept.state {
        State::Delivered => true,
        State::Failed => matches!(
            kept.step,
            Step::Gates | Step::Version | Step::Release | Step::Record
        ),
        State::Running | State::Queued | State::NothingCarried => false,
    }
}

/// Queue or deliver one checkout's commit; the entry it ends with.
async fn deliver(
    product: &str,
    checkout: &Path,
    repairs: &[Repair],
    owed: Owed,
    mode: Mode,
) -> Result<Option<Entry>, CmdError> {
    let (commit, version, bump, published, refusal) = match owed {
        Owed::Nothing => return Ok(None),
        Owed::Unadvanceable { commit, error } => (commit, None, false, false, Some(error)),
        Owed::InFlight { commit, version } => {
            let mut running = entry(product, &commit, Step::Release);
            running.version = Some(version);
            register::keep(&running)?;
            return Ok(Some(running));
        }
        Owed::Release {
            commit,
            version,
            bump,
        } => (commit, Some(version), bump, false, None),
        Owed::Record { commit, version } => (commit, Some(version), false, true, None),
    };
    let kept = register::kept(product, &commit)?;
    if let Some(kept) = kept.as_ref().filter(|kept| settled(kept)) {
        return Ok(Some(kept.clone()));
    }
    let mut current = entry(product, &commit, Step::Carried);
    if let Some(kept) = &kept {
        current.started_at = kept.started_at;
    }
    current.version = version.clone();
    current.carried = match carried(repairs, checkout, &commit).await {
        Ok(carried) => carried,
        Err(error) => return failed(current, error).await.map(Some),
    };
    if current.carried.is_empty() {
        current.state = State::NothingCarried;
        register::keep(&current)?;
        return Ok(Some(current));
    }
    let (Some(version), None) = (version, refusal.as_ref()) else {
        current.step = Step::Version;
        let error = match refusal {
            Some(error) => error,
            None => format!("the plan named no version for {product} at {commit}"),
        };
        return failed(current, CmdError::click(error)).await.map(Some);
    };
    // The format gates, the lock check and the module declarations read the
    // committed tree and compile nothing, so they run as soon as a commit
    // carries a repair, not behind the build gate. A commit kept queued has
    // passed them.
    let checked = kept
        .as_ref()
        .is_some_and(|kept| kept.state == State::Queued);
    if !published && !checked {
        current.step = Step::Gates;
        let gated = match declares_format_gate(checkout, product) {
            Ok(gated) => gated,
            Err(error) => return failed(current, error).await.map(Some),
        };
        let source = match gated {
            true => check_revision(checkout, &commit, Report::Stderr, Selection::Formatting).await,
            false => check_source(checkout, &commit, Report::Stderr).await,
        };
        if let Err(error) = source {
            return failed(current, error).await.map(Some);
        }
    }
    let first = match (bump, published) {
        (true, _) => Step::Version,
        (false, false) => Step::Release,
        (false, true) => Step::Record,
    };
    let channel = match mode {
        Mode::Queue => {
            current.state = State::Queued;
            current.step = first;
            register::keep(&current)?;
            return Ok(Some(current));
        }
        Mode::Deliver(channel) => channel,
    };
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
    let repository = repository_of(checkout)?;
    current.recorded = match record_released(product, repository).await {
        Ok(recorded) => recorded,
        Err(error) => return failed(current, error).await.map(Some),
    };
    current.state = State::Delivered;
    current.updated_at = Utc::now();
    register::keep(&current)?;
    Ok(Some(current))
}

/// One pass over `planned`, the plan of `root`'s checkouts: the entry each
/// releasing product ended with, in product order. Oko's open repairs are
/// read once for the pass; when Oko cannot answer, the pass queues nothing
/// and says why on stderr, so `release newest` still releases what its plan
/// holds on a machine without Oko.
pub async fn pass(root: &Path, planned: Vec<Planned>, mode: Mode) -> Result<Vec<Entry>, CmdError> {
    let repairs = match open_repairs().await {
        Ok(repairs) => repairs,
        Err(error) => {
            eprintln!("stado release queue: no commit is queued: Oko's open repairs could not be read: {error}");
            return Ok(Vec::new());
        }
    };
    let mut entries = Vec::new();
    for planned in planned {
        let checkout = root.join(&planned.checkout);
        let owed = owed(planned.standing);
        if let Some(entry) = deliver(&planned.product, &checkout, &repairs, owed, mode).await? {
            entries.push(entry);
        }
    }
    Ok(entries)
}
