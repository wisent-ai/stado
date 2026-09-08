if phase == "checkpoint":
    try:
        with open(fence_path, "r", encoding="utf-8") as handle:
            fence = json.load(handle)
    except Exception as error:
        fail("durable lifecycle fence is absent or unreadable: " + str(error))
    if (fence.get("schema") != "stado.storage-root-fence.v5"
            or fence.get("transaction") != tx or fence.get("status") != "fenced"
            or not fence.get("queue", {}).get("drained")
            or not (fence.get("staged_runtime") or {}).get("staged_sha256")
            or (fence.get("write_fence") or {}).get("status") != "acquired"
            or not fence.get("preflight_evidence")
            or not fence.get("rechecked_at")):
        fail("durable lifecycle fence is incomplete")
    if any(item.get("status") != "stopped" for item in fence.get("writers", [])):
        fail("durable lifecycle fence does not stop every recorded writer")
    fence_conflict_winner = conflict_winner_from_fence(fence)
    conflict_winner = fence_conflict_winner
    receipt = load_receipt() if os.path.exists(receipt_path) else None
    if receipt is not None and receipt.get("status") in (
        "checkpoint_ready", "applying", "data_committed_pending_activation",
        "activation_effects_armed", "rollback_effects_armed",
        "activated_pending_lifecycle", "complete"
    ):
        if receipt.get("conflict_winner") != fence_conflict_winner:
            fail("checkpoint conflict winner differs from the durable lifecycle fence")
        emit(receipt)
        raise SystemExit(0)
    if receipt is not None and receipt.get("status") != "checkpointing":
        fail("checkpoint receipt is not resumable: " + str(receipt.get("status")))
    if receipt is None:
        backup_paths, backup_objects, backup_physical = complete_physical_inventory(backup)
        primary_paths, primary_objects, primary_physical = complete_physical_inventory(primary)
        checkpoint_evidence = {
            "schema": "stado.storage-root-checkpoint-evidence.v1",
            "transaction": tx,
            "source": backup,
            "destination": primary,
            "conflict_winner": conflict_winner,
            "backup_objects": backup_objects,
            "primary_objects": primary_objects,
            "backup_physical": backup_physical,
            "primary_physical": primary_physical,
            "physical_snapshot_exclusions": [],
            "snapshot_scope": "full_physical_roots",
            "handoff_scope": "ecosystem/ qualified objects and matching .metadata/ecosystem sidecars",
        }
        checkpoint_evidence_reference = persist_immutable_json(
            checkpoint_evidence_path, checkpoint_evidence, "checkpoint evidence")
        receipt = {
            "schema": schema,
            "transaction": tx,
            "status": "checkpointing",
            "source": backup,
            "destination": primary,
            "backup_checkpoint": backup_snapshot,
            "primary_checkpoint": primary_snapshot,
            "effective_lifecycle_checkpoint": effective_lifecycle_snapshot,
            "checkpoint_started_at": time.time(),
            "writer_fence": fence,
            "checkpoint_evidence": checkpoint_evidence_reference,
            "conflict_winner": conflict_winner,
            "backup_objects": len(backup_objects),
            "primary_objects": len(primary_objects),
            "backup_physical_files": len(backup_physical.get("files", [])),
            "primary_physical_files": len(primary_physical.get("files", [])),
            "physical_snapshot_exclusions": [],
            "snapshot_scope": "full_physical_roots",
            "handoff_scope": "ecosystem/ qualified objects and matching .metadata/ecosystem sidecars",
        }
        atomic_json(receipt_path, receipt)
    else:
        backup_objects, primary_objects, backup_physical, primary_physical, conflict_winner = (
            load_checkpoint_evidence(receipt))
        if conflict_winner != fence_conflict_winner:
            fail("checkpoint conflict winner differs from the durable lifecycle fence")
        backup_paths = object_paths(backup)
        primary_paths = object_paths(primary)
        if [item.get("path") for item in backup_objects] != backup_paths:
            fail("backup qualified namespace no longer matches the interrupted checkpoint")
        if [item.get("path") for item in primary_objects] != primary_paths:
            fail("primary qualified namespace no longer matches the interrupted checkpoint")
        validate_complete_inventory(backup, backup_objects, "backup qualified namespace since checkpoint start")
        validate_complete_inventory(primary, primary_objects, "primary qualified namespace since checkpoint start")
        validate_physical_checkpoint(backup, backup_physical, "backup since checkpoint start")
        validate_physical_checkpoint(primary, primary_physical, "primary since checkpoint start")
    checkpoint_tree(backup, backup_snapshot, backup_physical)
    checkpoint_tree(primary, primary_snapshot, primary_physical)
    validate_physical_checkpoint(backup, backup_physical, "backup after checkpoint")
    validate_physical_checkpoint(primary, primary_physical, "primary after checkpoint")
    validate_complete_inventory(backup, backup_objects, "backup qualified namespace after checkpoint")
    validate_complete_inventory(primary, primary_objects, "primary qualified namespace after checkpoint")
    checkpoint_effective_lifecycle(primary_objects, backup_objects, conflict_winner)
    receipt["status"] = "checkpoint_ready"
    receipt["checkpointed_at"] = time.time()
    atomic_json(receipt_path, receipt)
    emit(receipt)
    raise SystemExit(0)

