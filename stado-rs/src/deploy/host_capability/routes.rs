//! The capability route table on the host that resolves it, and the vault
//! inventory read that answers beside it.
//!
//! Skarbiec spells this group `route resolve`, `route declare` and
//! `route verify`. It spelled them `routes list`, `routes add` and
//! `routes verify` until the merge that added declared route resolution, and
//! brokers older than that merge are still installed across the fleet — so
//! [`super::stale_broker`] turns their `unknown command: route` into one
//! sentence about delivery rather than a routing failure nobody can act on.

use serde_json::Value;

use super::{run_json, RemoteBroker};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The target's capability route table, with that host's own answer for each
/// route.
///
/// `route resolve` with no name reports every route the vault answers, each
/// row carrying `resource`, `item`, `field` and the two booleans every reader
/// here consumes, plus the `declared_by` that says whether the row came from
/// the hand-declared table or from what the item declares about itself.
pub async fn routes(
    target: &ComputeTarget,
    broker: &RemoteBroker,
    runner: &Runner,
) -> Result<Value, DeployError> {
    run_json(target, broker, &["route", "resolve"], runner).await
}

/// Declare one capability route on the target.
///
/// Idempotent in Skarbiec itself: a route that already says exactly this is
/// reported with `declared: false` and nothing is written, and a resource
/// already mapped elsewhere is refused rather than repointed. `--reason` is
/// required there and so it is required here.
pub async fn route_add(
    target: &ComputeTarget,
    broker: &RemoteBroker,
    resource: &str,
    item: &str,
    field: &str,
    reason: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    run_json(
        target,
        broker,
        &[
            "route",
            "declare",
            "--resource",
            resource,
            "--item",
            item,
            "--field",
            field,
            "--reason",
            reason,
        ],
        runner,
    )
    .await
}

/// The target's own verification of its route table.
///
/// `route resolve` reports two booleans per route and `route verify` reports
/// the SENTENCE behind a false one — which item would not open, and why. That
/// distinction matters over a channel: a non-interactive session may be unable
/// to open a vault the broker service on that host opens perfectly well, and
/// without the sentence the two are indistinguishable.
///
/// Skarbiec prints the report and THEN exits non-zero when any route is
/// broken, so a non-zero exit carrying a JSON report is the documented success
/// shape here, not a failure. A broker that never printed one is the case
/// [`super::stale_broker`] answers.
pub async fn verify_routes(
    target: &ComputeTarget,
    broker: &RemoteBroker,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let output =
        host_channel::run_command(target, &broker.command(&["route", "verify"]), runner).await?;
    let said = output.stdout.trim();
    if let Ok(report) = serde_json::from_str::<Value>(said) {
        return Ok(report);
    }
    let reason = host_channel::last_error_line(&output, "the host gave no reason");
    if let Some(stale) = super::stale_broker(target, broker, &reason) {
        return Err(stale);
    }
    Err(DeployError(format!(
        "{}: `skarbiec route verify` gave no report against {}: {reason}",
        target.name, broker.vault,
    )))
}

/// The nonsecret item inventory of the target's own vault.
///
/// `skarbiec list` reads the vault's envelope, the same way `fleet vaults`
/// reads it to count vaults, so this answers on a host whose gpg a channel
/// session cannot spawn. An item's name is its `id`; no field value is read.
pub async fn items(
    target: &ComputeTarget,
    broker: &RemoteBroker,
    runner: &Runner,
) -> Result<Vec<Value>, DeployError> {
    let answer = run_json(target, broker, &["list"], runner).await?;
    answer.as_array().cloned().ok_or_else(|| {
        DeployError(format!(
            "{}: skarbiec list was not a JSON array",
            target.name
        ))
    })
}
