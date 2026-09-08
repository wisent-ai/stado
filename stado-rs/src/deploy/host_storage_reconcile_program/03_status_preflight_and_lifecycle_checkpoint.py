def emit(receipt):
    decisions = receipt.get("lifecycle_decision_counts", {})
    summary = {
        "schema": receipt.get("schema"),
        "transaction": receipt.get("transaction"),
        "status": receipt.get("status"),
        "receipt_path": receipt_path,
        "backup_checkpoint": receipt.get("backup_checkpoint"),
        "primary_checkpoint": receipt.get("primary_checkpoint"),
        "backup_objects": receipt.get("backup_objects", 0),
        "primary_objects": receipt.get("primary_objects", 0),
        "verified_objects": receipt.get("verified_objects", 0),
        "backup_physical_files": receipt.get("backup_physical_files", 0),
        "primary_physical_files": receipt.get("primary_physical_files", 0),
        "physical_snapshot_exclusions": receipt.get("physical_snapshot_exclusions", []),
        "lifecycle_decisions": {
            "queued_cancellation": decisions.get("queued_cancellation", 0),
            "retained_outcome_cleanup": decisions.get("retained_outcome_cleanup", 0),
        },
    }
    print("STADO_STORAGE_RECONCILE\t" +
          json.dumps(summary, sort_keys=True, separators=(",", ":")))

if phase == "status":
    if os.path.isfile(receipt_path):
        emit(load_receipt())
    else:
        print("STADO_STORAGE_RECONCILE\t" + json.dumps({
            "schema": schema,
            "transaction": tx,
            "status": "absent",
            "receipt_path": receipt_path,
        }, sort_keys=True, separators=(",", ":")))
    raise SystemExit(0)


def inventory(root, paths):
    result = []
    for relative in paths:
        body_path = os.path.join(root, relative)
        body = regular_identity(body_path)
        if body is None:
            raise FileNotFoundError(body_path)
        result.append({
            "path": relative,
            "body": body,
            "metadata": regular_identity(metadata_path(root, relative)),
        })
    return result


def validate_inventory(root, objects, label):
    for item in objects:
        relative = item["path"]
        if regular_identity(os.path.join(root, relative)) != item["body"]:
            fail(label + " body changed: " + relative)
        if regular_identity(metadata_path(root, relative)) != item["metadata"]:
            fail(label + " metadata changed: " + relative)

def validate_complete_inventory(root, objects, label):
    expected = [item["path"] for item in objects]
    actual = object_paths(root)
    if actual != expected:
        fail(label + " namespace changed")
    validate_inventory(root, objects, label)
    expected_metadata = sorted(
        os.path.relpath(metadata_path(root, item["path"]), os.path.join(root, ".metadata"))
        for item in objects if item["metadata"] is not None
    )
    actual_metadata = metadata_paths(root)
    if actual_metadata != expected_metadata:
        fail(label + " metadata namespace changed")


def complete_physical_inventory(root):
    paths = object_paths(root)
    return paths, inventory(root, paths), physical_inventory(root)




def checkpoint_tree(source, destination, snapshot):
    if os.path.isdir(destination):
        seal_tree(destination)
        validate_sealed_tree(destination)
        validate_physical_checkpoint(destination, snapshot, "immutable checkpoint")
        return
    building = destination + ".building"
    os.makedirs(building, mode=0o700, exist_ok=True)
    for relative in snapshot["directories"]:
        directory = os.path.join(building, relative)
        if os.path.lexists(directory) and not os.path.isdir(directory):
            fail("checkpoint directory collides with non-directory: " + relative)
        os.makedirs(directory, mode=0o700, exist_ok=True)
    for item in snapshot["files"]:
        relative = item["path"]
        target = os.path.join(building, relative)
        if regular_identity(target) != item["body"]:
            if os.path.lexists(target):
                os.unlink(target)
            clone_file(os.path.join(source, relative), target)
        if regular_identity(target) != item["body"]:
            fail("physical checkpoint file did not verify: " + relative)
    validate_physical_checkpoint(building, snapshot, "building checkpoint")
    for directory, dirs, files in os.walk(building, topdown=False):
        for name in files:
            os.chmod(os.path.join(directory, name), 0o400)
        for name in dirs:
            os.chmod(os.path.join(directory, name), 0o500)
        os.chmod(directory, 0o500)
    os.replace(building, destination)
    fsync_dir(os.path.dirname(destination))
    seal_tree(destination)
    validate_sealed_tree(destination)
    validate_physical_checkpoint(destination, snapshot, "sealed checkpoint")


