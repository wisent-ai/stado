//! Phase dispatch, in the order the transaction has always checked things:
//! the lock proof for every effect, the three reads, the storage write fence,
//! the preflight, the fixed roots and Darwin, the checkpoint, then the phases
//! that need an immutable checkpoint.

mod apply;
mod checkpoint;
mod receipt;
mod union;

use std::fs;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use serde_json::{json, Value};

use super::fs::{real_directory, validate_complete_inventory};
use super::json::{load_immutable_json, object, read_json, truthy};
use super::{Context, Step};

const FENCE_SCHEMA: &str = "stado.storage-root-fence.v5";
const OWNER_SCHEMA: &str = "stado.storage-root-owner.v1";
const WRITE_FENCE_SCHEMA: &str = "stado.storage-root-write-fence.v1";
const EVIDENCE_SCHEMA: &str = "stado.storage-root-checkpoint-evidence.v1";

const READ_PHASES: [&str; 3] = ["read-fence", "read-owner", "status"];
const FENCED_PHASES: [&str; 5] = [
    "preflight",
    "checkpoint",
    "apply",
    "arm-activation",
    "arm-rollback",
];

/// What the immutable checkpoint evidence recorded of both roots.
struct Evidence {
    backup_objects: Vec<Value>,
    primary_objects: Vec<Value>,
    backup_physical: Value,
    primary_physical: Value,
    conflict_winner: String,
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// The inherited descriptor is the transaction lock and the owner record
/// names this token as executing; then the lock is taken again on it, which
/// the same open file description already holds.
fn prove_lock(context: &mut Context) -> Step<()> {
    let unavailable =
        |error: String| format!("resident transaction lock proof is unavailable: {error}");
    let fd: i32 = std::env::var("STADO_RECONCILE_LOCK_FD")
        .unwrap_or_else(|_| "-1".to_string())
        .parse()
        .map_err(|error| unavailable(format!("{error}")))?;
    if fd < 0 {
        return Err(unavailable("no lock descriptor was inherited".to_string()));
    }
    // SAFETY: fstat only reads the status of the inherited descriptor.
    let descriptor = nix::sys::stat::fstat(unsafe { BorrowedFd::borrow_raw(fd) })
        .map_err(|error| unavailable(error.to_string()))?;
    let canonical =
        fs::symlink_metadata(&context.lock_path).map_err(|error| unavailable(error.to_string()))?;
    let owner = read_json(&context.owner_path).map_err(unavailable)?;
    #[allow(clippy::unnecessary_cast)] // dev_t and ino_t differ in width across systems
    let same_lock =
        descriptor.st_dev as u64 == canonical.dev() && descriptor.st_ino as u64 == canonical.ino();
    if !same_lock
        || text(&owner, "schema") != Some(OWNER_SCHEMA)
        || text(&owner, "transaction") != Some(context.tx.as_str())
        || text(&owner, "token") != Some(context.owner_token.as_str())
        || text(&owner, "status") != Some("executing")
    {
        return Err(
            "resident transaction owner and inherited OS lock do not authorize this effect"
                .to_string(),
        );
    }
    // SAFETY: `fd` was proved above to be the open transaction lock.
    if unsafe { nix::libc::flock(fd, nix::libc::LOCK_EX | nix::libc::LOCK_NB) } != 0 {
        return Err(unavailable(std::io::Error::last_os_error().to_string()));
    }
    context.lock_fd = Some(fd);
    Ok(())
}

/// The recorded write fence is acquired, drained and still held exclusively.
fn require_storage_write_fence(context: &Context) -> Step<()> {
    let unobserved = |error: String| format!("storage write fence cannot be observed: {error}");
    let lock = format!("{}/storage-root-writes.lock", context.recovery);
    let intent_path = format!("{}/storage-root-write-fence.json", context.recovery);
    let lifecycle = read_json(&context.fence_path).map_err(unobserved)?;
    let intent = read_json(&intent_path).map_err(unobserved)?;
    let effect = object(&lifecycle, "write_fence");
    if text(&lifecycle, "schema") != Some(FENCE_SCHEMA)
        || text(&lifecycle, "transaction") != Some(context.tx.as_str())
        || effect.get("status").and_then(Value::as_str) != Some("acquired")
        || effect.get("intent") != Some(&intent)
        || text(&intent, "schema") != Some(WRITE_FENCE_SCHEMA)
        || text(&intent, "transaction") != Some(context.tx.as_str())
        || !truthy(object(&lifecycle, "queue").get("drained"))
    {
        return Err(
            "physical inventory requires the recorded drained storage write fence".to_string(),
        );
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&lock)
        .map_err(|error| unobserved(error.to_string()))?;
    // SAFETY: `file` stays open for the duration of the call.
    let shared =
        unsafe { nix::libc::flock(file.as_raw_fd(), nix::libc::LOCK_SH | nix::libc::LOCK_NB) };
    if shared == 0 {
        return Err("storage write-fence intent has no active exclusive hold".to_string());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() != Some(nix::libc::EWOULDBLOCK) {
        return Err(unobserved(error.to_string()));
    }
    Ok(())
}

/// Which root's bytes win a conflict: the root that was primary before.
fn conflict_winner_from_fence(context: &Context, fence: &Value) -> Step<String> {
    let roots = object(fence, "roots");
    let prior = roots.get("prior_primary").and_then(Value::as_str);
    let root = |key: &str| roots.get(key).and_then(Value::as_str);
    if text(fence, "schema") != Some(FENCE_SCHEMA)
        || text(fence, "transaction") != Some(context.tx.as_str())
        || root("primary") != Some(context.primary.as_str())
        || root("backup") != Some(context.backup.as_str())
        || !matches!(prior, Some(prior) if prior == context.primary || prior == context.backup)
    {
        return Err("storage conflict winner is invalid".to_string());
    }
    let winner = if prior == Some(context.primary.as_str()) {
        "primary"
    } else {
        "backup"
    };
    Ok(winner.to_string())
}

fn load_checkpoint_evidence(context: &Context, receipt: &Value) -> Step<Evidence> {
    let evidence = load_immutable_json(
        receipt.get("checkpoint_evidence"),
        &context.checkpoint_evidence_path,
        "checkpoint evidence",
    )?;
    let winner = text(&evidence, "conflict_winner");
    if text(&evidence, "schema") != Some(EVIDENCE_SCHEMA)
        || text(&evidence, "transaction") != Some(context.tx.as_str())
        || text(&evidence, "source") != Some(context.backup.as_str())
        || text(&evidence, "destination") != Some(context.primary.as_str())
        || !matches!(winner, Some("primary") | Some("backup"))
        || receipt.get("conflict_winner") != evidence.get("conflict_winner")
    {
        return Err("checkpoint evidence belongs to another reconciliation".to_string());
    }
    let physical = |key: &str| evidence.get(key).filter(|value| value.is_object()).cloned();
    let objects = |key: &str| evidence.get(key).and_then(Value::as_array).cloned();
    let (
        Some(backup_objects),
        Some(primary_objects),
        Some(backup_physical),
        Some(primary_physical),
    ) = (
        objects("backup_objects"),
        objects("primary_objects"),
        physical("backup_physical"),
        physical("primary_physical"),
    )
    else {
        return Err("checkpoint evidence inventories are invalid".to_string());
    };
    let files = |physical: &Value| {
        physical
            .get("files")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    };
    if receipt.get("backup_objects") != Some(&json!(backup_objects.len()))
        || receipt.get("primary_objects") != Some(&json!(primary_objects.len()))
        || receipt.get("backup_physical_files") != Some(&json!(files(&backup_physical)))
        || receipt.get("primary_physical_files") != Some(&json!(files(&primary_physical)))
    {
        return Err(
            "checkpoint receipt counts differ from immutable checkpoint evidence".to_string(),
        );
    }
    Ok(Evidence {
        backup_objects,
        primary_objects,
        backup_physical,
        primary_physical,
        conflict_winner: winner.unwrap_or_default().to_string(),
    })
}

pub(super) fn run(context: &mut Context) -> Step<()> {
    super::fs::make_private_dirs(&context.recovery)?;
    let phase = context.phase.clone();
    if !READ_PHASES.contains(&phase.as_str()) {
        prove_lock(context)?;
    }
    match phase.as_str() {
        "read-fence" => return receipt::read_fence_phase(context),
        "read-owner" => return receipt::read_owner_phase(context),
        "status" => return receipt::status_phase(context),
        _ => {}
    }
    if FENCED_PHASES.contains(&phase.as_str()) {
        require_storage_write_fence(context)?;
    }
    if phase == "preflight" {
        return checkpoint::preflight(context);
    }
    for root in [&context.primary, &context.backup] {
        if !real_directory(root) {
            return Err(format!("unsafe or absent fixed local storage root: {root}"));
        }
    }
    if !cfg!(target_os = "macos") {
        return Err(
            "copy-on-write storage reconciliation requires Darwin clonefile semantics".to_string(),
        );
    }
    if phase == "checkpoint" {
        return checkpoint::checkpoint(context);
    }
    let current = receipt::load_receipt(context)?;
    if receipt::status(&current) == "complete" && phase != "finalize" {
        receipt::emit(context, &current);
        return Ok(());
    }
    let evidence = load_checkpoint_evidence(context, &current)?;
    validate_complete_inventory(
        context,
        &context.backup_snapshot,
        &evidence.backup_objects,
        "backup checkpoint",
    )?;
    validate_complete_inventory(
        context,
        &context.primary_snapshot,
        &evidence.primary_objects,
        "primary checkpoint",
    )?;
    match phase.as_str() {
        "record-lifecycle-decisions" => receipt::record_lifecycle_decisions(context, current),
        "arm-activation" => union::arm_activation(context, current, &evidence),
        "arm-rollback" => union::arm_rollback(context, current, &evidence),
        "apply" => apply::apply(context, current, &evidence),
        "activate" => apply::activate(context, current),
        "finalize" => receipt::finalize(context, current),
        other => Err(format!("unknown reconciliation phase: {other}")),
    }
}
