use super::*;

pub(in crate::deploy::host_storage_reconcile) async fn typed_lifecycle_decisions(
    transaction: &str,
) -> Result<Vec<Value>, DeployError> {
    let receipt = read_transaction_receipt(transaction)?;
    let checkpoint_reference =
        receipt_evidence_reference(&receipt, "checkpoint_evidence", "checkpoint evidence")?;
    let checkpoint = read_json_evidence(
        transaction,
        CHECKPOINT_EVIDENCE_FILE,
        &checkpoint_reference,
        "checkpoint evidence",
    )?;
    if checkpoint.get("schema").and_then(Value::as_str)
        != Some("stado.storage-root-checkpoint-evidence.v1")
        || checkpoint.get("transaction").and_then(Value::as_str) != Some(transaction)
    {
        return Err(DeployError(
            "checkpoint evidence belongs to another reconciliation".to_string(),
        ));
    }
    let conflict_winner = checkpoint
        .get("conflict_winner")
        .and_then(Value::as_str)
        .filter(|winner| matches!(*winner, "primary" | "backup"))
        .ok_or_else(|| {
            DeployError("checkpoint evidence omitted its conflict winner".to_string())
        })?;
    if receipt.get("conflict_winner").and_then(Value::as_str) != Some(conflict_winner) {
        return Err(DeployError(
            "checkpoint receipt and evidence disagree on the conflict winner".to_string(),
        ));
    }
    let backup_paths = checkpoint
        .get("backup_objects")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("checkpoint evidence omitted backup objects".to_string()))?
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let primary_paths = checkpoint
        .get("primary_objects")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("checkpoint evidence omitted primary objects".to_string()))?
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let newly_authoritative = if conflict_winner == "primary" {
        backup_paths.difference(&primary_paths)
    } else {
        primary_paths.difference(&backup_paths)
    }
    .filter_map(|path| path.strip_prefix("ecosystem/probierz/"))
    .map(str::to_string)
    .collect::<Vec<_>>();
    let snapshot = transaction_directory(transaction)?.join("effective-lifecycle.checkpoint");
    if receipt
        .get("effective_lifecycle_checkpoint")
        .and_then(Value::as_str)
        .map(Path::new)
        != Some(snapshot.as_path())
    {
        return Err(DeployError(
            "checkpoint receipt does not name the resident immutable lifecycle snapshot"
                .to_string(),
        ));
    }
    let backend = crate::queue::LocalBackend::open_existing(&snapshot)
        .map_err(|error| DeployError(format!("cannot open lifecycle checkpoint: {error}")))?;
    let store = crate::queue::JobStorage::with_backend(
        std::sync::Arc::new(backend),
        "immutable-local-snapshot",
    );
    crate::monitor::reap::classify_reconciliation_snapshot(&store, &newly_authoritative)
        .await
        .map_err(|error| DeployError(format!("typed lifecycle snapshot refused: {error}")))
}

pub(in crate::deploy::host_storage_reconcile) async fn record_typed_lifecycle_decisions(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    decisions: &[Value],
    runner: &Runner,
) -> Result<Value, DeployError> {
    if !host_channel::target_is_this_host(target) {
        return Err(DeployError(
            "typed lifecycle decisions can only be recorded by the resident target worker"
                .to_string(),
        ));
    }
    write_json_evidence(
        transaction,
        LIFECYCLE_DECISIONS_FILE,
        decisions,
        "typed lifecycle decisions",
        false,
    )?;
    let output = host_channel::run_script_with_timeout(
        target,
        &bind_remote_script(RECORD_LIFECYCLE_DECISIONS, transaction),
        TIMEOUT,
        runner,
    )
    .await?;
    parse_remote_payload(&output)
}
pub(in crate::deploy::host_storage_reconcile) async fn typed_final_lifecycle_observations(
    transaction: &str,
    fence: &LifecycleFence,
) -> Result<Vec<Value>, DeployError> {
    let receipt = read_transaction_receipt(transaction)?;
    let decision_reference = receipt_evidence_reference(
        &receipt,
        "lifecycle_decisions_evidence",
        "typed lifecycle decisions",
    )?;
    let decisions_value = read_json_evidence(
        transaction,
        LIFECYCLE_DECISIONS_FILE,
        &decision_reference,
        "typed lifecycle decisions",
    )?;
    let decisions = decisions_value
        .as_array()
        .ok_or_else(|| DeployError("typed lifecycle decisions are not a list".to_string()))?;
    let snapshot = transaction_directory(transaction)?.join("effective-lifecycle.checkpoint");
    let backend = crate::queue::LocalBackend::open_existing(&snapshot)
        .map_err(|error| DeployError(format!("cannot open lifecycle checkpoint: {error}")))?;
    let snapshot_store = crate::queue::JobStorage::with_backend(
        std::sync::Arc::new(backend),
        "immutable-local-snapshot",
    );
    let live = recovered_object_store(fence)?;
    crate::monitor::reap::validate_reconciliation_final_state(&live, &snapshot_store, decisions)
        .await
        .map_err(|error| {
            DeployError(format!(
                "typed final lifecycle observation refused: {error}"
            ))
        })
}

pub(in crate::deploy::host_storage_reconcile) async fn record_typed_final_lifecycle_observations(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    observations: &[Value],
    runner: &Runner,
) -> Result<Value, DeployError> {
    if !host_channel::target_is_this_host(target) {
        return Err(DeployError(
            "typed final lifecycle observations can only be recorded by the resident target worker"
                .to_string(),
        ));
    }
    write_json_evidence(
        transaction,
        FINAL_LIFECYCLE_OBSERVATIONS_FILE,
        observations,
        "typed final lifecycle observations",
        false,
    )?;
    let output = host_channel::run_script_with_timeout(
        target,
        &bind_remote_script(FINALIZE, transaction),
        TIMEOUT,
        runner,
    )
    .await?;
    parse_remote_payload(&output)
}
