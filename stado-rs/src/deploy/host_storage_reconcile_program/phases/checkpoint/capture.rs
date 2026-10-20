//! What a checkpoint captures of both roots: a new checkpoint's inventory,
//! published as immutable evidence with the receipt it starts, or an
//! interrupted one's evidence, proved still true of both live roots.

use serde_json::{json, Value};

use super::super::receipt::ReceiptStatus;
use super::super::{load_checkpoint_evidence, Context, Step, EVIDENCE_SCHEMA};
use crate::deploy::host_storage_reconcile::ConflictWinner;
use crate::deploy::host_storage_reconcile_program::fs::{
    complete_physical_inventory, item_path, object_paths, validate_complete_inventory,
    validate_physical_checkpoint,
};
use crate::deploy::host_storage_reconcile_program::json::{
    atomic_json, now, persist_immutable_json,
};
use crate::deploy::host_storage_reconcile_program::SCHEMA;

const HANDOFF_SCOPE: &str =
    "ecosystem/ qualified objects and matching .metadata/ecosystem sidecars";

fn paths_of(objects: &[Value]) -> Step<Vec<String>> {
    objects
        .iter()
        .map(|item| item_path(item).map(str::to_string))
        .collect()
}

/// Both roots as the checkpoint captures them.
pub(super) struct Captured {
    pub(super) backup_objects: Vec<Value>,
    pub(super) primary_objects: Vec<Value>,
    pub(super) backup_physical: Value,
    pub(super) primary_physical: Value,
    pub(super) winner: ConflictWinner,
}

/// A new checkpoint: inventory both roots, publish the immutable evidence
/// and start the receipt.
pub(super) fn begin(
    context: &Context,
    fence: &Value,
    winner: ConflictWinner,
) -> Step<(Value, Captured)> {
    let (backup_objects, backup_physical) = complete_physical_inventory(context, &context.backup)?;
    let (primary_objects, primary_physical) =
        complete_physical_inventory(context, &context.primary)?;
    let evidence = json!({
        "schema": EVIDENCE_SCHEMA,
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
    // A physical inventory without its files list is refused, not counted as
    // a root holding no files in the receipt the rollback reads.
    let files = |physical: &Value, label: &str| {
        physical["files"]
            .as_array()
            .map(Vec::len)
            .ok_or_else(|| format!("the {label} physical inventory carries no files list"))
    };
    let receipt = json!({
        "schema": SCHEMA,
        "transaction": context.tx,
        "status": ReceiptStatus::Checkpointing,
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
        "backup_physical_files": files(&backup_physical, "backup")?,
        "primary_physical_files": files(&primary_physical, "primary")?,
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
pub(super) fn resume(context: &Context, receipt: &Value, winner: ConflictWinner) -> Step<Captured> {
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
