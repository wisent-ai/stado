//! Executing an approved plan: the mutation slot lease, the emergency-pause
//! and circuit-breaker gate taken after it, and the outcome recorded for both.

use chrono::Utc;

use crate::cli::resources::model::Plan;
use crate::queue::{JobStorage, StorageError};

use super::policy::AutonomyPolicy;

pub(super) async fn execute_with_circuit(
    store: &JobStorage,
    plan: &Plan,
    policy: &AutonomyPolicy,
) -> Result<(), StorageError> {
    let mut mutation_lease = None;
    for slot in usize::default()..policy.limits.max_concurrent_mutations {
        let subject = format!("mutation-slot-{slot}");
        if let Some(lease) = super::storage::acquire_placement_lease(
            store,
            &subject,
            &plan.operation_id,
            "autonomy-reconciler",
            policy.limits.decision_ttl_seconds,
            Utc::now(),
        )
        .await?
        {
            mutation_lease = Some((subject, lease.token));
            break;
        }
    }
    let Some((lease_subject, lease_token)) = mutation_lease else {
        return Err(StorageError::Other(
            "autonomy mutation concurrency limit reached".to_string(),
        ));
    };

    let control_gate = match super::storage::load_control(store).await {
        Ok(control) if control.emergency_paused => {
            Some("autonomy emergency pause became active before mutation".to_string())
        }
        Ok(control) if control.circuit_open_at(Utc::now()) => Some(format!(
            "autonomy circuit breaker opened before mutation until {}",
            control.circuit_open_until.as_deref().unwrap_or("unknown")
        )),
        Ok(_) => None,
        Err(error) => Some(format!(
            "autonomy control state became unreadable before mutation: {error}"
        )),
    };
    if let Some(mut detail) = control_gate {
        if let Some(release_error) =
            release_mutation_lease(store, &lease_subject, &lease_token).await
        {
            detail.push_str("; ");
            detail.push_str(&release_error);
        }
        return Err(StorageError::Other(detail));
    }

    let execution = crate::cli::resources::engine::execute_autonomous(plan).await;
    let release_error = release_mutation_lease(store, &lease_subject, &lease_token).await;
    match execution {
        Ok(()) => {
            super::storage::record_mutation_outcome(
                store,
                true,
                None,
                policy.limits.circuit_breaker_failures,
                policy.limits.circuit_breaker_cooldown_seconds,
            )
            .await?;
            if let Some(error) = release_error {
                return Err(StorageError::Other(error));
            }
            Ok(())
        }
        Err(error) => {
            let mut detail = error.to_string();
            if let Some(release_error) = release_error {
                detail.push_str("; ");
                detail.push_str(&release_error);
            }
            super::storage::record_mutation_outcome(
                store,
                false,
                Some(&detail),
                policy.limits.circuit_breaker_failures,
                policy.limits.circuit_breaker_cooldown_seconds,
            )
            .await?;
            Err(StorageError::Other(detail))
        }
    }
}
async fn release_mutation_lease(store: &JobStorage, subject: &str, token: &str) -> Option<String> {
    match super::storage::release_placement_lease(store, subject, token).await {
        Ok(true) => None,
        Ok(false) => Some("mutation lease ownership changed before release".to_string()),
        Err(error) => Some(format!("mutation lease release failed: {error}")),
    }
}
