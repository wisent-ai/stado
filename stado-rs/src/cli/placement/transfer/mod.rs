//! The fenced transaction itself: the state a move carries between hosts, the
//! progress a rollback reads, and the one remote-script primitive every step
//! below is built from.

mod execution;
mod readiness;
mod state;
mod units;

pub(in crate::cli::placement) use execution::{execute_move, release_claim, rollback};
pub(in crate::cli::placement) use state::cleanup_state_backup;

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
    bytes: Option<Vec<u8>>,
}

#[derive(Default)]
pub(in crate::cli::placement) struct Progress {
    source_stopped: bool,
    pub(in crate::cli::placement) destination_written: Vec<String>,
    route_applied: bool,
    destination_started: bool,
    source_retired: bool,
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
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: {operation} failed: {}",
            target.name,
            host_channel::last_error_line(&output, "remote command failed")
        )));
    }
    Ok(output)
}
