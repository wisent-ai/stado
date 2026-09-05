//! Interruption-safe reconciliation of the two fixed co-located local object roots.
//!
//! This is deliberately not `host object-relocate`: relocation moves one in-store
//! address and refuses overwrites. This transaction checkpoints both physical
//! roots with copy-on-write clones, then additively makes `local-storage`
//! contain `local-backup`'s exact objects and effective metadata. Backup bytes
//! and primary-only objects are never removed. The immutable full-primary
//! checkpoint retains conflicting primary bytes before the backup-winning
//! value is installed.

use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::{sleep, Instant};

use super::{host_channel, service};
use super::{shlex_quote, DeployError, Runner};

pub const CHECKPOINT: &str = "checkpoint";
pub const APPLY: &str = "apply";
pub const FINALIZE: &str = "finalize";
const TIMEOUT: Duration = Duration::from_secs(60 * 60);

const REMOTE_SCRIPT: &str = r#"set -u
STADO_RECONCILE_PHASE=@PHASE@ STADO_RECONCILE_TX=@TRANSACTION@ STADO_RECONCILE_FENCE=@FENCE@ /usr/bin/python3 - <<'STADO_RECONCILE_EOF'
import ctypes, fcntl, hashlib, json, os, stat, sys, time

phase = os.environ["STADO_RECONCILE_PHASE"]
tx = os.environ["STADO_RECONCILE_TX"]
home = os.path.expanduser("~")
primary = os.path.join(home, ".stado", "local-storage")
backup = os.path.join(home, ".stado", "local-backup")
work = os.path.join(home, ".stado", "recovery", "storage-root-reconcile", tx)
backup_snapshot = os.path.join(work, "local-backup.checkpoint")
primary_snapshot = os.path.join(work, "local-storage.checkpoint")
receipt_path = os.path.join(work, "receipt.json")
fence_path = os.path.join(work, "lifecycle-fence.json")
lock_path = os.path.join(home, ".stado", "recovery", "storage-root-reconcile.lock")
schema = "stado.storage-root-reconcile.v1"
staging = os.path.join(work, ".clone-staging")
fence_payload = os.environ.get("STADO_RECONCILE_FENCE", "")
lifecycle_root = "ecosystem/probierz/"


def fail(message):
    print("STADO_STORAGE_RECONCILE_ERROR\t" + str(message).replace("\t", " ").replace("\n", " "))
    raise SystemExit(0)


def fsync_dir(path):
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def digest(path):
    value = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def metadata_path(root, relative):
    name = relative if relative.endswith(".json") else relative + ".json"
    candidate = os.path.join(root, ".metadata", name)
    current = root
    for component in os.path.relpath(os.path.dirname(candidate), root).split(os.sep):
        current = os.path.join(current, component)
        if os.path.lexists(current) and os.path.islink(current):
            fail("symlinked metadata directory: " + current)
    return candidate

def atomic_json(path, value):
    os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
    temporary = path + ".new"
    with open(temporary, "w", encoding="utf-8") as handle:
        json.dump(value, handle, sort_keys=True, separators=(",", ":"))
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)
    fsync_dir(os.path.dirname(path))


def clone_file(source, destination):
    os.makedirs(os.path.dirname(destination), mode=0o700, exist_ok=True)
    if os.path.lexists(staging) and (os.path.islink(staging) or not os.path.isdir(staging)):
        fail("unsafe transaction clone staging root")
    os.makedirs(staging, mode=0o700, exist_ok=True)
    temporary = os.path.join(
        staging,
        hashlib.sha256(destination.encode("utf-8")).hexdigest(),
    )
    if os.path.lexists(temporary):
        if regular_identity(temporary) != regular_identity(source):
            os.unlink(temporary)
    if not os.path.exists(temporary):
        libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
        clone = libc.clonefile
        clone.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_int]
        clone.restype = ctypes.c_int
        if clone(os.fsencode(source), os.fsencode(temporary), 0) != 0:
            error = ctypes.get_errno()
            fail("clonefile refused copy-on-write clone: " + os.strerror(error))
    if digest(source) != digest(temporary):
        fail("copy-on-write clone verification failed")
    if hasattr(os, "chflags"):
        os.chflags(temporary, 0)
    os.chmod(temporary, 0o600)
    with open(temporary, "rb") as handle:
        os.fsync(handle.fileno())
    os.replace(temporary, destination)
    fsync_dir(os.path.dirname(destination))


def regular_identity(path):
    try:
        info = os.lstat(path)
    except FileNotFoundError:
        return None
    if not stat.S_ISREG(info.st_mode):
        fail("non-regular object: " + path)
    return {"bytes": info.st_size, "sha256": digest(path)}


def object_paths(root):
    ecosystem = os.path.join(root, "ecosystem")
    if not os.path.isdir(ecosystem) or os.path.islink(ecosystem):
        fail("unsafe or absent ecosystem root: " + ecosystem)
    result = []
    errors = []
    def onerror(error):
        errors.append(error)
    for directory, dirs, files in os.walk(ecosystem, followlinks=False, onerror=onerror):
        for name in dirs:
            if os.path.islink(os.path.join(directory, name)):
                fail("symlinked object directory: " + os.path.join(directory, name))
        for name in files:
            path = os.path.join(directory, name)
            info = os.lstat(path)
            if not stat.S_ISREG(info.st_mode):
                fail("non-regular backup object: " + path)
            result.append(os.path.relpath(path, root))
    if errors:
        fail("object enumeration failed: " + str(errors[0]))
    return sorted(result)



def seal_tree(root):
    immutable = getattr(stat, "UF_IMMUTABLE", 0)
    if not immutable or not hasattr(os, "chflags"):
        fail("Darwin immutable-file flags are unavailable")
    for directory, dirs, files in os.walk(root, topdown=False):
        for name in files:
            path = os.path.join(directory, name)
            info = os.lstat(path)
            if not info.st_flags & immutable:
                os.chmod(path, 0o400)
                os.chflags(path, info.st_flags | immutable)
        for name in dirs:
            path = os.path.join(directory, name)
            info = os.lstat(path)
            if not info.st_flags & immutable:
                os.chmod(path, 0o500)
                os.chflags(path, info.st_flags | immutable)
        info = os.lstat(directory)
        if not info.st_flags & immutable:
            os.chmod(directory, 0o500)
            os.chflags(directory, info.st_flags | immutable)


