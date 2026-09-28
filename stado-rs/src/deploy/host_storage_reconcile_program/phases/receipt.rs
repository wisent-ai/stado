//! The transaction receipt: its summary line, the three read phases (the
//! lifecycle fence, the operation owner with its token withheld, the receipt
//! summary), and the typed documents the Rust side records into it.

use std::fs;

use serde_json::{json, Value};

use crate::deploy::host_storage_reconcile_program::json::{
    canonical, immutable_json_file, now, object, read_json, truthy,
};
use crate::deploy::host_storage_reconcile_program::{Context, Step, SCHEMA};

use super::{FENCE_SCHEMA, OWNER_SCHEMA};

const SNAPSHOT_ENGINE: &str = "stado.typed-lifecycle-snapshot.v1";
const FINAL_ENGINE: &str = "stado.typed-lifecycle-final.v1";

/// The receipt summary line the worker parses.
pub(super) fn emit(context: &Context, receipt: &Value) {
    let decisions = object(receipt, "lifecycle_decision_counts");
    let count = |key: &str| receipt.get(key).cloned().unwrap_or(json!(0));
    let decided = |key: &str| decisions.get(key).cloned().unwrap_or(json!(0));
    let summary = json!({
        "schema": receipt.get("schema"),
        "transaction": receipt.get("transaction"),
        "status": receipt.get("status"),
        "receipt_path": context.receipt_path,
        "backup_checkpoint": receipt.get("backup_checkpoint"),
        "primary_checkpoint": receipt.get("primary_checkpoint"),
        "backup_objects": count("backup_objects"),
        "primary_objects": count("primary_objects"),
        "verified_objects": count("verified_objects"),
        "backup_physical_files": count("backup_physical_files"),
        "primary_physical_files": count("primary_physical_files"),
        "physical_snapshot_exclusions":
            receipt.get("physical_snapshot_exclusions").cloned().unwrap_or(json!([])),
        "lifecycle_decisions": {
            "queued_cancellation": decided("queued_cancellation"),
            "retained_outcome_cleanup": decided("retained_outcome_cleanup"),
        },
    });
    println!("STADO_STORAGE_RECONCILE\t{}", canonical(&summary));
}

/// Write the receipt durably, then report it.
pub(super) fn save(context: &Context, receipt: &Value) -> Step<()> {
    crate::deploy::host_storage_reconcile_program::json::atomic_json(
        &context.receipt_path,
        receipt,
    )?;
    emit(context, receipt);
    Ok(())
}

pub(super) fn status(receipt: &Value) -> &str {
    receipt
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

pub(super) fn set(receipt: &mut Value, key: &str, value: Value) {
    if let Some(fields) = receipt.as_object_mut() {
        fields.insert(key.to_string(), value);
    }
}

pub(super) fn load_receipt(context: &Context) -> Step<Value> {
    let receipt = match fs::read(&context.receipt_path) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
            .map_err(|error| format!("checkpoint receipt is unreadable: {error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err("checkpoint receipt is absent".to_string());
        }
        Err(error) => return Err(format!("checkpoint receipt is unreadable: {error}")),
    };
    if receipt.get("schema").and_then(Value::as_str) != Some(SCHEMA)
        || receipt.get("transaction").and_then(Value::as_str) != Some(context.tx.as_str())
    {
        return Err("checkpoint receipt belongs to another transaction".to_string());
    }
    Ok(receipt)
}

/// The durable lifecycle fence; `label` names the refusal when it cannot be read.
pub(super) fn read_fence(context: &Context, label: &str) -> Step<Value> {
    read_json(&context.fence_path).map_err(|error| format!("{label}: {error}"))
}

pub(super) fn read_fence_phase(context: &Context) -> Step<()> {
    let fence = match fs::read(&context.fence_path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("lifecycle fence is unreadable: {error}"))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({
            "schema": FENCE_SCHEMA, "transaction": context.tx, "status": "absent", "writers": [],
        }),
        Err(error) => return Err(format!("lifecycle fence is unreadable: {error}")),
    };
    println!("STADO_STORAGE_RECONCILE\t{}", canonical(&fence));
    Ok(())
}

pub(super) fn read_owner_phase(context: &Context) -> Step<()> {
    match fs::symlink_metadata(&context.owner_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("STADO_RECONCILE_OWNER\tabsent");
            return Ok(());
        }
        Err(error) => return Err(format!("operation owner is unreadable: {error}")),
        Ok(info) if !info.file_type().is_file() => {
            return Err("operation owner is not a regular file".to_string());
        }
        Ok(_) => {}
    }
    let mut owner = read_json(&context.owner_path)
        .map_err(|error| format!("operation owner is unreadable: {error}"))?;
    if owner.get("schema").and_then(Value::as_str) != Some(OWNER_SCHEMA)
        || owner.get("transaction").and_then(Value::as_str) != Some(context.tx.as_str())
    {
        return Err("operation owner identity is invalid".to_string());
    }
    if let Some(fields) = owner.as_object_mut() {
        fields.remove("token");
    }
    println!("STADO_RECONCILE_OWNER\t{}", canonical(&owner));
    Ok(())
}

