//! The preflight observation and the checkpoint: immutable evidence of both
//! roots, their sealed physical copies and the effective lifecycle snapshot.

use std::fs;

use serde_json::{json, Value};

use super::receipt::{emit, every_writer, load_receipt, read_fence, save, set, status};
use super::{conflict_winner_from_fence, load_checkpoint_evidence, text, Context, Step};
use crate::deploy::host_storage_reconcile_program::fs::{
    checkpoint_tree, complete_physical_inventory, item_path, object_paths,
    validate_complete_inventory, validate_physical_checkpoint,
};
use crate::deploy::host_storage_reconcile_program::json::{
    atomic_json, canonical, now, object, persist_immutable_json, truthy,
};
use crate::deploy::host_storage_reconcile_program::lifecycle::checkpoint_effective_lifecycle;
use crate::deploy::host_storage_reconcile_program::SCHEMA;

const HANDOFF_SCOPE: &str =
    "ecosystem/ qualified objects and matching .metadata/ecosystem sidecars";
/// Receipt states at or past a finished checkpoint.
const CHECKPOINTED: [&str; 7] = [
    "checkpoint_ready",
    "applying",
    "data_committed_pending_activation",
    "activation_effects_armed",
    "rollback_effects_armed",
    "activated_pending_lifecycle",
    "complete",
];

pub(super) fn preflight(context: &Context) -> Step<()> {
    let (backup_objects, backup_physical) = complete_physical_inventory(context, &context.backup)?;
    let (primary_objects, primary_physical) =
        complete_physical_inventory(context, &context.primary)?;
    let observed = json!({
        "schema": SCHEMA,
        "transaction": context.tx,
        "status": "observed",
        "observed_at": now()?,
        "backup_qualified": backup_objects,
        "primary_qualified": primary_objects,
        "backup_physical": backup_physical,
        "primary_physical": primary_physical,
        "physical_snapshot_exclusions": [],
    });
    println!("STADO_STORAGE_RECONCILE\t{}", canonical(&observed));
    Ok(())
}

fn fence_is_complete(context: &Context, fence: &Value) -> bool {
    text(fence, "schema") == Some(super::FENCE_SCHEMA)
        && text(fence, "transaction") == Some(context.tx.as_str())
        && text(fence, "status") == Some("fenced")
        && truthy(object(fence, "queue").get("drained"))
        && truthy(object(fence, "staged_runtime").get("staged_sha256"))
        && object(fence, "write_fence")
            .get("status")
            .and_then(Value::as_str)
            == Some("acquired")
        && truthy(fence.get("preflight_evidence"))
        && truthy(fence.get("rechecked_at"))
}

fn paths_of(objects: &[Value]) -> Step<Vec<String>> {
    objects
        .iter()
        .map(|item| item_path(item).map(str::to_string))
        .collect()
}

/// Both roots as the checkpoint captures them.
struct Captured {
    backup_objects: Vec<Value>,
    primary_objects: Vec<Value>,
    backup_physical: Value,
    primary_physical: Value,
    winner: String,
}

/// A new checkpoint: inventory both roots, publish the immutable evidence
/// and start the receipt.
fn begin(context: &Context, fence: &Value, winner: String) -> Step<(Value, Captured)> {
    let (backup_objects, backup_physical) = complete_physical_inventory(context, &context.backup)?;
    let (primary_objects, primary_physical) =
        complete_physical_inventory(context, &context.primary)?;
    let evidence = json!({
        "schema": super::EVIDENCE_SCHEMA,
        "transaction": context.tx,
        "source": context.backup,
        "destination": context.primary,
        "conflict_winner": winner,
        "backup_objects": backup_objects,
        "primary_objects": primary_objects,
        "backup_physical": backup_physical,
        "primary_physical": primary_physical,
        "physical_snapshot_exclusions": [],
        "snapshot_scope": "full_physical_roots",
        "handoff_scope": HANDOFF_SCOPE,
    });
    let reference = persist_immutable_json(
        &context.checkpoint_evidence_path,
        &evidence,
        "checkpoint evidence",
    )?;
    let files = |physical: &Value| physical["files"].as_array().map_or(0, Vec::len);
    let receipt = json!({
        "schema": SCHEMA,
        "transaction": context.tx,
        "status": "checkpointing",
        "source": context.backup,
        "destination": context.primary,
        "backup_checkpoint": context.backup_snapshot,
        "primary_checkpoint": context.primary_snapshot,
        "effective_lifecycle_checkpoint": context.effective_lifecycle_snapshot,
        "checkpoint_started_at": now()?,
        "writer_fence": fence,
        "checkpoint_evidence": reference,
        "conflict_winner": winner,
        "backup_objects": backup_objects.len(),
        "primary_objects": primary_objects.len(),
        "backup_physical_files": files(&backup_physical),
        "primary_physical_files": files(&primary_physical),
        "physical_snapshot_exclusions": [],
        "snapshot_scope": "full_physical_roots",
        "handoff_scope": HANDOFF_SCOPE,
    });
    atomic_json(&context.receipt_path, &receipt)?;
    let captured = Captured {
        backup_objects,
        primary_objects,
        backup_physical,
        primary_physical,
        winner,
    };
    Ok((receipt, captured))
}