def require_storage_write_fence():
    lock = os.path.join(home, ".stado", "recovery", "storage-root-writes.lock")
    intent_path = os.path.join(home, ".stado", "recovery", "storage-root-write-fence.json")
    try:
        with open(fence_path, encoding="utf-8") as handle:
            lifecycle = json.load(handle)
        with open(intent_path, encoding="utf-8") as handle:
            intent = json.load(handle)
        effect = lifecycle.get("write_fence") or {}
        if (lifecycle.get("schema") != "stado.storage-root-fence.v5"
                or lifecycle.get("transaction") != tx
                or effect.get("status") != "acquired"
                or effect.get("intent") != intent
                or intent.get("schema") != "stado.storage-root-write-fence.v1"
                or intent.get("transaction") != tx
                or not lifecycle.get("queue", {}).get("drained")):
            fail("physical inventory requires the recorded drained storage write fence")
        descriptor = os.open(lock, os.O_RDONLY | os.O_NOFOLLOW)
        try:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_SH | fcntl.LOCK_NB)
            except BlockingIOError:
                pass
            else:
                fail("storage write-fence intent has no active exclusive hold")
        finally:
            os.close(descriptor)
    except Exception as error:
        fail("storage write fence cannot be observed: " + str(error))


if phase in ("preflight", "checkpoint", "apply", "arm-activation", "arm-rollback"):
    require_storage_write_fence()


if phase == "preflight":
    backup_paths, backup_objects, backup_physical = complete_physical_inventory(backup)
    primary_paths, primary_objects, primary_physical = complete_physical_inventory(primary)
    print("STADO_STORAGE_RECONCILE\t" + json.dumps({
        "schema": schema,
        "transaction": tx,
        "status": "observed",
        "observed_at": time.time(),
        "backup_qualified": backup_objects,
        "primary_qualified": primary_objects,
        "backup_physical": backup_physical,
        "primary_physical": primary_physical,
        "physical_snapshot_exclusions": [],
    }, sort_keys=True, separators=(",", ":")))
    raise SystemExit(0)

def conflict_winner_from_fence(fence):
    roots = fence.get("roots") or {}
    prior_primary = roots.get("prior_primary")
    if (fence.get("schema") != "stado.storage-root-fence.v5"
            or fence.get("transaction") != tx
            or roots.get("primary") != primary
            or roots.get("backup") != backup
            or prior_primary not in (primary, backup)):
        fail("storage conflict winner is invalid")
    return "primary" if prior_primary == primary else "backup"


def validate_effective_lifecycle(root, expected):
    actual = []
    for directory, dirs, files in os.walk(root):
        dirs[:] = sorted(name for name in dirs if name not in (".locks", ".metadata"))
        for name in sorted(files):
            path = os.path.join(directory, name)
            if os.path.islink(path) or not os.path.isfile(path):
                fail("effective lifecycle snapshot contains a non-regular file: " + path)
            actual.append(os.path.relpath(path, root))
    if sorted(actual) != sorted(expected):
        fail("effective lifecycle snapshot namespace differs from its qualified A/B union")