pub(super) fn status_phase(context: &Context) -> Step<()> {
    if fs::metadata(&context.receipt_path).is_ok_and(|info| info.is_file()) {
        emit(context, &load_receipt(context)?);
    } else {
        let absent = json!({
            "schema": SCHEMA, "transaction": context.tx, "status": "absent",
            "receipt_path": context.receipt_path,
        });
        println!("STADO_STORAGE_RECONCILE\t{}", canonical(&absent));
    }
    Ok(())
}

/// Every recorded writer is in `wanted`; a fence with no writers has none out of it.
pub(super) fn every_writer(fence: &Value, wanted: &str) -> bool {
    fence
        .get("writers")
        .and_then(Value::as_array)
        .is_none_or(|writers| {
            writers
                .iter()
                .all(|writer| writer.get("status").and_then(Value::as_str) == Some(wanted))
        })
}

/// Record an immutable typed document (the lifecycle decisions or the final
/// observations) in the receipt once, with its validation proof.
fn record_typed(
    receipt: &mut Value,
    path: &str,
    label: &str,
    keys: (&str, &str),
    engine: &str,
) -> Step<Vec<Value>> {
    let (document, reference) = immutable_json_file(path, label)?;
    let Some(items) = document.as_array().cloned() else {
        return Err(format!("{label} are not a list"));
    };
    let (evidence_key, validation_key) = keys;
    let existing = receipt.get(evidence_key).filter(|value| !value.is_null());
    let validation = receipt.get(validation_key).filter(|value| !value.is_null());
    if existing.is_some() != validation.is_some() {
        return Err(format!(
            "{label} reference and validation proof are incomplete"
        ));
    }
    if existing.is_some_and(|existing| existing != &reference) {
        return Err(format!("{label} changed after their durable result"));
    }
    if validation.is_some_and(|validation| {
        validation.get("engine").and_then(Value::as_str) != Some(engine)
            || validation.get("sha256") != reference.get("sha256")
    }) {
        return Err(format!(
            "{label} validation proof changed after its durable result"
        ));
    }
    if validation.is_none() {
        let proof = json!({
            "engine": engine,
            "sha256": reference.get("sha256"),
            "validated_at": now()?,
        });
        set(receipt, validation_key, proof);
    }
    set(receipt, evidence_key, reference);
    Ok(items)
}

pub(super) fn record_lifecycle_decisions(context: &Context, mut receipt: Value) -> Step<()> {
    const BEFORE_ACTIVATION: [&str; 5] = [
        "checkpoint_ready",
        "applying",
        "data_committed_pending_activation",
        "activation_effects_armed",
        "rollback_effects_armed",
    ];
    if !BEFORE_ACTIVATION.contains(&status(&receipt)) {
        return Err(
            "lifecycle decisions require an immutable checkpoint before runtime activation"
                .to_string(),
        );
    }
    let decisions = record_typed(
        &mut receipt,
        &context.lifecycle_decisions_path,
        "typed lifecycle decisions",
        ("lifecycle_decisions_evidence", "lifecycle_validation"),
        SNAPSHOT_ENGINE,
    )?;
    let count = |kind: &str| {
        decisions
            .iter()
            .filter(|item| item.get("kind").and_then(Value::as_str) == Some(kind))
            .count()
    };
    let counts = json!({
        "queued_cancellation": count("queued_cancellation"),
        "retained_outcome_cleanup": count("retained_outcome_cleanup"),
    });
    set(&mut receipt, "lifecycle_decision_counts", counts);
    save(context, &receipt)
}

pub(super) fn finalize(context: &Context, mut receipt: Value) -> Step<()> {
    if !["activated_pending_lifecycle", "complete"].contains(&status(&receipt)) {
        return Err(format!(
            "reconciliation is not awaiting typed lifecycle finalization: {}",
            status(&receipt)
        ));
    }
    record_typed(
        &mut receipt,
        &context.final_lifecycle_observations_path,
        "typed final lifecycle observations",
        (
            "final_lifecycle_observations_evidence",
            "final_lifecycle_validation",
        ),
        FINAL_ENGINE,
    )?;
    set(&mut receipt, "status", json!("complete"));
    if !truthy(receipt.get("completed_at")) {
        set(&mut receipt, "completed_at", now()?);
    }
    set(&mut receipt, "canonical_recovery_verified", json!(true));
    save(context, &receipt)
}