/// An interrupted checkpoint: both live roots must still be exactly what
/// its immutable evidence recorded.
fn resume(context: &Context, receipt: &Value, winner: &str) -> Step<Captured> {
    let evidence = load_checkpoint_evidence(context, receipt)?;
    if evidence.conflict_winner != winner {
        return Err(
            "checkpoint conflict winner differs from the durable lifecycle fence".to_string(),
        );
    }
    if paths_of(&evidence.backup_objects)? != object_paths(&context.backup)? {
        return Err(
            "backup qualified namespace no longer matches the interrupted checkpoint".to_string(),
        );
    }
    if paths_of(&evidence.primary_objects)? != object_paths(&context.primary)? {
        return Err(
            "primary qualified namespace no longer matches the interrupted checkpoint".to_string(),
        );
    }
    let since = "since checkpoint start";
    validate_complete_inventory(
        context,
        &context.backup,
        &evidence.backup_objects,
        &format!("backup qualified namespace {since}"),
    )?;
    validate_complete_inventory(
        context,
        &context.primary,
        &evidence.primary_objects,
        &format!("primary qualified namespace {since}"),
    )?;
    validate_physical_checkpoint(
        context,
        &context.backup,
        &evidence.backup_physical,
        &format!("backup {since}"),
    )?;
    validate_physical_checkpoint(
        context,
        &context.primary,
        &evidence.primary_physical,
        &format!("primary {since}"),
    )?;
    Ok(Captured {
        backup_objects: evidence.backup_objects,
        primary_objects: evidence.primary_objects,
        backup_physical: evidence.backup_physical,
        primary_physical: evidence.primary_physical,
        winner: evidence.conflict_winner,
    })
}

pub(super) fn checkpoint(context: &Context) -> Step<()> {
    let fence = read_fence(context, "durable lifecycle fence is absent or unreadable")?;
    if !fence_is_complete(context, &fence) {
        return Err("durable lifecycle fence is incomplete".to_string());
    }
    if !every_writer(&fence, "stopped")? {
        return Err("durable lifecycle fence does not stop every recorded writer".to_string());
    }
    let fence_winner = conflict_winner_from_fence(context, &fence)?;
    let existing = match fs::symlink_metadata(&context.receipt_path) {
        Ok(_) => Some(load_receipt(context)?),
        Err(_) => None,
    };
    let (mut receipt, captured) = match existing {
        Some(receipt) if CHECKPOINTED.contains(&status(&receipt)) => {
            if text(&receipt, "conflict_winner") != Some(fence_winner.as_str()) {
                return Err(
                    "checkpoint conflict winner differs from the durable lifecycle fence"
                        .to_string(),
                );
            }
            emit(context, &receipt);
            return Ok(());
        }
        Some(receipt) if status(&receipt) != "checkpointing" => {
            return Err(format!(
                "checkpoint receipt is not resumable: {}",
                status(&receipt)
            ));
        }
        Some(receipt) => {
            let captured = resume(context, &receipt, &fence_winner)?;
            (receipt, captured)
        }
        None => begin(context, &fence, fence_winner)?,
    };
    let (backup, primary) = (&context.backup, &context.primary);
    checkpoint_tree(
        context,
        backup,
        &context.backup_snapshot,
        &captured.backup_physical,
    )?;
    checkpoint_tree(
        context,
        primary,
        &context.primary_snapshot,
        &captured.primary_physical,
    )?;
    let after = "after checkpoint";
    validate_physical_checkpoint(
        context,
        backup,
        &captured.backup_physical,
        &format!("backup {after}"),
    )?;
    validate_physical_checkpoint(
        context,
        primary,
        &captured.primary_physical,
        &format!("primary {after}"),
    )?;
    validate_complete_inventory(
        context,
        backup,
        &captured.backup_objects,
        &format!("backup qualified namespace {after}"),
    )?;
    validate_complete_inventory(
        context,
        primary,
        &captured.primary_objects,
        &format!("primary qualified namespace {after}"),
    )?;
    checkpoint_effective_lifecycle(
        context,
        &captured.primary_objects,
        &captured.backup_objects,
        &captured.winner,
    )?;
    set(&mut receipt, "status", json!("checkpoint_ready"));
    set(&mut receipt, "checkpointed_at", now()?);
    save(context, &receipt)
}