receipt = load_receipt()
if receipt.get("status") == "complete" and phase != "finalize":
    emit(receipt)
    raise SystemExit(0)
backup_objects, primary_objects, backup_physical, primary_physical, conflict_winner = (
    load_checkpoint_evidence(receipt))
validate_complete_inventory(backup_snapshot, backup_objects, "backup checkpoint")
validate_complete_inventory(primary_snapshot, primary_objects, "primary checkpoint")
if phase == "record-lifecycle-decisions":
    if receipt.get("status") not in (
            "checkpoint_ready", "applying", "data_committed_pending_activation",
            "activation_effects_armed", "rollback_effects_armed"):
        fail("lifecycle decisions require an immutable checkpoint before runtime activation")
    decisions, decision_reference = immutable_json_file(
        lifecycle_decisions_path, "typed lifecycle decisions")
    if not isinstance(decisions, list):
        fail("typed lifecycle decisions are not a list")
    existing_decisions = receipt.get("lifecycle_decisions_evidence")
    existing_validation = receipt.get("lifecycle_validation")
    if (existing_decisions is None) != (existing_validation is None):
        fail("typed lifecycle decision reference and validation proof are incomplete")
    if existing_decisions is not None and existing_decisions != decision_reference:
        fail("typed lifecycle decisions changed after their durable result")
    if existing_validation is not None and (
            existing_validation.get("engine") != "stado.typed-lifecycle-snapshot.v1"
            or existing_validation.get("sha256") != decision_reference["sha256"]):
        fail("typed lifecycle validation proof changed after its durable result")
    receipt["lifecycle_decisions_evidence"] = decision_reference
    receipt["lifecycle_decision_counts"] = {
        "queued_cancellation": sum(
            1 for item in decisions if item.get("kind") == "queued_cancellation"),
        "retained_outcome_cleanup": sum(
            1 for item in decisions if item.get("kind") == "retained_outcome_cleanup"),
    }
    if existing_validation is None:
        receipt["lifecycle_validation"] = {
            "engine": "stado.typed-lifecycle-snapshot.v1",
            "sha256": decision_reference["sha256"],
            "validated_at": time.time(),
        }
    atomic_json(receipt_path, receipt)
    emit(receipt)
    raise SystemExit(0)




def require_pinned_conflict_winner(fence):
    if conflict_winner_from_fence(fence) != conflict_winner:
        fail("pinned conflict winner differs from the durable lifecycle fence")


def prove_live_additive_union(label):
    backup_by_path = {item["path"]: item for item in backup_objects}
    primary_by_path = {item["path"]: item for item in primary_objects}
    primary_wins = conflict_winner == "primary"
    expected_paths = set(primary_by_path) | set(backup_by_path)
    validate_complete_inventory(backup, backup_objects, "live B " + label)
    if set(object_paths(primary)) != expected_paths:
        fail("primary namespace does not equal the additive checkpoint union " + label)
    for relative in sorted(expected_paths):
        expected = ((primary_by_path.get(relative) if primary_wins else None)
                    or backup_by_path.get(relative) or primary_by_path[relative])
        if regular_identity(os.path.join(primary, relative)) != expected["body"]:
            fail("primary body differs from additive checkpoint " + label + ": " + relative)
        if regular_identity(metadata_path(primary, relative)) != expected["metadata"]:
            fail("primary metadata differs from additive checkpoint " + label + ": " + relative)
    return backup_by_path, primary_by_path, expected_paths



