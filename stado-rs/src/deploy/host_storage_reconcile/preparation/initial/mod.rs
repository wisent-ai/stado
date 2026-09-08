use super::*;

mod candidate;
mod gate;
mod inventory;
mod roles;

pub(in crate::deploy::host_storage_reconcile) use gate::*;
pub(in crate::deploy::host_storage_reconcile) use roles::*;

use candidate::*;
use inventory::*;

pub(in crate::deploy::host_storage_reconcile) async fn initial_lifecycle_fence(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<LifecycleFence, DeployError> {
    let resident_owner = resident_owner_retention(transaction)?;
    let resident_owner_unit = resident_owner
        .get("native_manager")
        .and_then(|manager| manager.get("service"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            DeployError("resident owner evidence omitted its exact service".to_string())
        })?
        .to_string();
    let services = registry_services(storage_target, &resident_owner_unit, runner).await?;
    let repository_runner_gate = repository_runner_gate().await?;
    let staged_runtime = crate::deploy::host_release::stage_declared_release(
        &storage_target.name,
        "stado",
        storage_target
            .managed_versions
            .get("stado")
            .ok_or_else(|| {
                DeployError("target has no current declared Stado runtime".to_string())
            })?,
        runner,
    )
    .await?;
    let current_runner = repository_runner_gate
        .as_ref()
        .and_then(|gate| gate.get("current_runner"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let store = crate::queue::JobStorage::new()
        .await
        .map_err(|error| DeployError(format!("cannot read queue before fencing: {error}")))?;
    let prior_versioned = store
        .read_text_versioned(crate::queue::control::CONTROL_BLOB)
        .await
        .map_err(|error| DeployError(format!("cannot read prior queue state: {error}")))?;
    let prior = parse_queue_control(
        prior_versioned
            .as_ref()
            .map(|versioned| versioned.content.as_str()),
    )?;
    let mut writers = Vec::new();
    let mut transport_retained = Vec::new();
    let mut owning_runner_found = false;
    let mut non_storage_retained = Vec::new();
    let mut object_port = None;
    for candidate in &services {
        if let Some(writer) = fenced_writer(
            candidate,
            current_runner.as_deref(),
            &staged_runtime,
            &mut object_port,
            &mut owning_runner_found,
            &mut transport_retained,
            &mut non_storage_retained,
            runner,
        )
        .await?
        {
            writers.push(writer);
        }
    }
    writers.sort_by_key(|writer| stop_priority(&writer.role));
    if !writers.iter().any(|writer| writer.role == "object-api") {
        return Err(DeployError(
            "fleet service inventory did not resolve the canonical object API".to_string(),
        ));
    }
    if repository_runner_gate.is_some() && !owning_runner_found {
        return Err(DeployError(
            "runner gate did not map its owning native runner service".to_string(),
        ));
    }
    let runtime = observe_object_runtime(
        storage_target,
        object_port.ok_or_else(|| DeployError("object API port is absent".to_string()))?,
        runner,
    )
    .await?;
    let object_writer = writers
        .iter_mut()
        .find(|writer| writer.role == "object-api")
        .expect("canonical object API writer was required above");
    let roots = capture_storage_roots(transaction, runtime, object_writer, &staged_runtime)?;
    object_writer.forward_object_recovery = Some(object_recovery_script(
        object_writer,
        &roots.primary,
        Some(&roots.backup),
    )?);
    object_writer.rollback_object_recovery = Some(object_recovery_script(
        object_writer,
        &roots.prior_primary,
        roots.prior_backup.as_deref(),
    )?);
    let initial = LifecycleFence {
        schema: FENCE_SCHEMA.to_string(),
        transaction: transaction.to_string(),
        status: "preparing".to_string(),
        queue: QueueFence {
            was_paused: prior.paused,
            drained: false,
            resumed: false,
            pause: (!prior.paused).then(|| QueueEffect {
                status: "pause_intent".to_string(),
                expected_version: prior_versioned
                    .as_ref()
                    .map(|versioned| versioned.version.clone()),
                expected_content: prior_versioned
                    .as_ref()
                    .map(|versioned| versioned.content.clone()),
                intended: crate::queue::control::QueueControl {
                    paused: true,
                    reason: format!("storage reconciliation {transaction}"),
                    since: Utc::now().to_rfc3339(),
                    by: "stado storage-root-reconcile".to_string(),
                },
                superseding: None,
            }),
            restoration: None,
        },
        resident_owner,
        writers,
        transport_retained,
        non_storage_retained,
        staged_runtime: Some(staged_runtime),
        roots: Some(roots),
        write_fence: None,
        preflight_evidence: None,
        rollback_preparation: false,
        lease_acquisitions: Vec::new(),
        repository_runner_gate,
        prepared_at: Utc::now().timestamp(),
        rechecked_at: 0,
        activated_at: None,
        activation_sha256: None,
        restored_at: None,
    };
    write_fence(storage_target, transaction, &initial, runner).await?;
    Ok(initial)
}
