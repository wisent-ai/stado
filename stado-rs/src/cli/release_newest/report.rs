//! What the operator reads, and the submissions themselves.
//!
//! One product's failure is not the workspace's failure: every releasable
//! product is submitted, the outcome of each is printed as it happens, and
//! the command exits nonzero when any of them failed. A run that stopped
//! half-way through leaves its own durable run object, which is what
//! `stado release status` and `stado release resume` read.

use std::path::Path;

use serde::Serialize;

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

/// The workspace as it stands, with nothing submitted.
pub fn print_plan(root: &Path, planned: &[Planned], json: bool) {
    if json {
        let report =
            serde_json::json!({ "workspace": root.display().to_string(), "products": planned });
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
    let releasable = planned.iter().filter(|entry| entry.is_releasable()).count();
    println!(
        "{releasable} of {} product(s) would be released",
        planned.len()
    );
}

/// Submit every releasable product, in name order.
pub async fn submit_planned(
    root: &Path,
    planned: Vec<Planned>,
    channel: SubmitChannel,
    json: bool,
) -> Result<(), CmdError> {
    if !json {
        println!("workspace {}", root.display());
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
    let failed: Vec<&Outcome> = outcomes
        .iter()
        .filter(|outcome| !outcome.released)
        .collect();
    if json {
        let report = serde_json::json!({
            "workspace": root.display().to_string(),
            "products": planned,
            "outcomes": outcomes,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} of {} product(s) released",
            outcomes.len() - failed.len(),
            outcomes.len()
        );
    }
    if failed.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "release newest: {} product(s) refused: {}",
        failed.len(),
        failed
            .iter()
            .map(|outcome| outcome.product.as_str())
            .collect::<Vec<_>>()
            .join(", ")
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
        Standing::DeclaresNoReleases { reason } => format!("declares no releases: {reason}"),
        Standing::Unreadable { refusal } => format!("cannot be read: {refusal}"),
    }
}
