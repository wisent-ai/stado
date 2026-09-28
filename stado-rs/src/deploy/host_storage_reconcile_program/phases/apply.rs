//! The additive data commit from the backup checkpoint into live A, and the
//! activation record once the Rust side has restored every writer.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use serde_json::{json, Value};

use super::receipt::{emit, every_writer, read_fence, save, set, status};
use super::union::prove_live_additive_union;
use super::{conflict_winner_from_fence, text, Context, Evidence, Step, FENCE_SCHEMA};
use crate::deploy::host_storage_reconcile_program::fs::{
    clone_file, digest, fsync_dir, item_path, join, metadata_path, object_paths, parent_of,
    regular_identity, validate_complete_inventory,
};
use crate::deploy::host_storage_reconcile_program::json::{
    atomic_json, load_immutable_json, now, object, truthy,
};

const SNAPSHOT_ENGINE: &str = "stado.typed-lifecycle-snapshot.v1";
/// A SHA-256 digest written as lowercase hexadecimal.
const SHA256_HEX_CHARACTERS: usize = 64;

fn by_path(objects: &[Value]) -> Step<BTreeMap<String, Value>> {
    objects
        .iter()
        .map(|item| Ok((item_path(item)?.to_string(), item.clone())))
        .collect()
}

fn field(item: Option<&Value>, key: &str) -> Value {
    item.and_then(|item| item.get(key))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The recorded decisions are validated and none blocks activation.
fn require_decisions(context: &Context, receipt: &Value) -> Step<()> {
    let validation = receipt.get("lifecycle_validation");
    let reference = receipt.get("lifecycle_decisions_evidence");
    let proved = matches!((validation, reference), (Some(validation), Some(reference))
        if validation.get("engine").and_then(Value::as_str) == Some(SNAPSHOT_ENGINE)
            && reference.is_object()
            && validation.get("sha256") == reference.get("sha256"));
    if !proved {
        return Err("typed Rust lifecycle validation is absent".to_string());
    }
    let decisions = load_immutable_json(
        reference,
        &context.lifecycle_decisions_path,
        "typed lifecycle decisions",
    )?;
    let Some(decisions) = decisions.as_array() else {
        return Err("typed lifecycle decisions are not a list".to_string());
    };
    let blockers: Vec<&str> = decisions
        .iter()
        .filter(|item| item.get("kind").and_then(Value::as_str) == Some("block_unclassified_live"))
        .map(|item| item.get("path").and_then(Value::as_str).unwrap_or("?"))
        .collect();
    if !blockers.is_empty() {
        return Err(format!(
            "newly authoritative lifecycle state blocks activation: {}",
            blockers.join(", ")
        ));
    }
    Ok(())
}

/// Live A is between its checkpoint and the union, object by object, so an
/// interrupted apply can resume.
fn require_resumable_primary(context: &Context, evidence: &Evidence) -> Step<()> {
    let backup = by_path(&evidence.backup_objects)?;
    let primary = by_path(&evidence.primary_objects)?;
    let current: BTreeSet<String> = object_paths(&context.primary)?.into_iter().collect();
    let expected: BTreeSet<String> = primary.keys().chain(backup.keys()).cloned().collect();
    if !primary.keys().all(|path| current.contains(path)) || !current.is_subset(&expected) {
        return Err(
            "primary namespace drifted outside the resumable additive transition".to_string(),
        );
    }
    for relative in &expected {
        let alias = format!("{relative}.json");
        if expected.contains(&alias) {
            return Err(format!(
                "object metadata alias collision: {relative} and {alias}"
            ));
        }
        let body = regular_identity(context, &join(&context.primary, relative))?;
        let metadata = regular_identity(context, &metadata_path(&context.primary, relative)?)?;
        let before = primary.get(relative);
        let candidates: Vec<Option<&Value>> =
            if evidence.conflict_winner == "primary" && before.is_some() {
                vec![before]
            } else {
                let mut present: Vec<Option<&Value>> = [before, backup.get(relative)]
                    .into_iter()
                    .filter(Option::is_some)
                    .collect();
                if before.is_none() {
                    present.push(None);
                }
                present
            };
        let allowed = |key: &str, value: &Value| {
            candidates
                .iter()
                .any(|candidate| &field(*candidate, key) == value)
        };
        if !allowed("body", &body) || !allowed("metadata", &metadata) {
            return Err(format!(
                "primary object drifted after its checkpoint: {relative}"
            ));
        }
    }
    Ok(())
}

/// Bring one backup-checkpoint object into live A as the conflict rule wants it.
fn apply_object(
    context: &Context,
    item: &Value,
    before: Option<&Value>,
    primary_wins: bool,
) -> Step<()> {
    let relative = item_path(item)?;
    let desired = before.filter(|_| primary_wins).unwrap_or(item);
    let destination = join(&context.primary, relative);
    let current = regular_identity(context, &destination)?;
    if current != field(Some(desired), "body") {
        if current != field(before, "body") {
            return Err(format!(
                "primary body changed outside the transaction: {relative}"
            ));
        }
        clone_file(
            context,
            &join(&context.backup_snapshot, relative),
            &destination,
        )?;
    }
    let destination_meta = metadata_path(&context.primary, relative)?;
    let desired_meta = field(Some(desired), "metadata");
    let current_meta = regular_identity(context, &destination_meta)?;
    if current_meta != desired_meta {
        if current_meta != field(before, "metadata") {
            return Err(format!(
                "primary metadata changed outside the transaction: {relative}"
            ));
        }
        if !desired_meta.is_null() {
            let source = metadata_path(&context.backup_snapshot, relative)?;
            clone_file(context, &source, &destination_meta)?;
        } else if fs::metadata(&destination_meta).is_ok() {
            fs::remove_file(&destination_meta)
                .map_err(|error| format!("cannot remove {destination_meta}: {error}"))?;
            fsync_dir(&parent_of(&destination_meta))?;
        }
    }
    if regular_identity(context, &destination)? != field(Some(desired), "body") {
        return Err(format!("destination body did not verify: {relative}"));
    }
    if regular_identity(context, &destination_meta)? != desired_meta {
        return Err(format!("destination metadata did not verify: {relative}"));
    }
    Ok(())
}

pub(super) fn apply(context: &Context, mut receipt: Value, evidence: &Evidence) -> Step<()> {
    let applicable = [
        "checkpoint_ready",
        "applying",
        "data_committed_pending_activation",
    ];
    if !applicable.contains(&status(&receipt)) {
        return Err(format!(
            "checkpoint receipt is not applicable: {}",
            status(&receipt)
        ));
    }
    if status(&receipt) == "data_committed_pending_activation" {
        prove_live_additive_union(context, evidence, "while resuming committed data")?;
        emit(context, &receipt);
        return Ok(());
    }
    let fence = read_fence(context, "durable lifecycle fence cannot be rechecked")?;
    if conflict_winner_from_fence(context, &fence)? != evidence.conflict_winner {
        return Err("pinned conflict winner differs from the durable lifecycle fence".to_string());
    }
    let seconds = |value: &Value, key: &str| value.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    if text(&fence, "status") != Some("fenced")
        || !truthy(object(&fence, "queue").get("drained"))
        || !every_writer(&fence, "stopped")
        || seconds(&fence, "rechecked_at") < seconds(&receipt, "checkpointed_at")
    {
        return Err("lifecycle fence was not rechecked after checkpoint".to_string());
    }
    require_decisions(context, &receipt)?;
    validate_complete_inventory(
        context,
        &context.backup,
        &evidence.backup_objects,
        "live B after checkpoint",
    )?;
    require_resumable_primary(context, evidence)?;
    set(&mut receipt, "status", json!("applying"));
    atomic_json(&context.receipt_path, &receipt)?;
    let primary = by_path(&evidence.primary_objects)?;
    let primary_wins = evidence.conflict_winner == "primary";
    for item in &evidence.backup_objects {
        let before = primary.get(item_path(item)?);
        apply_object(context, item, before, primary_wins)?;
    }
    prove_live_additive_union(context, evidence, "after apply")?;
    let union: BTreeSet<String> = by_path(&evidence.backup_objects)?
        .into_keys()
        .chain(primary.into_keys())
        .collect();
    set(
        &mut receipt,
        "status",
        json!("data_committed_pending_activation"),
    );
    set(&mut receipt, "data_committed_at", now()?);
    set(&mut receipt, "verified_objects", json!(union.len()));
    set(
        &mut receipt,
        "conflict_winner",
        json!(evidence.conflict_winner),
    );
    set(&mut receipt, "primary_only_preserved", json!(true));
    set(&mut receipt, "backup_objects_not_written", json!(true));
    save(context, &receipt)
}

pub(super) fn activate(context: &Context, mut receipt: Value) -> Step<()> {
    if status(&receipt) == "activated_pending_lifecycle" {
        emit(context, &receipt);
        return Ok(());
    }
    if status(&receipt) != "activation_effects_armed" {
        return Err(format!(
            "reconciliation has no durable activation-effect boundary: {}",
            status(&receipt)
        ));
    }
    let fence = read_fence(context, "activated lifecycle fence cannot be read")?;
    let active = format!("{}/.stado/bin/stado", context.home);
    let expected = text(&fence, "activation_sha256")
        .unwrap_or_default()
        .to_string();
    let runtime_is_active = expected.len() == SHA256_HEX_CHARACTERS
        && fs::symlink_metadata(&active).is_ok_and(|info| info.is_file())
        && digest(context, &active)? == expected;
    if text(&fence, "schema") != Some(FENCE_SCHEMA)
        || text(&fence, "status") != Some("activated")
        || !truthy(object(&fence, "queue").get("resumed"))
        || !truthy(fence.get("restored_at"))
        || object(&fence, "write_fence")
            .get("status")
            .and_then(Value::as_str)
            != Some("released")
        || !runtime_is_active
    {
        return Err(
            "runtime activation and lifecycle restoration are not durably proved".to_string(),
        );
    }
    if !every_writer(&fence, "restored") {
        return Err(
            "activated fence does not restore every captured native service state".to_string(),
        );
    }
    let activated_at = fence.get("activated_at").cloned().unwrap_or(Value::Null);
    set(&mut receipt, "status", json!("activated_pending_lifecycle"));
    set(&mut receipt, "activated_at", activated_at);
    set(&mut receipt, "activated_sha256", json!(expected));
    save(context, &receipt)
}