def validate_sealed_tree(root):
    immutable = getattr(stat, "UF_IMMUTABLE", 0)
    for directory, dirs, files in os.walk(root, topdown=False):
        for name in files:
            info = os.lstat(os.path.join(directory, name))
            if not info.st_flags & immutable or stat.S_IMODE(info.st_mode) != 0o400:
                fail("checkpoint file is not immutable: " + os.path.join(directory, name))
        for name in dirs:
            info = os.lstat(os.path.join(directory, name))
            if not info.st_flags & immutable or stat.S_IMODE(info.st_mode) != 0o500:
                fail("checkpoint directory is not immutable: " + os.path.join(directory, name))
        info = os.lstat(directory)
        if not info.st_flags & immutable or stat.S_IMODE(info.st_mode) != 0o500:
            fail("checkpoint root is not immutable: " + directory)


def load_receipt():
    try:
        with open(receipt_path, "r", encoding="utf-8") as handle:
            value = json.load(handle)
    except FileNotFoundError:
        fail("checkpoint receipt is absent")
    if value.get("schema") != schema or value.get("transaction") != tx:
        fail("checkpoint receipt belongs to another transaction")
    return value


os.makedirs(os.path.dirname(lock_path), mode=0o700, exist_ok=True)
operation_lock = open(lock_path, "a+b")
try:
    fcntl.flock(operation_lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:
    fail("another storage-root reconciliation holds the transaction lock")

if phase == "read-fence":
    try:
        with open(fence_path, "r", encoding="utf-8") as handle:
            fence = json.load(handle)
    except FileNotFoundError:
        fence = {"schema": "stado.storage-root-fence.v1", "transaction": tx,
                 "status": "absent", "writers": []}
    print("STADO_STORAGE_RECONCILE\t" +
          json.dumps(fence, sort_keys=True, separators=(",", ":")))
    raise SystemExit(0)


if phase == "record-fence":
    try:
        fence = json.loads(fence_payload)
    except Exception as error:
        fail("lifecycle fence payload is invalid: " + str(error))
    if (fence.get("schema") != "stado.storage-root-fence.v1"
            or fence.get("transaction") != tx):
        fail("lifecycle fence payload belongs to another transaction")
    atomic_json(fence_path, fence)
    print("STADO_STORAGE_RECONCILE\t" + json.dumps({
        "schema": fence["schema"],
        "transaction": tx,
        "status": fence.get("status"),
        "receipt_path": fence_path,
        "writers": len(fence.get("writers", [])),
        "queue_drained": fence.get("queue", {}).get("drained", False),
    }, sort_keys=True, separators=(",", ":")))
    raise SystemExit(0)

def emit(receipt):
    decisions = receipt.get("lifecycle_decisions", [])
    summary = {
        "schema": receipt.get("schema"),
        "transaction": receipt.get("transaction"),
        "status": receipt.get("status"),
        "receipt_path": receipt_path,
        "backup_checkpoint": receipt.get("backup_checkpoint"),
        "primary_checkpoint": receipt.get("primary_checkpoint"),
        "backup_objects": len(receipt.get("backup_objects", [])),
        "primary_objects": len(receipt.get("primary_objects", [])),
        "verified_objects": receipt.get("verified_objects", 0),
        "lifecycle_decisions": {
            "queued_cancellation": sum(1 for item in decisions
                                        if item.get("kind") == "queued_cancellation"),
            "retained_outcome_cleanup": sum(1 for item in decisions
                                             if item.get("kind") == "retained_outcome_cleanup"),
        },
    }
    print("STADO_STORAGE_RECONCILE\t" +
          json.dumps(summary, sort_keys=True, separators=(",", ":")))


def inventory(root, paths):
    return [{
        "path": relative,
        "body": regular_identity(os.path.join(root, relative)),
        "metadata": regular_identity(metadata_path(root, relative)),
    } for relative in paths]


def validate_inventory(root, objects, label):
    for item in objects:
        relative = item["path"]
        if regular_identity(os.path.join(root, relative)) != item["body"]:
            fail(label + " body changed: " + relative)
        if regular_identity(metadata_path(root, relative)) != item["metadata"]:
            fail(label + " metadata changed: " + relative)


def checkpoint_tree(source, destination, objects):
    if os.path.isdir(destination):
        seal_tree(destination)
        validate_sealed_tree(destination)
        validate_inventory(destination, objects, "immutable checkpoint")
        return
    building = destination + ".building"
    os.makedirs(building, mode=0o700, exist_ok=True)
    for item in objects:
        relative = item["path"]
        target = os.path.join(building, relative)
        target_meta = metadata_path(building, relative)
        if regular_identity(target) != item["body"]:
            if os.path.lexists(target):
                os.unlink(target)
            clone_file(os.path.join(source, relative), target)
        if item["metadata"] is not None and regular_identity(target_meta) != item["metadata"]:
            if os.path.lexists(target_meta):
                os.unlink(target_meta)
            clone_file(metadata_path(source, relative), target_meta)
        if regular_identity(target) != item["body"]:
            fail("checkpoint body did not verify: " + relative)
        if regular_identity(target_meta) != item["metadata"]:
            fail("checkpoint metadata did not verify: " + relative)
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


def lifecycle_decisions():
    backup_paths = set(object_paths(backup_snapshot))
    primary_paths = set(object_paths(primary_snapshot))
    effective_paths = backup_paths | primary_paths
    primary_only = primary_paths - backup_paths
    lifecycle_names = {
        "queue", "running", "completed", "uploaded", "failed", "cancelled",
        "status", "queue_priority", "cancellations", "job-transitions", "runs",
    }

    def effective_file(relative):
        root = backup_snapshot if relative in backup_paths else primary_snapshot
        return os.path.join(root, relative)

    terminal = {"completed", "uploaded", "failed", "cancelled"}
    retained = {}
    run_prefix = lifecycle_root + "runs/"
    for relative in sorted(effective_paths):
        if not relative.startswith(run_prefix) or not relative.endswith(".json"):
            continue
        try:
            with open(effective_file(relative), "r", encoding="utf-8") as handle:
                manifest = json.load(handle)
        except Exception as error:
            fail("invalid effective run manifest " + relative + ": " + str(error))
        if manifest.get("schema") != "stado.run-submission.v3":
            continue
        entries = manifest.get("entries")
        if not isinstance(entries, list) or not entries:
            continue
        complete = True
        job_ids = []
        for entry in entries:
            job_id = entry.get("job_id") if isinstance(entry, dict) else None
            outcome = entry.get("outcome") if isinstance(entry, dict) else None
            prefix = outcome.get("prefix") if isinstance(outcome, dict) else None
            job = outcome.get("job") if isinstance(outcome, dict) else None
            if (not isinstance(job_id, str) or prefix not in terminal
                    or not isinstance(job, dict) or job.get("job_id") != job_id
                    or job.get("state") != prefix):
                complete = False
                break
            job_ids.append(job_id)
        if complete:
            run_id = relative[len(run_prefix):-5]
            for job_id in job_ids:
                retained[job_id] = {"run_id": run_id, "manifest": relative}

    queued_cancellations = set()
    cancellation_prefix = lifecycle_root + "cancellations/"
    for relative in effective_paths:
        if relative.startswith(cancellation_prefix) and relative.endswith(".json"):
            job_id = relative[len(cancellation_prefix):-5]
            if lifecycle_root + "queue/" + job_id + ".json" in effective_paths:
                queued_cancellations.add(job_id)

    known_ids = set(retained) | queued_cancellations
    for prefix in ("queue", "running", "completed", "uploaded", "failed", "cancelled"):
        start = lifecycle_root + prefix + "/"
        known_ids.update(path[len(start):-5] for path in effective_paths
                         if path.startswith(start) and path.endswith(".json"))

    grouped = {}
    for relative in sorted(primary_only):
        tail = relative[len(lifecycle_root):] if relative.startswith(lifecycle_root) else ""
        family = tail.split("/", 1)[0]
        if family not in lifecycle_names:
            continue
        job_id = None
        if family in {"queue", "running", "completed", "uploaded", "failed",
                      "cancelled", "cancellations"} and tail.endswith(".json"):
            job_id = tail.split("/", 1)[1][:-5]
        elif family == "status":
            parts = tail.split("/")
            if len(parts) >= 3:
                job_id = parts[1]
        elif family == "queue_priority":
            job_id = next((candidate for candidate in sorted(known_ids, key=len, reverse=True)
                           if relative.endswith("-" + candidate + ".json")), None)
        elif family == "job-transitions" and tail.endswith(".json"):
            digest_name = tail.split("/", 1)[1][:-5]
            job_id = next((candidate for candidate in known_ids
                           if hashlib.sha256(candidate.encode("utf-8")).hexdigest()
                           == digest_name), None)
        if job_id in queued_cancellations and family in {"queue", "cancellations", "queue_priority"}:
            key = ("queued_cancellation", job_id)
            detail = {"kind": key[0], "job_id": job_id}
        elif job_id in retained and family in {
            "queue", "running", "completed", "uploaded", "failed", "cancelled",
            "status", "queue_priority", "job-transitions"
        }:
            proof = retained[job_id]
            key = ("retained_outcome_cleanup", proof["run_id"])
            detail = {
                "kind": key[0],
                "run_id": proof["run_id"],
                "manifest": proof["manifest"],
                "job_ids": [],
            }
        else:
            key = ("preserve_unresolved", relative)
            detail = {"kind": key[0], "path": relative, "reason": "no strict effective terminal proof"}
        decision = grouped.setdefault(
            key,
            {**detail, "primary_only_paths": [], "transition_companions": []},
        )
        if family == "job-transitions" and key[0] == "retained_outcome_cleanup":
            decision["transition_companions"].append(relative)
        else:
            decision["primary_only_paths"].append(relative)
        if job_id and key[0] == "retained_outcome_cleanup":
            decision["job_ids"].append(job_id)
    for decision in grouped.values():
        if "job_ids" in decision:
            decision["job_ids"] = sorted(set(decision["job_ids"]))
    return list(grouped.values())


for fixed_root in (primary, backup):
    if not os.path.isdir(fixed_root) or os.path.islink(fixed_root):
        fail("unsafe or absent fixed local storage root: " + fixed_root)
if sys.platform != "darwin":
    fail("copy-on-write storage reconciliation requires Darwin clonefile semantics")

if phase == "checkpoint":
    try:
        with open(fence_path, "r", encoding="utf-8") as handle:
            fence = json.load(handle)
    except Exception as error:
        fail("durable lifecycle fence is absent or unreadable: " + str(error))
    if (fence.get("schema") != "stado.storage-root-fence.v1"
            or fence.get("transaction") != tx or fence.get("status") != "fenced"
            or not fence.get("queue", {}).get("drained")
            or not fence.get("transport_retained")
            or not fence.get("rechecked_at")):
        fail("durable lifecycle fence is incomplete")
    if any(item.get("status") != "stopped" for item in fence.get("writers", [])):
        fail("durable lifecycle fence does not stop every recorded writer")
    receipt = load_receipt() if os.path.exists(receipt_path) else None
    if receipt is not None and receipt.get("status") in (
        "checkpoint_ready", "applying", "applied_pending_recovery", "complete"
    ):
        emit(receipt)
        raise SystemExit(0)
    if receipt is not None and receipt.get("status") != "checkpointing":
        fail("checkpoint receipt is not resumable: " + str(receipt.get("status")))
    backup_paths = object_paths(backup)
    primary_paths = object_paths(primary)
    if receipt is None:
        backup_objects = inventory(backup, backup_paths)
        primary_objects = inventory(primary, primary_paths)
        receipt = {
            "schema": schema,
            "transaction": tx,
            "status": "checkpointing",
            "source": backup,
            "destination": primary,
            "backup_checkpoint": backup_snapshot,
            "primary_checkpoint": primary_snapshot,
            "checkpoint_started_at": time.time(),
            "writer_fence": fence,
            "backup_objects": backup_objects,
            "primary_objects": primary_objects,
            "lifecycle_decisions": [],
        }
        atomic_json(receipt_path, receipt)
    else:
        backup_objects = receipt.get("backup_objects")
        primary_objects = receipt.get("primary_objects")
        if not isinstance(backup_objects, list) or not isinstance(primary_objects, list):
            fail("checkpoint receipt inventories are invalid")
        if [item.get("path") for item in backup_objects] != backup_paths:
            fail("backup namespace no longer matches the interrupted checkpoint")
        if [item.get("path") for item in primary_objects] != primary_paths:
            fail("primary namespace no longer matches the interrupted checkpoint")
        validate_inventory(backup, backup_objects, "backup since checkpoint start")
        validate_inventory(primary, primary_objects, "primary since checkpoint start")
    checkpoint_tree(backup, backup_snapshot, backup_objects)
    checkpoint_tree(primary, primary_snapshot, primary_objects)
    if object_paths(backup) != backup_paths or object_paths(primary) != primary_paths:
        fail("storage namespace changed during fenced checkpoint")
    validate_inventory(backup, backup_objects, "backup after checkpoint")
    validate_inventory(primary, primary_objects, "primary after checkpoint")
    receipt["lifecycle_decisions"] = lifecycle_decisions()
    receipt["status"] = "checkpoint_ready"
    receipt["checkpointed_at"] = time.time()
    atomic_json(receipt_path, receipt)
    emit(receipt)
    raise SystemExit(0)

receipt = load_receipt()
if receipt.get("status") == "complete":
    emit(receipt)
    raise SystemExit(0)
backup_objects = receipt.get("backup_objects")
primary_objects = receipt.get("primary_objects")
if not isinstance(backup_objects, list) or not isinstance(primary_objects, list):
    fail("checkpoint receipt has no complete object inventories")
validate_inventory(backup_snapshot, backup_objects, "backup checkpoint")
validate_inventory(primary_snapshot, primary_objects, "primary checkpoint")

if phase == "apply":
    if receipt.get("status") not in ("checkpoint_ready", "applying", "applied_pending_recovery"):
        fail("checkpoint receipt is not applicable: " + str(receipt.get("status")))
    if receipt.get("status") == "applied_pending_recovery":
        emit(receipt)
        raise SystemExit(0)
    try:
        with open(fence_path, "r", encoding="utf-8") as handle:
            fence = json.load(handle)
    except Exception as error:
        fail("durable lifecycle fence cannot be rechecked: " + str(error))
    if (fence.get("status") != "fenced" or not fence.get("queue", {}).get("drained")
            or any(item.get("status") != "stopped" for item in fence.get("writers", []))
            or fence.get("rechecked_at", 0) < receipt.get("checkpointed_at", 0)):
        fail("lifecycle fence was not rechecked after checkpoint")
    backup_by_path = {item["path"]: item for item in backup_objects}
    primary_by_path = {item["path"]: item for item in primary_objects}
    validate_inventory(backup, backup_objects, "live B after checkpoint")
    current_paths = set(object_paths(primary))
    expected_paths = set(primary_by_path) | set(backup_by_path)
    if not set(primary_by_path).issubset(current_paths) or not current_paths.issubset(expected_paths):
        fail("primary namespace drifted outside the resumable additive transition")
    for relative in sorted(current_paths):
        current_body = regular_identity(os.path.join(primary, relative))
        current_meta = regular_identity(metadata_path(primary, relative))
        before = primary_by_path.get(relative)
        incoming = backup_by_path.get(relative)
        allowed_bodies = [item["body"] for item in (before, incoming) if item is not None]
        allowed_metadata = [item["metadata"] for item in (before, incoming) if item is not None]
        if current_body not in allowed_bodies or current_meta not in allowed_metadata:
            fail("primary object drifted after its checkpoint: " + relative)
    receipt["status"] = "applying"
    atomic_json(receipt_path, receipt)
    for item in backup_objects:
        relative = item["path"]
        source = os.path.join(backup_snapshot, relative)
        destination = os.path.join(primary, relative)
        current = regular_identity(destination)
        before = primary_by_path.get(relative)
        before_body = before["body"] if before is not None else None
        if current != item["body"]:
            if current != before_body:
                fail("primary body changed outside the transaction: " + relative)
            clone_file(source, destination)
        source_meta = metadata_path(backup_snapshot, relative)
        destination_meta = metadata_path(primary, relative)
        current_meta = regular_identity(destination_meta)
        before_meta = before["metadata"] if before is not None else None
        if current_meta != item["metadata"]:
            if current_meta != before_meta:
                fail("primary metadata changed outside the transaction: " + relative)
            if item["metadata"] is None:
                os.unlink(destination_meta)
                fsync_dir(os.path.dirname(destination_meta))
            else:
                clone_file(source_meta, destination_meta)
        if regular_identity(destination) != item["body"]:
            fail("destination body did not verify: " + relative)
        if regular_identity(destination_meta) != item["metadata"]:
            fail("destination metadata did not verify: " + relative)
    validate_inventory(backup, backup_objects, "live B after apply")
    if set(object_paths(primary)) != expected_paths:
        fail("primary namespace does not equal the additive checkpoint union")
    for relative in sorted(expected_paths):
        expected = backup_by_path.get(relative) or primary_by_path[relative]
        if regular_identity(os.path.join(primary, relative)) != expected["body"]:
            fail("final primary body differs from additive checkpoint: " + relative)
        if regular_identity(metadata_path(primary, relative)) != expected["metadata"]:
            fail("final primary metadata differs from additive checkpoint: " + relative)
    receipt["status"] = "applied_pending_recovery"
    receipt["applied_at"] = time.time()
    receipt["verified_objects"] = len(backup_objects)
    receipt["primary_only_preserved"] = True
    receipt["backup_objects_not_written"] = True
    atomic_json(receipt_path, receipt)
    emit(receipt)
    raise SystemExit(0)

if phase != "finalize":
    fail("unknown reconciliation phase: " + phase)
if receipt.get("status") != "applied_pending_recovery":
    fail("reconciliation is not awaiting canonical recovery: " + str(receipt.get("status")))
current_primary_paths = set(object_paths(primary))
for decision in receipt.get("lifecycle_decisions", []):
    kind = decision.get("kind")
    job_id = decision.get("job_id")
    if kind == "queued_cancellation":
        queued = lifecycle_root + "queue/" + job_id + ".json"
        cancelled = lifecycle_root + "cancelled/" + job_id + ".json"
        if queued in current_primary_paths or cancelled not in current_primary_paths:
            fail("canonical queued cancellation recovery is incomplete: " + job_id)
    elif kind == "retained_outcome_cleanup":
        for candidate in decision["primary_only_paths"]:
            if candidate in current_primary_paths:
                fail("destination-only lifecycle projection remains: " + candidate)
        for companion in decision.get("transition_companions", []):
            try:
                with open(os.path.join(primary, companion), "r", encoding="utf-8") as handle:
                    transition = json.load(handle)
            except Exception as error:
                fail("cannot verify transition companion: " + str(error))
            if transition.get("state") != "retired":
                fail("canonical transition recovery is incomplete: " + companion)
    elif kind == "preserve_unresolved":
        continue
    else:
        fail("unknown typed lifecycle decision in receipt: " + str(kind))
receipt["status"] = "complete"
receipt["completed_at"] = time.time()
receipt["canonical_recovery_verified"] = True
atomic_json(receipt_path, receipt)
emit(receipt)
STADO_RECONCILE_EOF
"#;

const FENCE_SCHEMA: &str = "stado.storage-root-fence.v1";
const RECORD_FENCE: &str = "record-fence";
const READ_FENCE: &str = "read-fence";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WriterFence {
    target: String,
    label: String,
    was_running: bool,
    prior_pid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prior_started_at: Option<String>,
    prior_loaded_environment: std::collections::BTreeMap<String, String>,
    prior_executable: Option<String>,
    prior_sha256: Option<String>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_pid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_started_at: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    restored_loaded_environment: std::collections::BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_executable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueueFence {
    was_paused: bool,
    drained: bool,
    resumed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LifecycleFence {
    schema: String,
    transaction: String,
    status: String,
    queue: QueueFence,
    writers: Vec<WriterFence>,
    transport_retained: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository_runner_gate: Option<Value>,
    prepared_at: i64,
    rechecked_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_at: Option<i64>,
}

fn bind_remote_script(phase: &str, transaction: &str, fence: &str) -> String {
    REMOTE_SCRIPT
        .replace("@PHASE@", &shlex_quote(phase))
        .replace("@TRANSACTION@", &shlex_quote(transaction))
        .replace("@FENCE@", &shlex_quote(fence))
}

fn parse_remote_payload(output: &super::CommandOutput) -> Result<Value, DeployError> {
    let mut payload = None;
    for line in output.stdout.lines() {
        if let Some(message) = line.strip_prefix("STADO_STORAGE_RECONCILE_ERROR\t") {
            return Err(DeployError(message.to_string()));
        }
        if let Some(encoded) = line.strip_prefix("STADO_STORAGE_RECONCILE\t") {
            payload = serde_json::from_str(encoded).ok();
        }
    }
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            output,
            "storage reconciliation host program failed",
        )));
    }
    payload.ok_or_else(|| DeployError("storage reconciliation returned no payload".to_string()))
}

