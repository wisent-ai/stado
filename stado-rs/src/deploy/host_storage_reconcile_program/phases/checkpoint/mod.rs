//! The preflight observation and the checkpoint: immutable evidence of both
//! roots, their sealed physical copies and the effective lifecycle snapshot.
//! What the checkpoint captures of the roots is read in `capture`.

mod capture;

use std::fs;

use serde_json::{json, Value};

use super::receipt::{
    emit, every_writer, load_receipt, read_fence, save, set, status, ReceiptStatus,
};
use super::{conflict_winner_from_fence, text, Context, Step};
use crate::deploy::host_storage_reconcile::FenceStatus;
use crate::deploy::host_storage_reconcile_program::fs::{
    checkpoint_tree, complete_physical_inventory, validate_complete_inventory,
    validate_physical_checkpoint,
};
use crate::deploy::host_storage_reconcile_program::json::{canonical, now, object, truthy};
use crate::deploy::host_storage_reconcile_program::lifecycle::checkpoint_effective_lifecycle;
use crate::deploy::host_storage_reconcile_program::SCHEMA;

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
        && FenceStatus::read(fence, "status") == Ok(FenceStatus::Fenced)
        && truthy(object(fence, "queue").get("drained"))
        && truthy(object(fence, "staged_runtime").get("staged_sha256"))
        && object(fence, "write_fence")
            .get("status")
            .and_then(Value::as_str)
            == Some("acquired")
        && truthy(fence.get("preflight_evidence"))
        && truthy(fence.get("rechecked_at"))
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
    // Only a receipt that is not there starts a new checkpoint; one that
    // cannot be inspected may record a finished one, and starting over would
    // write a fresh receipt over it.
    let existing = match fs::symlink_metadata(&context.receipt_path) {
        Ok(_) => Some(load_receipt(context)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "checkpoint receipt {} cannot be inspected: {error}",
                context.receipt_path
            ))
        }
    };
    let existing = match existing {
        Some(receipt) => {
            let current = status(&receipt)?;
            Some((receipt, current))
        }
        None => None,
    };
    let (mut receipt, captured) = match existing {
        Some((receipt, current)) if current.checkpointed() => {
            let pinned = crate::deploy::host_storage_reconcile::ConflictWinner::read(
                &receipt,
                "conflict_winner",
            )
            .map_err(|reason| format!("checkpoint receipt: {reason}"))?;
            if pinned != fence_winner {
                return Err(
                    "checkpoint conflict winner differs from the durable lifecycle fence"
                        .to_string(),
                );
            }
            emit(context, &receipt);
            return Ok(());
        }
        Some((_, current)) if current != ReceiptStatus::Checkpointing => {
            return Err(format!("checkpoint receipt is not resumable: {current}"));
        }
        Some((receipt, _)) => {
            let captured = capture::resume(context, &receipt, fence_winner)?;
            (receipt, captured)
        }
        None => capture::begin(context, &fence, fence_winner)?,
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
        captured.winner,
    )?;
    set(
        &mut receipt,
        "status",
        json!(ReceiptStatus::CheckpointReady),
    );
    set(&mut receipt, "checkpointed_at", now()?);
    save(context, &receipt)
}
