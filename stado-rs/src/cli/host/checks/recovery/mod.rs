//! The declared host repairs the repair capability applies.

pub(in crate::cli::host) mod link;
pub(in crate::cli::host) mod object_api;
pub(in crate::cli::host) mod skarbiec;
pub(in crate::cli::host) mod verifier;

use serde_json::{json, Value};

use crate::cli::CmdError;

/// Run the fixed host recovery implementation for the declared repair
/// capability. The capability owns rendering and the apply boundary.
pub(crate) async fn apply_host_repair(target: &str) -> Result<Value, CmdError> {
    let report =
        crate::deploy::host_recovery::recover_host(target, &crate::deploy::production_runner())
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    Ok(report)
}

/// Apply the declared release-state repair and return its post-delivery
/// inventory proof to the repair capability.
pub(crate) async fn apply_release_state_repair(target: &str) -> Result<Value, CmdError> {
    let runner = crate::deploy::production_runner();
    let registry = crate::cli::registry::read_registry().await?;
    if registry.targets.iter().all(|entry| entry.name != target) {
        return Err(CmdError::click(format!(
            "{target} has no target declaration; add it to the fleet registry."
        )));
    }

    let mut standings = vec![crate::deploy::reconcile::examine(target, &runner).await];

    let mut deliveries: Vec<Value> = Vec::new();
    for standing in &standings {
        if !standing.needs_delivery() {
            continue;
        }
        let entry = registry
            .targets
            .iter()
            .find(|entry| entry.name == standing.target)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{} target declaration disappeared during repair; retry after the registry is stable.",
                    standing.target
                ))
            })?;
        for drifted in &standing.drift {
            if drifted.verdict != "behind" && drifted.verdict != "absent" {
                continue;
            }
            let binary = &drifted.binary;
            let version = entry.declared_version(binary).ok_or_else(|| {
                CmdError::click(format!(
                    "{} declares no desired {binary} version; add it to the target's version declaration.",
                    standing.target
                ))
            })?;
            let outcome = crate::deploy::host_release::release_host(
                &standing.target,
                binary,
                version,
                false,
                false,
                &runner,
            )
            .await;
            deliveries.push(match outcome {
                Ok(report)
                    if matches!(
                        report.get("status").and_then(Value::as_str),
                        Some(
                            crate::deploy::host_release::RELEASED_STATUS
                                | crate::deploy::host_release::ALREADY_ACTIVE_STATUS
                        )
                    ) =>
                {
                    json!({
                        "target": standing.target,
                        "binary": binary,
                        "version": version,
                        "status": "delivered",
                        "report": report,
                    })
                }
                Ok(report) => json!({
                    "target": standing.target,
                    "binary": binary,
                    "version": version,
                    "status": "failed",
                    "detail": report
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("delivery returned a non-success report"),
                    "report": report,
                }),
                Err(error) => json!({
                    "target": standing.target,
                    "binary": binary,
                    "version": version,
                    "status": "failed",
                    "detail": error.to_string(),
                }),
            });
        }
    }
    standings.clear();
    standings.push(crate::deploy::reconcile::examine(target, &runner).await);

    let healthy = standings
        .iter()
        .all(crate::deploy::reconcile::HostStanding::settled)
        && deliveries
            .iter()
            .all(|entry| entry.get("status").and_then(Value::as_str) == Some("delivered"));
    let mut report = crate::deploy::reconcile::report(&standings, &deliveries);
    report["healthy"] = json!(healthy);
    Ok(report)
}
