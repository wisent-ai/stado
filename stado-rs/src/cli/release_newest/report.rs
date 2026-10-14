//! What the operator reads, and the submissions themselves.
//!
//! One product's failure is not the workspace's failure: every releasable
//! product is submitted, the outcome of each is printed as it happens, and
//! the command exits nonzero when any of them failed. A run that stopped
//! half-way through leaves its own durable run object, which is what
//! `stado release status` and `stado release resume` read.

use std::path::Path;

use serde::Serialize;

use chrono::{DateTime, Utc};

use crate::cli::release_loop::push::{Phase, Unpushed};
use crate::cli::release_loop::register::{Entry, State};
use crate::cli::release_loop::Unplaced;
use crate::cli::release_submit::{submit, ReleaseSubmitArgs, SubmitChannel};
use crate::cli::CmdError;

use super::{Planned, Standing};

/// One product's release attempt, after it finished or refused.
#[derive(Debug, Clone, Serialize)]
struct Outcome {
    product: String,
    version: String,
    commit: String,
    released: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    refusal: Option<String>,
}

/// The delivery queue's register as it stands, newest entry first: JSON
/// (`{path, entries}`), or one queue line per entry.
pub fn print_register(json: bool) -> Result<(), CmdError> {
    let path = crate::cli::release_loop::register::path()?;
    let document = crate::cli::release_loop::register::read()?;
    if json {
        let report =
            serde_json::json!({ "path": path.display().to_string(), "entries": document.entries });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("register {}", path.display());
    crate::cli::release_loop::print(&document.entries);
    Ok(())
}

/// The workspace as it stands, the delivery queue kept from it, every open
/// repair no checkout carries and every checkout's unpushed commits, with
/// nothing submitted.
pub fn print_plan(
    root: &Path,
    planned: &[Planned],
    queued: &[Entry],
    unplaced: &[Unplaced],
    unpushed: &[Unpushed],
    json: bool,
) {
    if json {
        let report = serde_json::json!({
            "workspace": root.display().to_string(), "products": planned, "queue": queued,
            "unplaced": unplaced, "unpushed": unpushed,
        });
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("release newest: the plan could not be printed as JSON: {error}")
            }
        }
        return;
    }
    println!("workspace {}", root.display());
    for entry in planned {
        println!("  {}  {}", entry.product, describe(&entry.standing));
    }
    crate::cli::release_loop::print(queued);
    for repair in unplaced {
        println!(
            "  unplaced  {}  {}: {}",
            repair.defect, repair.revision, repair.why
        );
    }
    for checkout in unpushed {
        match (checkout.ahead, &checkout.unreadable) {
            (Some(ahead), _) => println!("  unpushed  {}  {ahead}", checkout.repository),
            (None, Some(why)) => println!("  unpushed  {}  unread: {why}", checkout.repository),
            (None, None) => println!("  unpushed  {}  unread", checkout.repository),
        }
    }
    let releasable = planned.iter().filter(|entry| entry.is_releasable()).count();
    println!(
        "{releasable} of {} product(s) would be released",
        planned.len()
    );
}

/// Submit every releasable product, in the plan's order (`--first` ahead,
/// then name order), after the delivery queue's own deliveries in
/// `delivered` and the push phase in `pushing`; a queued commit that refused
/// in this command, at or after `started`, fails it like a refused
/// submission, and so does a push that did not land.
pub async fn submit_planned(
    root: &Path,
    planned: Vec<Planned>,
    delivered: &[Entry],
    pushing: Option<&Phase>,
    started: DateTime<Utc>,
    channel: SubmitChannel,
    json: bool,
) -> Result<(), CmdError> {
    if !json {
        println!("workspace {}", root.display());
        crate::cli::release_loop::print(delivered);
        if let Some(phase) = pushing {
            crate::cli::release_loop::push::print(phase);
        }
        for entry in planned.iter().filter(|entry| !entry.is_releasable()) {
            println!("  {}  {}", entry.product, describe(&entry.standing));
        }
    }
    let mut outcomes = Vec::new();
    for entry in &planned {
        let Standing::Releasable {
            commit, version, ..
        } = &entry.standing
        else {
            continue;
        };
        if !json {
            println!("  {} {version} from {commit}: releasing", entry.product);
        }
        let args = ReleaseSubmitArgs::for_checkout(&entry.checkout, commit, version, channel);
        let refusal = match submit(&args).await {
            Ok(()) => None,
            Err(error) => Some(error.to_string()),
        };
        if !json {
            match &refusal {
                Some(refusal) => println!("  {} {version}: refused: {refusal}", entry.product),
                None => println!("  {} {version}: released", entry.product),
            }
        }
        outcomes.push(Outcome {
            product: entry.product.clone(),
            version: version.clone(),
            commit: commit.clone(),
            released: refusal.is_none(),
            refusal,
        });
    }
    let refused_queue: Vec<&str> = delivered
        .iter()
        .filter(|entry| entry.state == State::Failed && entry.updated_at >= started)
        .map(|entry| entry.product.as_str())
        .collect();
    let unpushed: Vec<&str> = pushing
        .into_iter()
        .flat_map(|phase| phase.pushed.iter())
        .filter(|pushed| pushed.error.is_some())
        .map(|pushed| pushed.repository.as_str())
        .collect();
    let failed: Vec<&str> = outcomes
        .iter()
        .filter(|outcome| !outcome.released)
        .map(|outcome| outcome.product.as_str())
        .chain(refused_queue)
        .chain(unpushed)
        .collect();
    if json {
        let report = serde_json::json!({
            "workspace": root.display().to_string(),
            "products": planned,
            "queue": delivered,
            "push": pushing,
            "outcomes": outcomes,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} of {} product(s) released",
            outcomes.len() - outcomes.iter().filter(|outcome| !outcome.released).count(),
            outcomes.len()
        );
    }
    if failed.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "release newest: {} product(s) or checkout(s) refused: {}",
        failed.len(),
        failed.join(", ")
    )))
}

fn describe(standing: &Standing) -> String {
    match standing {
        Standing::Releasable {
            commit,
            version,
            uncommitted: 0,
        } => format!("{version} from {commit} is not published yet"),
        Standing::Releasable {
            commit,
            version,
            uncommitted,
        } => format!(
            "{version} from {commit} is not published yet; the checkout's \
             {uncommitted} uncommitted path(s) are not part of it"
        ),
        Standing::Published { version, run, .. } => {
            format!("{version} is already published by run {run}")
        }
        Standing::InFlight {
            commit,
            version,
            run,
        } => format!(
            "{version} from {commit} is being released by run {run}; \
             follow it with `stado release status`"
        ),
        Standing::VersionTaken {
            commit,
            version,
            run,
            taken_by,
            next,
            unadvanceable,
        } => {
            let needed = match (next, unadvanceable) {
                (Some(next), _) => format!(
                    "it needs at least {next}, the versioning rule's step for an internal change \
                     (a change to its surface takes what `stado release gate decide` answers)"
                ),
                (None, Some(cause)) => format!("its next version cannot be computed: {cause}"),
                (None, None) => "its next version was not computed".to_string(),
            };
            format!(
                "{version} belongs to commit {taken_by} (run {run}), and a version's source never \
                 changes; {commit} is new source under it, so {needed} before it can be released"
            )
        }
        Standing::DeclaresNoReleases { reason } => format!("declares no releases: {reason}"),
        Standing::Unreadable { refusal } => format!("cannot be read: {refusal}"),
    }
}
