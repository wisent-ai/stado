if phase == "apply":
    if receipt.get("status") not in (
            "checkpoint_ready", "applying", "data_committed_pending_activation"):
        fail("checkpoint receipt is not applicable: " + str(receipt.get("status")))
    if receipt.get("status") == "data_committed_pending_activation":
        prove_live_additive_union("while resuming committed data")
        emit(receipt)
        raise SystemExit(0)
    try:
        with open(fence_path, "r", encoding="utf-8") as handle:
            fence = json.load(handle)
    except Exception as error:
        fail("durable lifecycle fence cannot be rechecked: " + str(error))
    require_pinned_conflict_winner(fence)
    if (fence.get("status") != "fenced" or not fence.get("queue", {}).get("drained")
            or any(item.get("status") != "stopped" for item in fence.get("writers", []))
            or fence.get("rechecked_at", 0) < receipt.get("checkpointed_at", 0)):
        fail("lifecycle fence was not rechecked after checkpoint")
    validation = receipt.get("lifecycle_validation")
    decision_reference = receipt.get("lifecycle_decisions_evidence")
    if (not isinstance(validation, dict)
            or validation.get("engine") != "stado.typed-lifecycle-snapshot.v1"
            or not isinstance(decision_reference, dict)
            or validation.get("sha256") != decision_reference.get("sha256")):
        fail("typed Rust lifecycle validation is absent")
    decisions = load_immutable_json(
        decision_reference, lifecycle_decisions_path, "typed lifecycle decisions")
    if not isinstance(decisions, list):
        fail("typed lifecycle decisions are not a list")
    blockers = [item for item in decisions
                if item.get("kind") == "block_unclassified_live"]
    if blockers:
        fail("newly authoritative lifecycle state blocks activation: " +
             ", ".join(item.get("path", "?") for item in blockers))
    backup_by_path = {item["path"]: item for item in backup_objects}
    primary_by_path = {item["path"]: item for item in primary_objects}
    primary_wins = conflict_winner == "primary"
    validate_complete_inventory(backup, backup_objects, "live B after checkpoint")
    current_paths = set(object_paths(primary))
    expected_paths = set(primary_by_path) | set(backup_by_path)
    if not set(primary_by_path).issubset(current_paths) or not current_paths.issubset(expected_paths):
        fail("primary namespace drifted outside the resumable additive transition")
    for relative in sorted(expected_paths):
        alias = relative + ".json"
        if alias in expected_paths:
            fail("object metadata alias collision: " + relative + " and " + alias)
        current_body = regular_identity(os.path.join(primary, relative))
        current_meta = regular_identity(metadata_path(primary, relative))
        before = primary_by_path.get(relative)
        incoming = backup_by_path.get(relative)
        if primary_wins and before is not None:
            allowed_bodies = [before["body"]]
            allowed_metadata = [before["metadata"]]
        else:
            allowed_bodies = [item["body"] for item in (before, incoming) if item is not None]
            allowed_metadata = [item["metadata"] for item in (before, incoming) if item is not None]
            if before is None:
                allowed_bodies.append(None)
                allowed_metadata.append(None)
        if current_body not in allowed_bodies or current_meta not in allowed_metadata:
            fail("primary object drifted after its checkpoint: " + relative)
    receipt["status"] = "applying"
    atomic_json(receipt_path, receipt)
    for item in backup_objects:
        relative = item["path"]
        destination = os.path.join(primary, relative)
        current = regular_identity(destination)
        before = primary_by_path.get(relative)
        desired = before if primary_wins and before is not None else item
        if current != desired["body"]:
            before_body = before["body"] if before is not None else None
            if current != before_body:
                fail("primary body changed outside the transaction: " + relative)
            clone_file(os.path.join(backup_snapshot, relative), destination)
        destination_meta = metadata_path(primary, relative)
        current_meta = regular_identity(destination_meta)
        if current_meta != desired["metadata"]:
            before_meta = before["metadata"] if before is not None else None
            if current_meta != before_meta:
                fail("primary metadata changed outside the transaction: " + relative)
            if desired["metadata"] is None:
                if os.path.exists(destination_meta):
                    os.unlink(destination_meta)
                    fsync_dir(os.path.dirname(destination_meta))
            else:
                clone_file(metadata_path(backup_snapshot, relative), destination_meta)
        if regular_identity(destination) != desired["body"]:
            fail("destination body did not verify: " + relative)
        if regular_identity(destination_meta) != desired["metadata"]:
            fail("destination metadata did not verify: " + relative)
    prove_live_additive_union("after apply")
    receipt["status"] = "data_committed_pending_activation"
    receipt["data_committed_at"] = time.time()
    receipt["verified_objects"] = len(expected_paths)
    receipt["conflict_winner"] = conflict_winner
    receipt["primary_only_preserved"] = True
    receipt["backup_objects_not_written"] = True
    atomic_json(receipt_path, receipt)
    emit(receipt)
    raise SystemExit(0)

