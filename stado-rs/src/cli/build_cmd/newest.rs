//! `stado build newest` — build every product in this workspace from the
//! commit it stands on, and say what failed. Nothing is released.
//!
//! `stado release newest` was asked for as "build newest", and the operator
//! said of what was delivered: "to jest release. a nie build". This is the
//! build: the same reading of the workspace — every product checkout, the
//! commit it is on, the version that commit declares — and then one build
//! per product, kept under its own id. A version already published is still
//! built when its checkout moved on, because the question here is whether
//! the newest commit compiles, not whether it is released.

use std::path::PathBuf;

use clap::Args;
use serde::Serialize;

use crate::cli::build_cmd::{
    current_build, ensure_build, ensure_object_store, read_source, stage_source,
};
use crate::cli::release_newest::{plan, workspace, Planned, Standing};
use crate::cli::CmdError;
use crate::release_pipeline::{BuildRunState, PlatformRunState};

#[derive(Args)]
pub struct BuildNewestArgs {
    /// The directory holding the product checkouts. Defaults to the parent of
    /// the checkout this command is run in.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Build only these products; repeat for several. The default is every
    /// product the workspace holds.
    #[arg(long = "product")]
    products: Vec<String>,
    /// Read what would be built and why the rest is skipped, without
    /// queueing anything.
    #[arg(long)]
    plan: bool,
    /// Follow every queued build to its end, and exit nonzero naming each
    /// product whose build failed.
    #[arg(long)]
    wait: bool,
    #[arg(long)]
    json: bool,
}

/// One product's build, after it was queued, followed, or refused.
#[derive(Debug, Clone, Serialize)]
struct Outcome {
    product: String,
    version: String,
    commit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    build_id: Option<String>,
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<String>,
}

impl Outcome {
    fn failed(&self) -> bool {
        self.build_id.is_none() || self.state == BuildRunState::Failed.word()
    }
}

/// The commit and version a checkout is built from, whatever its release
/// standing: published or not, its newest commit is what compiles or not.
fn buildable(entry: &Planned) -> Option<(&str, &str)> {
    match &entry.standing {
        Standing::Releasable {
            commit, version, ..
        }
        | Standing::Published {
            commit, version, ..
        } => Some((commit.as_str(), version.as_str())),
        Standing::DeclaresNoReleases { .. } | Standing::Unreadable { .. } => None,
    }
}

fn describe(entry: &Planned) -> String {
    match &entry.standing {
        Standing::Releasable {
            commit, version, ..
        }
        | Standing::Published {
            commit, version, ..
        } => {
            format!("{version} from {commit} would be built")
        }
        Standing::DeclaresNoReleases { reason } => format!("declares no releases: {reason}"),
        Standing::Unreadable { refusal } => format!("cannot be read: {refusal}"),
    }
}

pub async fn newest(args: &BuildNewestArgs) -> Result<(), CmdError> {
    let root = workspace(args.root.clone())?;
    let planned = plan(&root, &args.products).await?;
    if args.plan {
        if args.json {
            let report = serde_json::json!({
                "workspace": root.display().to_string(),
                "products": planned,
            });
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!("workspace {}", root.display());
            for entry in &planned {
                println!("  {}  {}", entry.product, describe(entry));
            }
            let buildable = planned
                .iter()
                .filter(|entry| buildable(entry).is_some())
                .count();
            println!("{buildable} of {} product(s) would be built", planned.len());
        }
        return Ok(());
    }
    if !args.json {
        println!("workspace {}", root.display());
        for entry in planned.iter().filter(|entry| buildable(entry).is_none()) {
            println!("  {}  {}", entry.product, describe(entry));
        }
    }
    let mut outcomes = Vec::new();
    for entry in &planned {
        let Some((commit, version)) = buildable(entry) else {
            continue;
        };
        if !args.json {
            println!("  {} {version} from {commit}: building", entry.product);
        }
        let mut outcome = Outcome {
            product: entry.product.clone(),
            version: version.to_owned(),
            commit: commit.to_owned(),
            build_id: None,
            state: "refused".to_owned(),
            failure: None,
        };
        match build_checkout(entry, commit, version).await {
            Ok((build_id, state, failure)) => {
                outcome.build_id = Some(build_id);
                outcome.state = state.word().to_owned();
                outcome.failure = failure;
            }
            Err(error) => outcome.failure = Some(error.to_string()),
        }
        if !args.json {
            match (&outcome.build_id, &outcome.failure) {
                (Some(build_id), None) => {
                    println!(
                        "  {} {version}: build {build_id} {}",
                        entry.product, outcome.state
                    )
                }
                (Some(build_id), Some(failure)) => println!(
                    "  {} {version}: build {build_id} {}: {failure}",
                    entry.product, outcome.state
                ),
                (None, failure) => println!(
                    "  {} {version}: refused: {}",
                    entry.product,
                    failure.as_deref().unwrap_or_default()
                ),
            }
        }
        outcomes.push(outcome);
    }
    if args.wait {
        for outcome in &mut outcomes {
            let Some(build_id) = &outcome.build_id else {
                continue;
            };
            let build = current_build(build_id, true).await?;
            outcome.state = build.state.word().to_owned();
            outcome.failure = build.failure.clone().or_else(|| {
                build
                    .platforms
                    .values()
                    .filter(|platform| platform.state == PlatformRunState::Failed)
                    .filter_map(|platform| {
                        platform
                            .failure
                            .as_deref()
                            .map(|failure| format!("{}: {failure}", platform.platform))
                    })
                    .reduce(|left, right| format!("{left}; {right}"))
            });
            if !args.json {
                match &outcome.failure {
                    Some(failure) => println!(
                        "  {} {}: build {build_id} {}: {failure}",
                        outcome.product, outcome.version, outcome.state
                    ),
                    None => println!(
                        "  {} {}: build {build_id} {}",
                        outcome.product, outcome.version, outcome.state
                    ),
                }
            }
        }
    }
    let failed: Vec<&Outcome> = outcomes.iter().filter(|outcome| outcome.failed()).collect();
    if args.json {
        let report = serde_json::json!({
            "workspace": root.display().to_string(),
            "products": planned,
            "outcomes": outcomes,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let verb = if args.wait { "built" } else { "queued" };
        println!(
            "{} of {} product(s) {verb}; `stado build status <id>` follows each build",
            outcomes.len() - failed.len(),
            outcomes.len()
        );
    }
    if failed.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "build newest: {} product(s) failed or refused: {}",
        failed.len(),
        failed
            .iter()
            .map(|outcome| outcome.product.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// One checkout's build, as `stado build submit` would queue it.
async fn build_checkout(
    entry: &Planned,
    commit: &str,
    version: &str,
) -> Result<(String, BuildRunState, Option<String>), CmdError> {
    let reading = read_source(&entry.checkout, Some(commit), version)?;
    ensure_object_store().await?;
    let staged = stage_source(&reading).await?;
    let (build, enqueue_failure) = ensure_build(&reading, &staged, version).await?;
    Ok((
        build.build_id,
        build.state,
        enqueue_failure.map(|error| error.to_string()),
    ))
}