async fn read_fence(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<Option<LifecycleFence>, DeployError> {
    let output = host_channel::run_script_with_timeout(
        target,
        &bind_remote_script(READ_FENCE, transaction, ""),
        TIMEOUT,
        runner,
    )
    .await?;
    let value = parse_remote_payload(&output)?;
    if value.get("status").and_then(Value::as_str) == Some("absent") {
        return Ok(None);
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|error| DeployError(format!("invalid durable lifecycle fence: {error}")))
}

async fn write_fence(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    fence: &LifecycleFence,
    runner: &Runner,
) -> Result<(), DeployError> {
    let encoded = serde_json::to_string(fence)
        .map_err(|error| DeployError(format!("cannot encode lifecycle fence: {error}")))?;
    let output = host_channel::run_script_with_timeout(
        target,
        &bind_remote_script(RECORD_FENCE, transaction, &encoded),
        TIMEOUT,
        runner,
    )
    .await?;
    parse_remote_payload(&output)?;
    Ok(())
}
async fn repository_runner_gate() -> Result<Option<Value>, DeployError> {
    if std::env::var("STADO_RECONCILE_GITHUB_RUNNER_GATE").as_deref() != Ok("1") {
        return Ok(None);
    }
    let repository = std::env::var("GITHUB_REPOSITORY")
        .map_err(|_| DeployError("GITHUB_REPOSITORY is required for runner fencing".to_string()))?;
    let current_runner = std::env::var("RUNNER_NAME")
        .map_err(|_| DeployError("RUNNER_NAME is required for runner fencing".to_string()))?;
    let token = super::host_precheck_runner::github_credential().await?;
    let endpoint =
        format!("https://api.github.com/repos/{repository}/actions/runners?per_page=100");
    let response = reqwest::Client::new()
        .get(endpoint)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| DeployError(format!("cannot read repository runner fence: {error}")))?;
    if !response.status().is_success() {
        return Err(DeployError(format!(
            "repository runner fence returned HTTP {}",
            response.status()
        )));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|error| DeployError(format!("invalid repository runner fence: {error}")))?;
    let runners = body
        .get("runners")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("repository runner fence omitted runners".to_string()))?;
    let total_count = body
        .get("total_count")
        .and_then(Value::as_u64)
        .ok_or_else(|| DeployError("repository runner fence omitted total_count".to_string()))?;
    if total_count != runners.len() as u64 {
        return Err(DeployError(format!(
            "repository runner fence was incomplete: returned {} of {total_count}",
            runners.len()
        )));
    }
    let mut current_online_busy = false;
    let mut other_busy = Vec::new();
    for runner in runners {
        let name = runner.get("name").and_then(Value::as_str).unwrap_or_default();
        let busy = runner.get("busy").and_then(Value::as_bool) == Some(true);
        let online = runner.get("status").and_then(Value::as_str) == Some("online");
        if name == current_runner {
            current_online_busy |= online && busy;
        } else if busy {
            other_busy.push(name.to_string());
        }
    }
    if !current_online_busy || !other_busy.is_empty() {
        return Err(DeployError(format!(
            "repository runner fence refused: current_online_busy={current_online_busy}, other_busy={other_busy:?}"
        )));
    }
    Ok(Some(json!({
        "repository": repository,
        "current_runner": current_runner,
        "current_online_busy": true,
        "total_count": total_count,
        "other_busy": other_busy,
        "checked_at": Utc::now().timestamp(),
    })))
}