if phase == "activate":
    if receipt.get("status") == "activated_pending_lifecycle":
        emit(receipt)
        raise SystemExit(0)
    if receipt.get("status") != "activation_effects_armed":
        fail("reconciliation has no durable activation-effect boundary: " + str(receipt.get("status")))
    try:
        with open(fence_path, "r", encoding="utf-8") as handle:
            fence = json.load(handle)
    except Exception as error:
        fail("activated lifecycle fence cannot be read: " + str(error))
    active_path = os.path.expanduser("~/.stado/bin/stado")
    expected_digest = fence.get("activation_sha256")
    if (fence.get("schema") != "stado.storage-root-fence.v5"
            or fence.get("status") != "activated"
            or not fence.get("queue", {}).get("resumed")
            or not fence.get("restored_at")
            or (fence.get("write_fence") or {}).get("status") != "released"
            or not isinstance(expected_digest, str)
            or len(expected_digest) != 64
            or os.path.islink(active_path)
            or not os.path.isfile(active_path)
            or digest(active_path) != expected_digest):
        fail("runtime activation and lifecycle restoration are not durably proved")
    if any(item.get("status") != "restored" for item in fence.get("writers", [])):
        fail("activated fence does not restore every captured native service state")
    receipt["status"] = "activated_pending_lifecycle"
    receipt["activated_at"] = fence.get("activated_at")
    receipt["activated_sha256"] = expected_digest
    atomic_json(receipt_path, receipt)
    emit(receipt)
    raise SystemExit(0)

if phase != "finalize":
    fail("unknown reconciliation phase: " + phase)
if receipt.get("status") not in ("activated_pending_lifecycle", "complete"):
    fail("reconciliation is not awaiting typed lifecycle finalization: " + str(receipt.get("status")))
observations, observation_reference = immutable_json_file(
    final_lifecycle_observations_path, "typed final lifecycle observations")
if not isinstance(observations, list):
    fail("typed final lifecycle observations are not a list")
existing_observations = receipt.get("final_lifecycle_observations_evidence")
existing_validation = receipt.get("final_lifecycle_validation")
if (existing_observations is None) != (existing_validation is None):
    fail("typed final observation reference and validation proof are incomplete")
if existing_observations is not None and existing_observations != observation_reference:
    fail("typed final lifecycle observations changed after their durable result")
if existing_validation is not None and (
        existing_validation.get("engine") != "stado.typed-lifecycle-final.v1"
        or existing_validation.get("sha256") != observation_reference["sha256"]):
    fail("typed final lifecycle validation proof changed after its durable result")
receipt["final_lifecycle_observations_evidence"] = observation_reference
if existing_validation is None:
    receipt["final_lifecycle_validation"] = {
        "engine": "stado.typed-lifecycle-final.v1",
        "sha256": observation_reference["sha256"],
        "validated_at": time.time(),
    }
receipt["status"] = "complete"
receipt["completed_at"] = receipt.get("completed_at") or time.time()
receipt["canonical_recovery_verified"] = True
atomic_json(receipt_path, receipt)
emit(receipt)
