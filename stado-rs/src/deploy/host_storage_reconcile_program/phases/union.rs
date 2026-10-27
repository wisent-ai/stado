//! The proof that live A is the additive union of the two checkpoints, and
//! the two arming phases: activation (after the data commit) and rollback
//! (before it, restoring A exactly).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

mod restore;

use serde_json::{json, Value};

use super::receipt::{emit, every_writer, read_fence, save, set, status};
use super::{conflict_winner_from_fence, Context, Evidence, Step};
use crate::deploy::host_storage_reconcile_program::fs::{
    fsync_dir, item_path, join, metadata_path, object_paths, parent_of, physical_inventory,
    regular_identity, validate_complete_inventory, validate_physical_checkpoint,
};
use crate::deploy::host_storage_reconcile_program::json::{flag, list, now};
use restore::{directories, restore_file};

fn by_path(objects: &[Value]) -> Step<BTreeMap<String, Value>> {
    objects
        .iter()
        .map(|item| Ok((item_path(item)?.to_string(), item.clone())))
        .collect()
}

/// A member of a checkpointed item, or null for an item the checkpoint did
/// not record or a member it lacks: the comparison below reads null as
/// "absent on that side", which is what both mean.
fn field(item: Option<&Value>, key: &str) -> Value {
    match item.and_then(|item| item.get(key)) {
        Some(member) => member.clone(),
        None => Value::Null,
    }
}

/// Live B is still its checkpoint and live A holds exactly the union of
/// both checkpoints, each object as the conflict rule chose it.
pub(super) fn prove_live_additive_union(
    context: &Context,
    evidence: &Evidence,
    label: &str,
) -> Step<()> {
    let backup = by_path(&evidence.backup_objects)?;
    let primary = by_path(&evidence.primary_objects)?;
    let primary_wins = matches!(
        evidence.conflict_winner,
        crate::deploy::host_storage_reconcile::ConflictWinner::Primary
    );
    validate_complete_inventory(
        context,
        &context.backup,
        &evidence.backup_objects,
        &format!("live B {label}"),
    )?;
    let expected: BTreeSet<&String> = primary.keys().chain(backup.keys()).collect();
    let live = object_paths(&context.primary)?;
    if live.iter().collect::<BTreeSet<_>>() != expected {
        return Err(format!(
            "primary namespace does not equal the additive checkpoint union {label}"
        ));
    }
    for relative in expected {
        let chosen = primary
            .get(relative)
            .filter(|_| primary_wins)
            .or_else(|| backup.get(relative))
            .or_else(|| primary.get(relative));
        let body = regular_identity(context, &join(&context.primary, relative))?;
        if body != field(chosen, "body") {
            return Err(format!(
                "primary body differs from additive checkpoint {label}: {relative}"
            ));
        }
        let metadata = regular_identity(context, &metadata_path(&context.primary, relative)?)?;
        if metadata != field(chosen, "metadata") {
            return Err(format!(
                "primary metadata differs from additive checkpoint {label}: {relative}"
            ));
        }
    }
    Ok(())
}

