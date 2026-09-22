//! Making one move, under the lease that makes it the only one.
//!
//! A profile is held for the length of the change, so two reconcilers cannot
//! move the same profile at once, and the lease is released whether the change
//! succeeded or not. A release that finds someone else holding the lease turns
//! a successful change into a reported failure on purpose: the change happened,
//! but this pass can no longer say it owned it.

use chrono::Utc;
use serde_json::Value;

use crate::autonomy::policy::AutonomyPolicy;
use crate::queue::{JobStorage, StorageError};

use super::super::plan::Due;
use super::super::{words, DueAction, ReliefReport, ReliefRow};

/// Carry out one due action. Answers whether the tick's relocation was spent:
/// a profile held by another reconciler spends nothing.
pub(super) async fn execute(
    store: &JobStorage,
    policy: &AutonomyPolicy,
    document: &Value,
    generation: &str,
    due: Due,
    row: &mut ReliefRow,
    pass: &mut ReliefReport,
) -> Result<bool, StorageError> {
    let lease_subject = format!("placement:{}", due.profile.name);
    let Some(lease) = crate::autonomy::storage::acquire_placement_lease(
        store,
        &lease_subject,
        &pass.decision_id,
        "placement-relief",
        policy.limits.decision_ttl_seconds,
        Utc::now(),
    )
    .await?
    else {
        row.classification = words::LEASE_BLOCKED.to_string();
        row.detail = format!("{}; another reconciler owns this profile", row.detail);
        pass.summary.blocked += 1;
        return Ok(false);
    };

    if matches!(due.action, DueAction::Standby) {
        let reason = format!(
            "placement relief: {} is over its memory watermark and no declared host has \
             more headroom",
            row.placed_on.as_deref().unwrap_or_default()
        );
        let prepared =
            crate::cli::placement::standby::prepare(&due.profile.name, &due.to_host, &reason).await;
        let released =
            crate::autonomy::storage::release_placement_lease(store, &lease_subject, &lease.token)
                .await;
        match (prepared, released) {
            (Ok(report), Ok(true)) => {
                let standby_words = crate::cli::placement::standby::words::DECLARED;
                row.classification = if report.outcome == standby_words {
                    words::STANDBY_PREPARED.to_string()
                } else {
                    words::STANDBY_PENDING.to_string()
                };
                row.detail = format!("{}; {}: {}", row.detail, report.outcome, report.detail);
                pass.summary.relocated += 1;
                crate::autonomy::storage::record_mutation_outcome(
                    store,
                    true,
                    None,
                    policy.limits.circuit_breaker_failures,
                    policy.limits.circuit_breaker_cooldown_seconds,
                )
                .await?;
            }
            (Ok(_), Ok(false)) => {
                row.classification = words::STANDBY_REFUSED.to_string();
                row.detail = format!(
                    "{}; standby finished, but mutation lease ownership changed before release",
                    row.detail
                );
                pass.summary.failures += 1;
            }
            (Ok(_), Err(error)) => {
                row.classification = words::STANDBY_REFUSED.to_string();
                row.detail = format!(
                    "{}; standby finished, but mutation lease release failed: {error}",
                    row.detail
                );
                pass.summary.failures += 1;
            }
            (Err(error), _) => {
                row.classification = words::STANDBY_REFUSED.to_string();
                row.detail = format!("{}; {error}", row.detail);
                pass.summary.failures += 1;
                crate::autonomy::storage::record_mutation_outcome(
                    store,
                    false,
                    Some(&error.to_string()),
                    policy.limits.circuit_breaker_failures,
                    policy.limits.circuit_breaker_cooldown_seconds,
                )
                .await?;
            }
        }
        return Ok(true);
    }

    let mut result = crate::cli::placement::relocate(
        document.clone(),
        generation.to_string(),
        due.profile,
        &due.to_host,
    )
    .await;
    match crate::autonomy::storage::release_placement_lease(store, &lease_subject, &lease.token)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            result = Err(
                "relocation finished, but mutation lease ownership changed before release"
                    .to_string(),
            );
        }
        Err(error) => {
            result = Err(format!(
                "relocation finished, but mutation lease release failed: {error}"
            ));
        }
    }
    match result {
        Ok(receipt) => {
            row.classification = words::RELOCATED.to_string();
            row.transaction_id = receipt.transaction_id;
            row.detail = format!(
                "{}; moved {} from {} to {} ({})",
                row.detail,
                receipt.profile,
                receipt.from_host,
                receipt.to_host,
                receipt.registry_generation.map_or_else(
                    || receipt.status.to_string(),
                    |generation| { format!("registry generation {generation}") }
                )
            );
            pass.relocations
                .insert(row.profile.clone(), pass.created_at.clone());
            pass.summary.relocated += 1;
            crate::autonomy::storage::record_mutation_outcome(
                store,
                true,
                None,
                policy.limits.circuit_breaker_failures,
                policy.limits.circuit_breaker_cooldown_seconds,
            )
            .await?;
        }
        Err(error) => {
            row.classification = words::RELOCATION_FAILED.to_string();
            row.detail = format!("{}; {error}", row.detail);
            pass.summary.failures += 1;
            crate::autonomy::storage::record_mutation_outcome(
                store,
                false,
                Some(&error),
                policy.limits.circuit_breaker_failures,
                policy.limits.circuit_breaker_cooldown_seconds,
            )
            .await?;
        }
    }
    Ok(true)
}