def checkpoint_effective_lifecycle(primary_objects, backup_objects, conflict_winner):
    selected = {}
    sources = (
        ((backup_snapshot, backup_objects), (primary_snapshot, primary_objects))
        if conflict_winner == "primary"
        else ((primary_snapshot, primary_objects), (backup_snapshot, backup_objects))
    )
    for source_root, objects in sources:
        for item in objects:
            relative = item["path"]
            if relative.startswith(lifecycle_root):
                selected[relative[len(lifecycle_root):]] = (source_root, relative, item)
    expected = sorted(selected)
    if os.path.isdir(effective_lifecycle_snapshot):
        validate_sealed_tree(effective_lifecycle_snapshot)
        validate_effective_lifecycle(effective_lifecycle_snapshot, expected)
        for relative, (_, source_relative, item) in selected.items():
            if regular_identity(os.path.join(effective_lifecycle_snapshot, relative)) != item["body"]:
                fail("effective lifecycle body differs from the immutable overlay: " + source_relative)
            if regular_identity(metadata_path(effective_lifecycle_snapshot, relative)) != item["metadata"]:
                fail("effective lifecycle metadata differs from the immutable overlay: " + source_relative)
        return
    building = effective_lifecycle_snapshot + ".building"
    os.makedirs(os.path.join(building, ".locks"), mode=0o700, exist_ok=True)
    os.makedirs(os.path.join(building, ".metadata"), mode=0o700, exist_ok=True)
    for relative, (source_root, source_relative, item) in selected.items():
        destination = os.path.join(building, relative)
        if regular_identity(destination) != item["body"]:
            if os.path.lexists(destination):
                os.unlink(destination)
            clone_file(os.path.join(source_root, source_relative), destination)
        source_metadata = metadata_path(source_root, source_relative)
        destination_metadata = metadata_path(building, relative)
        if regular_identity(destination_metadata) != item["metadata"]:
            if os.path.lexists(destination_metadata):
                os.unlink(destination_metadata)
            if item["metadata"] is not None:
                clone_file(source_metadata, destination_metadata)
        if regular_identity(destination) != item["body"]:
            fail("effective lifecycle body did not verify: " + source_relative)
        if regular_identity(destination_metadata) != item["metadata"]:
            fail("effective lifecycle metadata did not verify: " + source_relative)
    validate_effective_lifecycle(building, expected)
    for directory, dirs, files in os.walk(building, topdown=False):
        for name in files:
            os.chmod(os.path.join(directory, name), 0o400)
        for name in dirs:
            os.chmod(os.path.join(directory, name), 0o500)
        os.chmod(directory, 0o500)
    os.replace(building, effective_lifecycle_snapshot)
    fsync_dir(os.path.dirname(effective_lifecycle_snapshot))
    seal_tree(effective_lifecycle_snapshot)
    validate_sealed_tree(effective_lifecycle_snapshot)
    validate_effective_lifecycle(effective_lifecycle_snapshot, expected)


for fixed_root in (primary, backup):
    if not os.path.isdir(fixed_root) or os.path.islink(fixed_root):
        fail("unsafe or absent fixed local storage root: " + fixed_root)
if sys.platform != "darwin":
    fail("copy-on-write storage reconciliation requires Darwin clonefile semantics")

def load_checkpoint_evidence(receipt):
    evidence = load_immutable_json(
        receipt.get("checkpoint_evidence"),
        checkpoint_evidence_path,
        "checkpoint evidence",
    )
    if (not isinstance(evidence, dict)
            or evidence.get("schema") != "stado.storage-root-checkpoint-evidence.v1"
            or evidence.get("transaction") != tx
            or evidence.get("source") != backup
            or evidence.get("destination") != primary
            or evidence.get("conflict_winner") not in ("primary", "backup")
            or receipt.get("conflict_winner") != evidence.get("conflict_winner")):
        fail("checkpoint evidence belongs to another reconciliation")
    primary_objects = evidence.get("primary_objects")
    backup_objects = evidence.get("backup_objects")
    backup_physical = evidence.get("backup_physical")
    primary_physical = evidence.get("primary_physical")
    if (not isinstance(backup_objects, list)
            or not isinstance(primary_objects, list)
            or not isinstance(backup_physical, dict)
            or not isinstance(primary_physical, dict)):
        fail("checkpoint evidence inventories are invalid")
    if (receipt.get("backup_objects") != len(backup_objects)
            or receipt.get("primary_objects") != len(primary_objects)
            or receipt.get("backup_physical_files") != len(backup_physical.get("files", []))
            or receipt.get("primary_physical_files") != len(primary_physical.get("files", []))):
        fail("checkpoint receipt counts differ from immutable checkpoint evidence")
    return (backup_objects, primary_objects, backup_physical, primary_physical,
            evidence["conflict_winner"])