fn restore_primary_checkpoint(context: &Context, evidence: &Evidence) -> Step<()> {
    let backup = by_path(&evidence.backup_objects)?;
    let primary = by_path(&evidence.primary_objects)?;
    let primary_wins = matches!(
        evidence.conflict_winner,
        crate::deploy::host_storage_reconcile::ConflictWinner::Primary
    );
    let mut files = BTreeMap::new();
    for file in list(
        &evidence.primary_physical,
        "files",
        "primary physical checkpoint",
    )? {
        files.insert(item_path(file)?.to_string(), file.clone());
    }
    let paths: BTreeSet<&String> = primary.keys().chain(backup.keys()).collect();
    for relative in paths {
        let before = primary.get(relative);
        let applied = before
            .filter(|_| primary_wins)
            .or(backup.get(relative))
            .or(before);
        let destination = join(&context.primary, relative);
        let current = regular_identity(context, &destination)?;
        let before_body = field(before, "body");
        if current != before_body && current != field(applied, "body") {
            return Err(format!("primary body changed outside rollback: {relative}"));
        }
        if current != before_body {
            restore_file(
                context,
                &destination,
                &join(&context.primary_snapshot, relative),
                &before_body,
                files.get(relative.as_str()),
                &format!("primary physical checkpoint omitted body: {relative}"),
            )?;
        }
        let destination_metadata = metadata_path(&context.primary, relative)?;
        let current_metadata = regular_identity(context, &destination_metadata)?;
        let before_metadata = field(before, "metadata");
        if current_metadata != before_metadata && current_metadata != field(applied, "metadata") {
            return Err(format!(
                "primary metadata changed outside rollback: {relative}"
            ));
        }
        if current_metadata != before_metadata {
            // The metadata file sits beside its object under the primary; a
            // path outside it is a defect of this program, said as such.
            let metadata_relative =
                match destination_metadata.strip_prefix(&format!("{}/", context.primary)) {
                    Some(relative) => relative,
                    None => {
                        return Err(format!(
                            "metadata path {destination_metadata} is not under the primary {}",
                            context.primary
                        ))
                    }
                };
            restore_file(
                context,
                &destination_metadata,
                &metadata_path(&context.primary_snapshot, relative)?,
                &before_metadata,
                files.get(metadata_relative),
                &format!("primary physical checkpoint omitted metadata: {relative}"),
            )?;
        }
    }
    let before_directories =
        directories(&evidence.primary_physical, "primary physical checkpoint")?;
    let current_directories = directories(
        &physical_inventory(context, &context.primary)?,
        "live primary physical inventory",
    )?;
    let mut extra: Vec<&String> = current_directories
        .difference(&before_directories)
        .collect();
    // Deepest first, so a directory is empty when it is removed.
    extra.sort_by_key(|path| std::cmp::Reverse((path.matches('/').count(), path.len())));
    for relative in extra {
        let path = join(&context.primary, relative);
        fs::remove_dir(&path).map_err(|error| {
            format!("transaction-created directory cannot be rolled back: {path}: {error}")
        })?;
        fsync_dir(&parent_of(&path)?)?;
    }
    validate_complete_inventory(
        context,
        &context.primary,
        &evidence.primary_objects,
        "live A after rollback",
    )?;
    if physical_inventory(context, &context.primary)? != evidence.primary_physical {
        return Err("live physical A differs from its exact rollback checkpoint".to_string());
    }
    Ok(())
}

/// The fence pins the same conflict winner; whether it is still fenced,
/// drained and has every writer stopped.
fn fence_holds(context: &Context, evidence: &Evidence, fence: &Value) -> Step<bool> {
    if conflict_winner_from_fence(context, fence)? != evidence.conflict_winner {
        return Err("pinned conflict winner differs from the durable lifecycle fence".to_string());
    }
    let stopped = every_writer(fence, "stopped")?;
    use crate::deploy::host_storage_reconcile::FenceStatus;
    Ok(FenceStatus::read(fence, "status")? == FenceStatus::Fenced
        && flag(fence.pointer("/queue/drained"), "queue.drained")?
        && stopped)
}

pub(super) fn arm_activation(
    context: &Context,
    mut receipt: Value,
    evidence: &Evidence,
) -> Step<()> {
    use crate::deploy::host_storage_reconcile::ReceiptStatus;
    let current = status(&receipt)?;
    if current == ReceiptStatus::ActivationEffectsArmed {
        emit(context, &receipt);
        return Ok(());
    }
    if current != ReceiptStatus::DataCommittedPendingActivation {
        return Err("activation effects require a committed frozen union".to_string());
    }
    let fence = read_fence(context, "activation fence cannot be rechecked")?;
    if !fence_holds(context, evidence, &fence)? {
        return Err("activation effects require every writer to remain stopped".to_string());
    }
    prove_live_additive_union(context, evidence, "at activation-effect boundary")?;
    set(
        &mut receipt,
        "status",
        json!(ReceiptStatus::ActivationEffectsArmed),
    );
    set(&mut receipt, "activation_effect_boundary_at", now()?);
    save(context, &receipt)
}

pub(super) fn arm_rollback(context: &Context, mut receipt: Value, evidence: &Evidence) -> Step<()> {
    use crate::deploy::host_storage_reconcile::ReceiptStatus;
    let current = status(&receipt)?;
    if current == ReceiptStatus::RollbackEffectsArmed {
        emit(context, &receipt);
        return Ok(());
    }
    if !current.before_data_commit() {
        return Err("rollback is safe only before the data-commit boundary".to_string());
    }
    let fence = read_fence(context, "rollback fence cannot be rechecked")?;
    if !fence_holds(context, evidence, &fence)? {
        return Err("rollback effects require every writer to remain stopped".to_string());
    }
    restore_primary_checkpoint(context, evidence)?;
    validate_complete_inventory(
        context,
        &context.backup,
        &evidence.backup_objects,
        "live B before rollback",
    )?;
    validate_physical_checkpoint(
        context,
        &context.backup,
        &evidence.backup_physical,
        "live physical B before rollback",
    )?;
    set(&mut receipt, "primary_checkpoint_restored_at", now()?);
    set(
        &mut receipt,
        "status",
        json!(ReceiptStatus::RollbackEffectsArmed),
    );
    set(&mut receipt, "rollback_effect_boundary_at", now()?);
    save(context, &receipt)
}