def restore_primary_checkpoint():
    backup_by_path = {item["path"]: item for item in backup_objects}
    primary_by_path = {item["path"]: item for item in primary_objects}
    primary_files = {
        item["path"]: item for item in primary_physical.get("files", [])
    }
    for relative in sorted(set(primary_by_path) | set(backup_by_path)):
        before = primary_by_path.get(relative)
        incoming = backup_by_path.get(relative)
        applied = (before if conflict_winner == "primary" and before is not None
                   else incoming or before)
        destination = os.path.join(primary, relative)
        current = regular_identity(destination)
        before_body = before["body"] if before is not None else None
        applied_body = applied["body"] if applied is not None else None
        if current not in (before_body, applied_body):
            fail("primary body changed outside rollback: " + relative)
        if current != before_body:
            if before is None:
                os.unlink(destination)
                fsync_dir(os.path.dirname(destination))
            else:
                clone_file(os.path.join(primary_snapshot, relative), destination)
                original = primary_files.get(relative)
                if original is None:
                    fail("primary physical checkpoint omitted body: " + relative)
                os.chmod(destination, original["mode"])
                fsync_dir(os.path.dirname(destination))
        destination_metadata = metadata_path(primary, relative)
        current_metadata = regular_identity(destination_metadata)
        before_metadata = before["metadata"] if before is not None else None
        applied_metadata = applied["metadata"] if applied is not None else None
        if current_metadata not in (before_metadata, applied_metadata):
            fail("primary metadata changed outside rollback: " + relative)
        if current_metadata != before_metadata:
            if before_metadata is None:
                os.unlink(destination_metadata)
                fsync_dir(os.path.dirname(destination_metadata))
            else:
                clone_file(
                    metadata_path(primary_snapshot, relative),
                    destination_metadata,
                )
                metadata_relative = os.path.relpath(destination_metadata, primary)
                original = primary_files.get(metadata_relative)
                if original is None:
                    fail("primary physical checkpoint omitted metadata: " + relative)
                os.chmod(destination_metadata, original["mode"])
                fsync_dir(os.path.dirname(destination_metadata))
    original_directories = set(primary_physical.get("directories", []))
    current_directories = set(physical_inventory(primary).get("directories", []))
    extra_directories = current_directories - original_directories
    for relative in sorted(
            extra_directories,
            key=lambda path: (path.count(os.sep), len(path)),
            reverse=True):
        path = os.path.join(primary, relative)
        try:
            os.rmdir(path)
        except OSError as error:
            fail("transaction-created directory cannot be rolled back: "
                 + path + ": " + str(error))
        fsync_dir(os.path.dirname(path))
    validate_complete_inventory(primary, primary_objects, "live A after rollback")
    if physical_inventory(primary) != primary_physical:
        fail("live physical A differs from its exact rollback checkpoint")


if phase == "arm-activation":
    if receipt.get("status") == "activation_effects_armed":
        emit(receipt)
        raise SystemExit(0)
    if receipt.get("status") != "data_committed_pending_activation":
        fail("activation effects require a committed frozen union")
    try:
        with open(fence_path, "r", encoding="utf-8") as handle:
            fence = json.load(handle)
    except Exception as error:
        fail("activation fence cannot be rechecked: " + str(error))
    require_pinned_conflict_winner(fence)
    if (fence.get("status") != "fenced"
            or not fence.get("queue", {}).get("drained")
            or any(item.get("status") != "stopped" for item in fence.get("writers", []))):
        fail("activation effects require every writer to remain stopped")
    prove_live_additive_union("at activation-effect boundary")
    receipt["status"] = "activation_effects_armed"
    receipt["activation_effect_boundary_at"] = time.time()
    atomic_json(receipt_path, receipt)
    emit(receipt)
    raise SystemExit(0)


if phase == "arm-rollback":
    if receipt.get("status") == "rollback_effects_armed":
        emit(receipt)
        raise SystemExit(0)
    if receipt.get("status") not in ("checkpoint_ready", "applying"):
        fail("rollback is safe only before the data-commit boundary")
    try:
        with open(fence_path, "r", encoding="utf-8") as handle:
            fence = json.load(handle)
    except Exception as error:
        fail("rollback fence cannot be rechecked: " + str(error))
    require_pinned_conflict_winner(fence)
    if (fence.get("status") != "fenced"
            or not fence.get("queue", {}).get("drained")
            or any(item.get("status") != "stopped" for item in fence.get("writers", []))):
        fail("rollback effects require every writer to remain stopped")
    restore_primary_checkpoint()
    validate_complete_inventory(backup, backup_objects, "live B before rollback")
    validate_physical_checkpoint(backup, backup_physical, "live physical B before rollback")
    receipt["primary_checkpoint_restored_at"] = time.time()
    receipt["status"] = "rollback_effects_armed"
    receipt["rollback_effect_boundary_at"] = time.time()
    atomic_json(receipt_path, receipt)
    emit(receipt)
    raise SystemExit(0)


