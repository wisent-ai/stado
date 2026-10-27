//! The fenced transaction itself: the state a move carries between hosts, the
//! progress a rollback reads, and the one remote-script primitive every step
//! below is built from.

mod cutover;
mod execution;
mod journal;
mod readiness;
mod role;
mod state;
mod units;

pub(in crate::cli::placement) use execution::{execute_move, release_claim, rollback};
pub(in crate::cli::placement) use journal::{
    forget as forget_progress, recorded as recorded_progress, running_pid as running_move_pid,
    summary as progress_summary, sweep as sweep_progress,
};
pub(in crate::cli::placement) use readiness::preflight;
pub(in crate::cli::placement) use role::{apply_role, ensure_runs_on, move_role, prepare_role, store_fence};
pub(in crate::cli::placement) use state::settle_state_backup;

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;

use crate::cli::placement::candidates::deploy_error;
use crate::cli::{registry, CmdError};
use crate::deploy::{host_channel, CommandOutput, Runner};
use crate::placement::{PlacementProfile, PlacementState, PlacementTransaction};
use crate::targets::{ComputeTarget, Registry};

type RegistryCommitter =
    Arc<dyn Fn(Value, String) -> BoxFuture<'static, Result<String, CmdError>> + Send + Sync>;

pub(in crate::cli::placement) fn production_committer() -> RegistryCommitter {
    Arc::new(|document, expected_generation| {
        Box::pin(async move { registry::push_document_if(&document, &expected_generation).await })
    })
}

#[derive(Debug, Clone)]
struct StateSnapshot {
    spec: PlacementState,
    /// A file state's bytes, `None` when the source held none.
    bytes: Option<Vec<u8>>,
    /// A tree state's copy on this machine, `None` when the source held none.
    tree: Option<std::path::PathBuf>,
}

/// How long one step of a move took, on which host, and whether it ended
/// with its answer: the receipt lists them so a move that ran long says where
/// its time went.
#[derive(Clone, serde::Serialize)]
pub(in crate::cli::placement) struct StepTiming {
    pub(in crate::cli::placement) step: String,
    pub(in crate::cli::placement) host: String,
    pub(in crate::cli::placement) seconds: f64,
    pub(in crate::cli::placement) succeeded: bool,
}

#[derive(Default)]
pub(in crate::cli::placement) struct Progress {
    source_stopped: bool,
    pub(in crate::cli::placement) destination_written: Vec<PlacementState>,
    route_applied: bool,
    destination_started: bool,
    source_retired: bool,
    /// Every step this process ran, in order, with its time.
    pub(in crate::cli::placement) steps: Vec<StepTiming>,
}

impl Progress {
    /// The progress of a move whose process ended without saying how far it
    /// got: every step counted as taken, so a rollback undoes each one. Each
    /// undo ends where it should on a step that never happened: a state with
    /// no backup is left untouched, a destination unit is retired again and a
    /// source unit started again.
    pub(in crate::cli::placement) fn every_step(profile: &PlacementProfile) -> Self {
        Self {
            source_stopped: true,
            destination_written: profile.state.clone(),
            route_applied: true,
            destination_started: true,
            source_retired: true,
            steps: Vec::new(),
        }
    }
}

pub(in crate::cli::placement) struct MoveContext {
    pub(in crate::cli::placement) profile: PlacementProfile,
    pub(in crate::cli::placement) source: ComputeTarget,
    pub(in crate::cli::placement) destination: ComputeTarget,
    pub(in crate::cli::placement) registry: Registry,
    pub(in crate::cli::placement) claimed_document: Value,
    pub(in crate::cli::placement) claim_generation: String,
    pub(in crate::cli::placement) transaction: PlacementTransaction,
}

fn marker_line<'a>(output: &'a CommandOutput, marker: &str) -> Option<&'a str> {
    output.stdout.lines().find(|line| line.starts_with(marker))
}

async fn run_host_script(
    target: &ComputeTarget,
    script: &str,
    runner: &Runner,
    operation: &str,
) -> Result<CommandOutput, CmdError> {
    let output = host_channel::run_script(target, script, runner)
        .await
        .map_err(deploy_error)?;
    if !output.succeeded() {
        return Err(CmdError::click(format!(
            "{}: {operation} failed: {}",
            target.name,
            host_channel::last_error_line(&output, "remote command failed")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Ok(output)
}
