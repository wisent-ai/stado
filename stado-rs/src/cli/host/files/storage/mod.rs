//! The host-local storage replica: its backup audit and its root
//! reconciliation.

pub(in crate::cli::host) mod audit;

use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::host::checks::probes::report_outcome;

/// The replica roots every fleet host uses, relative to the managed home. Both
/// are the values the service catalog declares for the object API unit
/// (`WC_LOCAL_STORAGE_PATH`) and its replica, so this command reads the same
/// layout the fleet installs rather than asking an operator to name a path.
const BACKUP_ROOT: &str = ".stado/local-backup";

const PRIMARY_ROOT: &str = ".stado/local-storage";

pub struct StorageRootReconciliationResult {
    pub report: Value,
    pub outcome: Result<(), CmdError>,
}

/// One product result for both the CLI and the authenticated host API.
pub async fn storage_root_reconcile_result(
    target: &str,
    transaction: &str,
    phase: &str,
) -> Result<StorageRootReconciliationResult, crate::deploy::DeployError> {
    let runner = crate::deploy::production_runner();
    let report =
        crate::deploy::host_storage_reconcile::reconcile_host(target, transaction, phase, &runner)
            .await?;
    // A write acknowledges its resident owner; only STATUS observes completion.
    let outcome = report_outcome(&report, if phase == "status" { "ok" } else { "accepted" });
    Ok(StorageRootReconciliationResult { report, outcome })
}

pub async fn storage_root_reconcile_worker(
    target: &str,
    target_config: &str,
    transaction: &str,
    phase: &str,
    source_revision: &str,
    tool_sha256: &str,
    runner_gate: &str,
) -> Result<(), CmdError> {
    use base64::Engine;

    let decode = |label: &str, encoded: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| CmdError::click(format!("invalid resident {label}: {error}")))
    };
    let target_config = serde_json::from_slice::<crate::targets::ComputeTarget>(&decode(
        "target config",
        target_config,
    )?)
    .map_err(|error| CmdError::click(format!("invalid resident target config: {error}")))?;
    if target_config.name != target {
        return Err(CmdError::click(
            "resident target config belongs to another target",
        ));
    }
    let runner_gate = if runner_gate.is_empty() {
        None
    } else {
        Some(
            serde_json::from_slice::<Value>(&decode("runner gate", runner_gate)?).map_err(
                |error| CmdError::click(format!("invalid resident runner gate: {error}")),
            )?,
        )
    };
    let runner = crate::deploy::production_runner();
    crate::deploy::host_storage_reconcile::reconcile_host_worker(
        target_config,
        transaction,
        phase,
        source_revision,
        tool_sha256,
        runner_gate,
        &runner,
    )
    .await
    .map(|_| ())
    .map_err(|error| CmdError::click(error.to_string()))
}