fn transport_service(service: &service::ManagedService) -> bool {
    let identity = format!(
        "{} {} {}",
        service.unit_id(),
        service.program,
        service.args.join(" ")
    )
    .to_ascii_lowercase();
    [
        "resolver",
        "release-agent",
        "runner",
        "tailscale",
        "caddy",
        "proxy",
        "forward",
        "skarbiec",
    ]
    .iter()
    .any(|word| identity.contains(word))
}

async fn registry_services(
    storage_target_name: &str,
) -> Result<Vec<(crate::targets::ComputeTarget, service::ManagedService)>, DeployError> {
    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(|error| DeployError(format!("cannot load registry for lifecycle fence: {error}")))?;
    let mut result = Vec::new();
    for target in registry
        .targets
        .into_iter()
        .filter(|target| target.name == storage_target_name)
    {
        for declared in service::declared_services(&target) {
            result.push((target.clone(), declared));
        }
    }
    Ok(result)
}

async fn prepare_lifecycle_fence(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<LifecycleFence, DeployError> {
    let services = registry_services(&storage_target.name).await?;
    let mut fence = match read_fence(storage_target, transaction, runner).await? {
        Some(existing) => existing,
        None => {
            let store = crate::queue::JobStorage::new()
                .await
                .map_err(|error| DeployError(format!("cannot read queue before fencing: {error}")))?;
            let prior = crate::queue::control::read(&store)
                .await
                .map_err(|error| DeployError(format!("cannot read prior queue state: {error}")))?;
            let mut writers = Vec::new();
            let mut transport_retained = Vec::new();
            for (target, declared) in &services {
                let state = super::service_label_print::print_label(
                    target,
                    declared.unit_id(),
                    service::BootoutScope::Any,
                    runner,
                )
                .await?;
                if declared.unit_id().contains("object-api")
                    && (state.pid.is_none()
                        || state.process_started_at.is_none()
                        || state.process_executable.is_none()
                        || state.process_sha256.is_none()
                        || [
                            "WC_STORAGE_BACKEND",
                            "WC_LOCAL_STORAGE_PATH",
                            "WC_BACKUP_LOCAL_STORAGE_PATH",
                            "STADO_CONFIG",
                        ]
                        .iter()
                        .any(|key| {
                            state
                                .loaded_environment
                                .get(*key)
                                .is_none_or(String::is_empty)
                        }))
                {
                    return Err(DeployError(format!(
                        "{} cannot be fenced without its loaded routing, pid, start, and process image",
                        declared.unit_id()
                    )));
                }
                if transport_service(declared) {
                    transport_retained.push(state.to_json());
                } else {
                    let was_running = state.pid.is_some();
                    writers.push(WriterFence {
                        target: target.name.clone(),
                        label: declared.unit_id().to_string(),
                        was_running,
                        prior_pid: state.pid,
                        prior_started_at: state.process_started_at,
                        prior_loaded_environment: state.loaded_environment,
                        prior_executable: state.process_executable,
                        prior_sha256: state.process_sha256,
                        status: if was_running {
                            "pending".to_string()
                        } else {
                            "stopped".to_string()
                        },
                        restored_pid: None,
                        restored_started_at: None,
                        restored_loaded_environment: std::collections::BTreeMap::new(),
                        restored_executable: None,
                        restored_sha256: None,
                    });
                }
            }
            let initial = LifecycleFence {
                schema: FENCE_SCHEMA.to_string(),
                transaction: transaction.to_string(),
                status: "preparing".to_string(),
                queue: QueueFence {
                    was_paused: prior.paused,
                    drained: false,
                    resumed: false,
                },
                writers,
                transport_retained,
                repository_runner_gate: None,
                prepared_at: Utc::now().timestamp(),
                rechecked_at: 0,
                restored_at: None,
            };
            write_fence(storage_target, transaction, &initial, runner).await?;
            initial
        }
    };
    if fence.schema != FENCE_SCHEMA || fence.transaction != transaction {
        return Err(DeployError(
            "durable lifecycle fence belongs to another transaction".to_string(),
        ));
    }
    if fence.status == "restored" {
        return Ok(fence);
    }
    if fence.status == "fenced" {
        return recheck_lifecycle_fence(storage_target, transaction, runner).await;
    }

    if !fence.queue.drained {
        if let Some(runner_gate) = repository_runner_gate().await? {
            fence.repository_runner_gate = Some(runner_gate);
            write_fence(storage_target, transaction, &fence, runner).await?;
        }
        let store = crate::queue::JobStorage::new()
            .await
            .map_err(|error| DeployError(format!("cannot pause queue for fencing: {error}")))?;
        let current = crate::queue::control::read(&store)
            .await
            .map_err(|error| DeployError(format!("cannot recheck queue fence: {error}")))?;
        if !current.paused {
            crate::queue::control::set_paused(
                &store,
                true,
                &format!("storage reconciliation {transaction}"),
                "stado storage-root-reconcile",
            )
            .await
            .map_err(|error| DeployError(format!("cannot pause queue for fencing: {error}")))?;
        }
        let deadline =
            Instant::now() + Duration::from_secs(crate::queue::control::default_drain_timeout_s());
        while !crate::queue::control::is_drained(&store)
            .await
            .map_err(|error| DeployError(format!("cannot prove queue drained: {error}")))?
        {
            if Instant::now() >= deadline {
                return Err(DeployError(
                    "queue remained active until the canonical drain deadline; fence retained"
                        .to_string(),
                ));
            }
            sleep(Duration::from_secs(5)).await;
        }
        fence.queue.drained = true;
        write_fence(storage_target, transaction, &fence, runner).await?;
    }

    for index in 0..fence.writers.len() {
        if fence.writers[index].status == "stopped" {
            continue;
        }
        let target_name = fence.writers[index].target.clone();
        let label = fence.writers[index].label.clone();
        let (target, declared) = services
            .iter()
            .find(|(target, declared)| {
                target.name == target_name && declared.unit_id() == label
            })
            .ok_or_else(|| {
                DeployError(format!(
                    "writer {label} on {target_name} disappeared from the registry"
                ))
            })?;
        let stopped = service::stop_service(target, declared, runner).await?;
        if !stopped.succeeded("stopped") {
            return Err(DeployError(format!(
                "{label} on {target_name} did not stop: {}",
                stopped.failure()
            )));
        }
        fence.writers[index].status = "stopped".to_string();
        write_fence(storage_target, transaction, &fence, runner).await?;
    }
    for writer in &fence.writers {
        let (target, _) = services
            .iter()
            .find(|(target, declared)| {
                target.name == writer.target && declared.unit_id() == writer.label
            })
            .ok_or_else(|| DeployError("writer declaration disappeared during fence".to_string()))?;
        let state = super::service_label_print::print_label(
            target,
            &writer.label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        if state.pid.is_some() {
            return Err(DeployError(format!(
                "writer {} on {} still has pid {}",
                writer.label,
                writer.target,
                state.pid.unwrap_or_default()
            )));
        }
    }
    fence.status = "fenced".to_string();
    fence.rechecked_at = Utc::now().timestamp();
    write_fence(storage_target, transaction, &fence, runner).await?;
    Ok(fence)
}

async fn recheck_lifecycle_fence(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<LifecycleFence, DeployError> {
    let mut fence = read_fence(storage_target, transaction, runner)
        .await?
        .ok_or_else(|| DeployError("durable lifecycle fence is absent".to_string()))?;
    if fence.status != "fenced" || !fence.queue.drained {
        return Err(DeployError(
            "durable lifecycle fence is not in the fenced/drained state".to_string(),
        ));
    }
    let services = registry_services(&storage_target.name).await?;
    for writer in &fence.writers {
        let (target, _) = services
            .iter()
            .find(|(target, declared)| {
                target.name == writer.target && declared.unit_id() == writer.label
            })
            .ok_or_else(|| DeployError("writer declaration disappeared during fence".to_string()))?;
        let state = super::service_label_print::print_label(
            target,
            &writer.label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        if state.pid.is_some() {
            return Err(DeployError(format!(
                "writer {} on {} resumed during the storage fence",
                writer.label, writer.target
            )));
        }
    }
    for retained in &fence.transport_retained {
        let target_name = retained.get("host").and_then(Value::as_str).unwrap_or_default();
        let label = retained.get("label").and_then(Value::as_str).unwrap_or_default();
        let (target, _) = services
            .iter()
            .find(|(target, declared)| {
                target.name == target_name && declared.unit_id() == label
            })
            .ok_or_else(|| DeployError("retained transport declaration disappeared".to_string()))?;
        let state = super::service_label_print::print_label(
            target,
            label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        if state.pid.is_none() {
            return Err(DeployError(format!(
                "retained transport {label} on {target_name} is no longer running"
            )));
        }
    }
    fence.rechecked_at = Utc::now().timestamp();
    write_fence(storage_target, transaction, &fence, runner).await?;
    Ok(fence)
}

async fn restore_lifecycle_fence(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<LifecycleFence, DeployError> {
    let mut fence = read_fence(storage_target, transaction, runner)
        .await?
        .ok_or_else(|| DeployError("durable lifecycle fence is absent".to_string()))?;
    if !matches!(fence.status.as_str(), "fenced" | "restoring" | "restored") {
        return Err(DeployError(format!(
            "lifecycle fence cannot be restored from {}",
            fence.status
        )));
    }
    if fence.status == "restored" {
        return Ok(fence);
    }
    let services = registry_services(&storage_target.name).await?;
    fence.status = "restoring".to_string();
    write_fence(storage_target, transaction, &fence, runner).await?;
    for index in 0..fence.writers.len() {
        if !fence.writers[index].was_running {
            fence.writers[index].status = "restored_stopped".to_string();
            continue;
        }
        if fence.writers[index].status == "restored" {
            continue;
        }
        let target_name = fence.writers[index].target.clone();
        let label = fence.writers[index].label.clone();
        let prior_pid = fence.writers[index].prior_pid.clone();
        let (target, declared) = services
            .iter()
            .find(|(target, declared)| {
                target.name == target_name && declared.unit_id() == label
            })
            .ok_or_else(|| {
                DeployError(format!(
                    "writer {label} on {target_name} disappeared before restore"
                ))
            })?;
        if label.contains("object-api") {
            let recovered = host_channel::run_script_with_timeout(
                target,
                include_str!("../../../deploy/recover_object_api.sh"),
                Duration::from_secs(240),
                runner,
            )
            .await?;
            if !recovered.ok() {
                return Err(DeployError(format!(
                    "{label} on {target_name} did not restore through canonical object recovery: {}",
                    host_channel::last_error_line(&recovered, "remote command failed")
                )));
            }
        } else if declared.program.is_empty() {
            let restarted = service::restart_service(target, declared, runner).await?;
            if !restarted.succeeded("restarted") {
                return Err(DeployError(format!(
                    "{label} on {target_name} did not restore: {}",
                    restarted.failure()
                )));
            }
        } else {
            let extra_env = declared
                .env
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect::<Vec<_>>();
            let plan = service::plan_deploy_labelled(
                target,
                &declared.name,
                declared.unit_id(),
                &declared.program,
                &declared.args,
                &extra_env,
            )?;
            let ensured = service::ensure_service(target, &plan, runner).await?;
            if !ensured.succeeded() {
                return Err(DeployError(format!(
                    "{label} on {target_name} did not restore from its declaration: {}",
                    ensured.report.failure()
                )));
            }
        }
        let state = super::service_label_print::print_label(
            target,
            &label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        let fresh_pid = state.pid.clone().ok_or_else(|| {
            DeployError(format!("{label} on {target_name} restored without a process"))
        })?;
        if prior_pid.as_deref() == Some(fresh_pid.as_str()) {
            return Err(DeployError(format!(
                "{label} on {target_name} did not start a fresh process"
            )));
        }
        if label.contains("object-api") {
            let loaded = &state.loaded_environment;
            let primary = loaded
                .get("WC_LOCAL_STORAGE_PATH")
                .map(String::as_str)
                .unwrap_or_default();
            let backup = loaded
                .get("WC_BACKUP_LOCAL_STORAGE_PATH")
                .map(String::as_str)
                .unwrap_or_default();
            if loaded.get("WC_STORAGE_BACKEND").map(String::as_str) != Some("local")
                || !primary.ends_with("/.stado/local-storage")
                || !backup.ends_with("/.stado/local-backup")
                || loaded.get("STADO_CONFIG").is_none_or(String::is_empty)
            {
                return Err(DeployError(format!(
                    "{label} restored with a loaded environment that does not route A as primary and B as backup"
                )));
            }
            if state.process_started_at.is_none()
                || state.process_executable.is_none()
                || state.process_sha256.is_none()
            {
                return Err(DeployError(format!(
                    "{label} restored without process start and immutable running-image identity"
                )));
            }
        }
        fence.writers[index].restored_pid = Some(fresh_pid);
        fence.writers[index].restored_started_at = state.process_started_at;
        fence.writers[index].restored_loaded_environment = state.loaded_environment;
        fence.writers[index].restored_executable = state.process_executable;
        fence.writers[index].restored_sha256 = state.process_sha256;
        fence.writers[index].status = "restored".to_string();
        write_fence(storage_target, transaction, &fence, runner).await?;
    }
    if !fence.queue.was_paused && !fence.queue.resumed {
        let store = crate::queue::JobStorage::new()
            .await
            .map_err(|error| DeployError(format!("cannot restore queue state: {error}")))?;
        crate::queue::control::set_paused(
            &store,
            false,
            &format!("storage reconciliation {transaction} complete"),
            "stado storage-root-reconcile",
        )
        .await
        .map_err(|error| DeployError(format!("cannot resume queue after restore: {error}")))?;
        fence.queue.resumed = true;
    }
    fence.status = "restored".to_string();
    fence.restored_at = Some(Utc::now().timestamp());
    write_fence(storage_target, transaction, &fence, runner).await?;
    Ok(fence)
}

fn validate_transaction(transaction: &str) -> Result<(), DeployError> {
    if transaction.is_empty()
        || transaction.len() > 96
        || !transaction
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(DeployError(
            "transaction must contain 1-96 ASCII letters, digits, or '-'".to_string(),
        ));
    }
    Ok(())
}

pub async fn reconcile_host(
    target_name: &str,
    transaction: &str,
    phase: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    validate_transaction(transaction)?;
    if !matches!(phase, CHECKPOINT | APPLY | FINALIZE) {
        return Err(DeployError(format!(
            "phase must be {CHECKPOINT}, {APPLY}, or {FINALIZE}, not {phase:?}"
        )));
    }
    let target = host_channel::canonical_target(target_name).await?;
    let lifecycle_fence = match phase {
        CHECKPOINT => prepare_lifecycle_fence(&target, transaction, runner).await?,
        APPLY => recheck_lifecycle_fence(&target, transaction, runner).await?,
        FINALIZE => restore_lifecycle_fence(&target, transaction, runner).await?,
        _ => unreachable!("phase validated above"),
    };
    let script = bind_remote_script(phase, transaction, "");
    let output = host_channel::run_script_with_timeout(&target, &script, TIMEOUT, runner).await?;
    let mut payload = None;
    let mut refusal = None;
    for line in output.stdout.lines() {
        if let Some(encoded) = line.strip_prefix("STADO_STORAGE_RECONCILE\t") {
            payload = serde_json::from_str::<Value>(encoded).ok();
        } else if let Some(message) = line.strip_prefix("STADO_STORAGE_RECONCILE_ERROR\t") {
            refusal = Some(message.to_string());
        }
    }
    let mut report = host_channel::base_report(&target);
    report.insert("transaction".to_string(), json!(transaction));
    report.insert("phase".to_string(), json!(phase));
    report.insert("receipt".to_string(), payload.unwrap_or(Value::Null));
    report.insert(
        "lifecycle_fence".to_string(),
        serde_json::to_value(lifecycle_fence)
            .map_err(|error| DeployError(format!("cannot report lifecycle fence: {error}")))?,
    );
    if let Some(reason) = refusal {
        report.insert("status".to_string(), json!("refused"));
        report.insert("error".to_string(), json!(reason));
    } else if output.ok() && !report["receipt"].is_null() {
        report.insert("status".to_string(), json!("ok"));
    } else {
        report.insert("status".to_string(), json!("failed"));
        report.insert(
            "error".to_string(),
            json!(host_channel::last_error_line(
                &output,
                "storage-root reconciliation did not return a receipt"
            )),
        );
    }
    Ok(Value::Object(report))
}
