//! Interruption-safe reconciliation of the two fixed co-located local object roots.
//!
//! This is deliberately not `host object-relocate`: relocation moves one in-store
//! address and refuses overwrites. This transaction checkpoints both physical
//! roots with copy-on-write clones, then additively makes `local-storage`
//! contain `local-backup`'s exact objects and effective metadata. Backup bytes
//! and primary-only objects are never removed. The immutable full-primary
//! checkpoint retains conflicting primary bytes before the backup-winning
//! value is installed.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::{sleep, Instant};

use base64::Engine;
use super::{host_channel, service};
use super::{shlex_quote, DeployError, Runner};
use sha2::{Digest, Sha256};

pub const CHECKPOINT: &str = "checkpoint";
pub const APPLY: &str = "apply";
pub const FINALIZE: &str = "finalize";
pub const ACTIVATE: &str = "activate";
pub const ROLLBACK: &str = "rollback";
pub const STATUS: &str = "status";
pub const RUN: &str = "run";
pub const RESUME: &str = "resume";
const TIMEOUT: Duration = Duration::from_secs(60 * 60);
const PREFLIGHT: &str = "preflight";
const RECORD_LIFECYCLE_DECISIONS: &str = "record-lifecycle-decisions";
static RESIDENT_OWNER_TOKEN: OnceLock<String> = OnceLock::new();
static RESIDENT_RUNNER_GATE: OnceLock<Value> = OnceLock::new();
static RESIDENT_LOCK_FD: OnceLock<i32> = OnceLock::new();
static RESIDENT_TARGET: OnceLock<crate::targets::ComputeTarget> = OnceLock::new();
const ROLLBACK_OBJECT_API_SCRIPT: &str = r#"set -euo pipefail
if [ "$(/usr/bin/uname -s)" != Darwin ]; then
  printf 'unsupported_os\n' >&2
  exit 65
fi
label=com.wisent.always-on.stado-object-api
plist="/Library/LaunchDaemons/$label.plist"
program="$HOME/.stado/bin/stado"
store=@PRIMARY@
backup_backend=@BACKUP_BACKEND@
backup_store=@BACKUP@
config=@CONFIG@
port=@PORT@
log="$HOME/.stado/logs/$label.log"
work="$HOME/.stado/work/object-api-recovery"
[ -x "$program" ] && [ -d "$store" ] && [ -r "$store/registry.json" ]
if [ -n "$backup_store" ]; then
  [ "$backup_backend" = local ] && [ -d "$backup_store" ] && [ -r "$backup_store/registry.json" ]
fi
/bin/mkdir -p "$work" "$HOME/.stado/logs"
/bin/chmod 700 "$work" "$HOME/.stado/logs"
/usr/bin/touch "$log"
/bin/chmod 600 "$log"
staged=$(/usr/bin/mktemp "$work/$label.captured-prior.XXXXXX")
trap '/bin/rm -f "$staged"' EXIT HUP INT TERM
account=$(/usr/bin/id -un)
/usr/bin/python3 - "$staged" "$label" "$program" "$store" "$backup_backend" "$backup_store" "$account" "$log" "$HOME" "$config" "$port" <<'PY'
import plistlib, sys
path, label, program, store, backup_backend, backup_store, account, log, home, config, port = sys.argv[1:]
environment = {
    "HOME": home,
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    "STADO_CONFIG": config,
    "GNUPGHOME": f"{home}/.gnupg",
    "SKARBIEC_VAULT_FILE": f"{home}/.stado/skarbiec.vault.json",
    "WC_OBJECT_SKARBIEC_TOKEN_FILE": f"{home}/.stado/stado-object-api-verifier-skarbiec-token",
    "WC_RELEASE_SKARBIEC_TOKEN_FILE": f"{home}/.stado/stado-release-api-verifier-skarbiec-token",
    "WC_STORAGE_BACKEND": "local",
    "WC_LOCAL_STORAGE_PATH": store,
}
if backup_store:
    environment["WC_BACKUP_STORAGE_BACKEND"] = backup_backend
    environment["WC_BACKUP_LOCAL_STORAGE_PATH"] = backup_store
document = {
    "Label": label,
    "ProgramArguments": [program, "dashboard", "--bind", "127.0.0.1", "--port", port],
    "EnvironmentVariables": environment,
    "RunAtLoad": True,
    "KeepAlive": True,
    "UserName": account,
    "StandardOutPath": log,
    "StandardErrorPath": log,
}
with open(path, "wb") as handle:
    plistlib.dump(document, handle, fmt=plistlib.FMT_XML, sort_keys=False)
PY
/usr/bin/plutil -lint "$staged" >/dev/null
/usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel "$staged" "$plist"
/usr/bin/sudo -n /bin/launchctl bootout "system/$label" >/dev/null 2>&1 || true
/usr/bin/sudo -n /bin/launchctl enable "system/$label"
/usr/bin/sudo -n /bin/launchctl bootstrap system "$plist"
printf 'STADO_OBJECT_API_ROUTE\tcaptured-prior\n'
"#;

const REMOTE_SCRIPT: &str = r#"set -u
STADO_RECONCILE_PHASE=@PHASE@ STADO_RECONCILE_TX=@TRANSACTION@ STADO_RECONCILE_FENCE=@FENCE@ STADO_RECONCILE_OWNER_TOKEN=@OWNER_TOKEN@ STADO_RECONCILE_LOCK_FD=@LOCK_FD@ /usr/bin/python3 - <<'STADO_RECONCILE_EOF'
import ctypes, datetime, fcntl, hashlib, json, os, stat, sys, time

phase = os.environ["STADO_RECONCILE_PHASE"]
tx = os.environ["STADO_RECONCILE_TX"]
home = os.path.expanduser("~")
primary = os.path.join(home, ".stado", "local-storage")
backup = os.path.join(home, ".stado", "local-backup")
work = os.path.join(home, ".stado", "recovery", "storage-root-reconcile", tx)
backup_snapshot = os.path.join(work, "local-backup.checkpoint")
primary_snapshot = os.path.join(work, "local-storage.checkpoint")
effective_lifecycle_snapshot = os.path.join(work, "effective-lifecycle.checkpoint")
owner_path = os.path.join(work, "operation-owner.json")
owner_token = os.environ.get("STADO_RECONCILE_OWNER_TOKEN", "")
receipt_path = os.path.join(work, "receipt.json")
fence_path = os.path.join(work, "lifecycle-fence.json")
lock_path = os.path.join(home, ".stado", "recovery", "storage-root-reconcile.lock")
schema = "stado.storage-root-reconcile.v1"
staging = os.path.join(work, ".clone-staging")
fence_payload = os.environ.get("STADO_RECONCILE_FENCE", "")
lifecycle_root = "ecosystem/probierz/"
transition_retired_state = @TRANSITION_RETIRED_STATE@


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


def metadata_paths(root):
    metadata_root = os.path.join(root, ".metadata")
    ecosystem = os.path.join(metadata_root, "ecosystem")
    if not os.path.exists(ecosystem):
        return []
    if not os.path.isdir(ecosystem) or os.path.islink(ecosystem):
        fail("unsafe metadata ecosystem root: " + ecosystem)
    result = []
    errors = []
    def onerror(error):
        errors.append(error)
    for directory, dirs, files in os.walk(ecosystem, followlinks=False, onerror=onerror):
        for name in dirs:
            if os.path.islink(os.path.join(directory, name)):
                fail("symlinked metadata directory: " + os.path.join(directory, name))
        for name in files:
            path = os.path.join(directory, name)
            info = os.lstat(path)
            if not stat.S_ISREG(info.st_mode):
                fail("non-regular metadata object: " + path)
            result.append(os.path.relpath(path, metadata_root))
    if errors:
        fail("metadata enumeration failed: " + str(errors[0]))
    return sorted(result)


def physical_inventory(root):
    files = []
    directories = []
    errors = []
    def onerror(error):
        errors.append(error)
    for directory, dirs, names in os.walk(root, followlinks=False, onerror=onerror):
        relative_directory = os.path.relpath(directory, root)
        if relative_directory != ".":
            directories.append(relative_directory)
        for name in dirs:
            path = os.path.join(directory, name)
            if os.path.islink(path):
                fail("symlinked physical-root directory: " + path)
        for name in names:
            path = os.path.join(directory, name)
            info = os.lstat(path)
            if not stat.S_ISREG(info.st_mode):
                fail("non-regular physical-root entry: " + path)
            files.append({
                "path": os.path.relpath(path, root),
                "body": {"bytes": info.st_size, "sha256": digest(path)},
                "mode": stat.S_IMODE(info.st_mode),
            })
    if errors:
        fail("physical-root enumeration failed: " + str(errors[0]))
    return {
        "files": sorted(files, key=lambda item: item["path"]),
        "directories": sorted(directories),
        "exclusions": [],
    }


def validate_physical_checkpoint(root, snapshot, label):
    current = physical_inventory(root)
    expected_files = [{"path": item["path"], "body": item["body"]}
                      for item in snapshot["files"]]
    current_files = [{"path": item["path"], "body": item["body"]}
                     for item in current["files"]]
    if (current_files != expected_files
            or current["directories"] != snapshot["directories"]):
        fail(label + " physical root changed")


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
if phase not in ("read-fence", "status"):
    try:
        lock_fd = int(os.environ.get("STADO_RECONCILE_LOCK_FD", "-1"))
        descriptor = os.fstat(lock_fd)
        canonical = os.lstat(lock_path)
        with open(owner_path, "r", encoding="utf-8") as handle:
            owner = json.load(handle)
        if (lock_fd < 0
                or descriptor.st_dev != canonical.st_dev
                or descriptor.st_ino != canonical.st_ino
                or owner.get("schema") != "stado.storage-root-owner.v1"
                or owner.get("transaction") != tx
                or owner.get("token") != owner_token
                or owner.get("status") != "executing"):
            fail("resident transaction owner and inherited OS lock do not authorize this effect")
        fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except Exception as error:
        fail("resident transaction lock proof is unavailable: " + str(error))

if phase == "read-fence":
    try:
        with open(fence_path, "r", encoding="utf-8") as handle:
            fence = json.load(handle)
    except FileNotFoundError:
        fence = {"schema": "stado.storage-root-fence.v3", "transaction": tx,
                 "status": "absent", "writers": []}
    print("STADO_STORAGE_RECONCILE\t" +
          json.dumps(fence, sort_keys=True, separators=(",", ":")))
    raise SystemExit(0)


if phase == "record-fence":
    try:
        fence = json.loads(fence_payload)
    except Exception as error:
        fail("lifecycle fence payload is invalid: " + str(error))
    if (fence.get("schema") != "stado.storage-root-fence.v3"
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
        "backup_physical_files": len(receipt.get("backup_physical", {}).get("files", [])),
        "primary_physical_files": len(receipt.get("primary_physical", {}).get("files", [])),
        "physical_snapshot_exclusions": receipt.get("physical_snapshot_exclusions", []),
        "lifecycle_decisions": {
            "queued_cancellation": sum(1 for item in decisions
                                        if item.get("kind") == "queued_cancellation"),
            "retained_outcome_cleanup": sum(1 for item in decisions
                                             if item.get("kind") == "retained_outcome_cleanup"),
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


if phase == "preflight":
    backup_paths = object_paths(backup)
    primary_paths = object_paths(primary)
    backup_physical = physical_inventory(backup)
    primary_physical = physical_inventory(primary)
    print("STADO_STORAGE_RECONCILE\t" + json.dumps({
        "schema": schema,
        "transaction": tx,
        "status": "observed",
        "observed_at": time.time(),
        "backup_qualified": inventory(backup, backup_paths),
        "primary_qualified": inventory(primary, primary_paths),
        "backup_physical": backup_physical,
        "primary_physical": primary_physical,
        "physical_snapshot_exclusions": [],
    }, sort_keys=True, separators=(",", ":")))
    raise SystemExit(0)

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


def checkpoint_effective_lifecycle(primary_objects, backup_objects):
    selected = {}
    for source_root, objects in (
            (primary_snapshot, primary_objects), (backup_snapshot, backup_objects)):
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

if phase == "checkpoint":
    try:
        with open(fence_path, "r", encoding="utf-8") as handle:
            fence = json.load(handle)
    except Exception as error:
        fail("durable lifecycle fence is absent or unreadable: " + str(error))
    if (fence.get("schema") != "stado.storage-root-fence.v3"
            or fence.get("transaction") != tx or fence.get("status") != "fenced"
            or not fence.get("queue", {}).get("drained")
            or not (fence.get("staged_runtime") or {}).get("staged_sha256")
            or not fence.get("rechecked_at")):
        fail("durable lifecycle fence is incomplete")
    if any(item.get("status") != "stopped" for item in fence.get("writers", [])):
        fail("durable lifecycle fence does not stop every recorded writer")
    receipt = load_receipt() if os.path.exists(receipt_path) else None
    if receipt is not None and receipt.get("status") in (
        "checkpoint_ready", "applying", "data_committed_pending_activation",
        "activated_pending_lifecycle", "complete"
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
        backup_physical = physical_inventory(backup)
        primary_physical = physical_inventory(primary)
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
            "backup_objects": backup_objects,
            "primary_objects": primary_objects,
            "backup_physical": backup_physical,
            "primary_physical": primary_physical,
            "physical_snapshot_exclusions": [],
            "snapshot_scope": "full_physical_roots",
            "handoff_scope": "ecosystem/ qualified objects and matching .metadata/ecosystem sidecars",
            "lifecycle_decisions": [],
        }
        atomic_json(receipt_path, receipt)
    else:
        backup_objects = receipt.get("backup_objects")
        primary_objects = receipt.get("primary_objects")
        backup_physical = receipt.get("backup_physical")
        primary_physical = receipt.get("primary_physical")
        if (not isinstance(backup_objects, list)
                or not isinstance(primary_objects, list)
                or not isinstance(backup_physical, dict)
                or not isinstance(primary_physical, dict)):
            fail("checkpoint receipt inventories are invalid")
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
    checkpoint_effective_lifecycle(primary_objects, backup_objects)
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
validate_complete_inventory(backup_snapshot, backup_objects, "backup checkpoint")
validate_complete_inventory(primary_snapshot, primary_objects, "primary checkpoint")
if phase == "record-lifecycle-decisions":
    if receipt.get("status") not in (
            "checkpoint_ready", "applying", "data_committed_pending_activation"):
        fail("lifecycle decisions require an immutable checkpoint before runtime activation")
    try:
        decisions = json.loads(fence_payload)
    except Exception as error:
        fail("typed lifecycle decisions are invalid: " + str(error))
    if not isinstance(decisions, list):
        fail("typed lifecycle decisions are not a list")
    encoded_decisions = json.dumps(decisions, sort_keys=True, separators=(",", ":")).encode("utf-8")
    decision_sha256 = hashlib.sha256(encoded_decisions).hexdigest()
    existing_decisions = receipt.get("lifecycle_decisions")
    existing_validation = receipt.get("lifecycle_validation")
    if existing_decisions is not None and existing_decisions != decisions:
        fail("typed lifecycle decisions changed after their durable result")
    if existing_validation is not None and (
            existing_validation.get("engine") != "stado.typed-lifecycle-snapshot.v1"
            or existing_validation.get("sha256") != decision_sha256):
        fail("typed lifecycle validation proof changed after its durable result")
    receipt["lifecycle_decisions"] = decisions
    if existing_validation is None:
        receipt["lifecycle_validation"] = {
            "engine": "stado.typed-lifecycle-snapshot.v1",
            "sha256": decision_sha256,
            "validated_at": time.time(),
        }
    atomic_json(receipt_path, receipt)
    emit(receipt)
    raise SystemExit(0)


def prove_live_additive_union(label):
    backup_by_path = {item["path"]: item for item in backup_objects}
    primary_by_path = {item["path"]: item for item in primary_objects}
    expected_paths = set(primary_by_path) | set(backup_by_path)
    validate_complete_inventory(backup, backup_objects, "live B " + label)
    if set(object_paths(primary)) != expected_paths:
        fail("primary namespace does not equal the additive checkpoint union " + label)
    for relative in sorted(expected_paths):
        expected = backup_by_path.get(relative) or primary_by_path[relative]
        if regular_identity(os.path.join(primary, relative)) != expected["body"]:
            fail("primary body differs from additive checkpoint " + label + ": " + relative)
        if regular_identity(metadata_path(primary, relative)) != expected["metadata"]:
            fail("primary metadata differs from additive checkpoint " + label + ": " + relative)
    return backup_by_path, primary_by_path, expected_paths



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
    if (fence.get("status") != "fenced" or not fence.get("queue", {}).get("drained")
            or any(item.get("status") != "stopped" for item in fence.get("writers", []))
            or fence.get("rechecked_at", 0) < receipt.get("checkpointed_at", 0)):
        fail("lifecycle fence was not rechecked after checkpoint")
    validation = receipt.get("lifecycle_validation")
    if (not isinstance(validation, dict)
            or validation.get("engine") != "stado.typed-lifecycle-snapshot.v1"):
        fail("typed Rust lifecycle validation is absent")
    blockers = [item for item in receipt.get("lifecycle_decisions", [])
                if item.get("kind") == "block_unclassified_live"]
    if blockers:
        fail("A-only lifecycle state blocks activation: " +
             ", ".join(item.get("path", "?") for item in blockers))
    backup_by_path = {item["path"]: item for item in backup_objects}
    primary_by_path = {item["path"]: item for item in primary_objects}
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
    prove_live_additive_union("after apply")
    receipt["status"] = "data_committed_pending_activation"
    receipt["data_committed_at"] = time.time()
    receipt["verified_objects"] = len(backup_objects)
    receipt["primary_only_preserved"] = True
    receipt["backup_objects_not_written"] = True
    atomic_json(receipt_path, receipt)
    emit(receipt)
    raise SystemExit(0)

if phase == "activate":
    if receipt.get("status") == "activated_pending_lifecycle":
        prove_live_additive_union("while resuming activated lifecycle")
        emit(receipt)
        raise SystemExit(0)
    if receipt.get("status") != "data_committed_pending_activation":
        fail("reconciliation has no committed data proof for runtime activation: " + str(receipt.get("status")))
    try:
        with open(fence_path, "r", encoding="utf-8") as handle:
            fence = json.load(handle)
    except Exception as error:
        fail("activated lifecycle fence cannot be read: " + str(error))
    active_path = os.path.expanduser("~/.stado/bin/stado")
    expected_digest = fence.get("activation_sha256")
    if (fence.get("schema") != "stado.storage-root-fence.v3"
            or fence.get("status") != "activated"
            or not fence.get("queue", {}).get("resumed")
            or not fence.get("restored_at")
            or not isinstance(expected_digest, str)
            or len(expected_digest) != 64
            or os.path.islink(active_path)
            or not os.path.isfile(active_path)
            or digest(active_path) != expected_digest):
        fail("runtime activation and lifecycle restoration are not durably proved")
    if any(item.get("status") != "restored" for item in fence.get("writers", [])):
        fail("activated fence does not restore every captured native service state")
    prove_live_additive_union("after activation")
    receipt["status"] = "activated_pending_lifecycle"
    receipt["activated_at"] = fence.get("activated_at")
    receipt["activated_sha256"] = expected_digest
    atomic_json(receipt_path, receipt)
    emit(receipt)
    raise SystemExit(0)

if phase != "finalize":
    fail("unknown reconciliation phase: " + phase)
if receipt.get("status") != "activated_pending_lifecycle":
    fail("reconciliation is not awaiting lifecycle cleanup: " + str(receipt.get("status")))
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
            if transition.get("state") != transition_retired_state:
                fail("canonical transition recovery is incomplete: " + companion)
    elif kind == "terminal_run_recovery":
        relative = decision.get("path")
        try:
            with open(os.path.join(primary, relative), "r", encoding="utf-8") as handle:
                manifest = json.load(handle)
        except Exception as error:
            fail("cannot verify canonically retained run: " + str(error))


        entries = manifest.get("entries") if isinstance(manifest, dict) else None
        if (not manifest.get("cleanup_completed_at")
                or not isinstance(entries, list) or not entries
                or any(
                    not isinstance(entry, dict)
                    or entry.get("state") not in {"terminal", "reaped"}
                    or not isinstance(entry.get("outcome"), dict)
                    or entry["outcome"].get("prefix") not in {"completed", "uploaded", "failed", "cancelled"}
                    or not isinstance(entry["outcome"].get("job"), dict)
                    or entry["outcome"]["job"].get("job_id") != entry.get("job_id")
                    for entry in entries
                )):
            fail("canonical retained-run recovery is incomplete: " + str(relative))
    elif kind in {"preserve_historical", "preserve_historical_transition",
                  "preserve_historical_run"}:
        continue
    elif kind == "block_unclassified_live":
        fail("unclassified live lifecycle decision reached finalize: " +
             str(decision.get("path")))
    else:
        fail("unknown typed lifecycle decision in receipt: " + str(kind))
receipt["status"] = "complete"
receipt["completed_at"] = time.time()
receipt["canonical_recovery_verified"] = True
atomic_json(receipt_path, receipt)
emit(receipt)
STADO_RECONCILE_EOF
"#;

const FENCE_SCHEMA: &str = "stado.storage-root-fence.v3";
const RECORD_FENCE: &str = "record-fence";
const READ_FENCE: &str = "read-fence";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileSnapshot {
    body_base64: String,
    sha256: String,
    mode: u32,
    uid: u32,
    gid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PreparedScript {
    body: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WriterFence {
    target: String,
    label: String,
    role: String,
    path: String,
    listener_port: Option<u16>,
    was_loaded: bool,
    was_runnable: bool,
    loaded_domains: Vec<String>,
    autostart: BTreeMap<String, bool>,
    prior_pid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prior_started_at: Option<String>,
    prior_loaded_environment: BTreeMap<String, String>,
    registry_declared_environment: BTreeMap<String, String>,
    unit_declared_environment: BTreeMap<String, String>,
    prior_executable: Option<String>,
    prior_sha256: Option<String>,
    prior_device: Option<u64>,
    prior_inode: Option<u64>,
    unit_snapshot: Option<FileSnapshot>,
    prior_native_state: Option<String>,
    prior_last_exit_code: Option<String>,
    prior_restart: Option<String>,
    prior_triggers: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    forward_object_recovery: Option<PreparedScript>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rollback_object_recovery: Option<PreparedScript>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_pid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_started_at: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    restored_loaded_environment: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_executable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_device: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_inode: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_route: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueueFence {
    was_paused: bool,
    drained: bool,
    resumed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LeaseAcquisition {
    subject_id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    lease: Option<crate::autonomy::storage::PlacementLease>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LifecycleFence {
    schema: String,
    transaction: String,
    status: String,
    queue: QueueFence,
    writers: Vec<WriterFence>,
    transport_retained: Vec<Value>,
    staged_runtime: Option<super::host_release::StagedRelease>,
    preflight: Value,
    #[serde(default)]
    lease_acquisitions: Vec<LeaseAcquisition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository_runner_gate: Option<Value>,
    prepared_at: i64,
    rechecked_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    activated_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    activation_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_at: Option<i64>,
}

fn bind_remote_script(phase: &str, transaction: &str, fence: &str) -> String {
    REMOTE_SCRIPT
        .replace("@PHASE@", &shlex_quote(phase))
        .replace("@TRANSACTION@", &shlex_quote(transaction))
        .replace("@FENCE@", &shlex_quote(fence))
        .replace(
            "@OWNER_TOKEN@",
            &shlex_quote(RESIDENT_OWNER_TOKEN.get().map(String::as_str).unwrap_or("")),
        )
        .replace(
            "@LOCK_FD@",
            &shlex_quote(
                &RESIDENT_LOCK_FD
                    .get()
                    .copied()
                    .unwrap_or(-1)
                    .to_string(),
            ),
        )
        .replace(
            "@TRANSITION_RETIRED_STATE@",
            &shlex_quote(crate::queue::storage::TRANSITION_RETIRED_STATE),
        )
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
    if let Some(gate) = RESIDENT_RUNNER_GATE.get() {
        return Ok(Some(gate.clone()));
    }
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        return Ok(None);
    }
    let required = |name: &str| {
        std::env::var(name).map_err(|_| {
            DeployError(format!(
                "{name} is required when storage reconciliation owns an Actions runner"
            ))
        })
    };
    let repository = required("GITHUB_REPOSITORY")?;
    let owner = repository
        .split_once('/')
        .map(|(owner, _)| owner)
        .filter(|owner| !owner.is_empty())
        .ok_or_else(|| DeployError("GITHUB_REPOSITORY is not owner/repository".to_string()))?;
    let current_runner = required("RUNNER_NAME")?;
    let run_id = required("GITHUB_RUN_ID")?;
    let source_sha = required("GITHUB_SHA")?;
    let token = super::host_precheck_runner::github_credential().await?;
    let client = reqwest::Client::new();
    let request = |endpoint: String| {
        client
            .get(endpoint)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .bearer_auth(&token)
    };

    let run_endpoint = format!("https://api.github.com/repos/{repository}/actions/runs/{run_id}");
    let run_response = request(run_endpoint)
        .send()
        .await
        .map_err(|error| DeployError(format!("cannot read current workflow run: {error}")))?;
    if !run_response.status().is_success() {
        return Err(DeployError(format!(
            "current workflow run returned HTTP {}",
            run_response.status()
        )));
    }
    let run: Value = run_response
        .json()
        .await
        .map_err(|error| DeployError(format!("invalid current workflow run: {error}")))?;
    if run.get("id").and_then(Value::as_u64).map(|id| id.to_string()) != Some(run_id.clone())
        || run.get("head_sha").and_then(Value::as_str) != Some(source_sha.as_str())
        || !matches!(
            run.get("status").and_then(Value::as_str),
            Some("in_progress" | "queued")
        )
    {
        return Err(DeployError(
            "GitHub run identity does not match this source invocation".to_string(),
        ));
    }

    let jobs_endpoint = format!(
        "https://api.github.com/repos/{repository}/actions/runs/{run_id}/jobs?filter=latest&per_page=100"
    );
    let jobs_response = request(jobs_endpoint)
        .send()
        .await
        .map_err(|error| DeployError(format!("cannot read current workflow jobs: {error}")))?;
    if !jobs_response.status().is_success() {
        return Err(DeployError(format!(
            "current workflow jobs returned HTTP {}",
            jobs_response.status()
        )));
    }
    let jobs: Value = jobs_response
        .json()
        .await
        .map_err(|error| DeployError(format!("invalid current workflow jobs: {error}")))?;
    let job_rows = jobs
        .get("jobs")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("current workflow jobs omitted jobs".to_string()))?;
    if jobs.get("total_count").and_then(Value::as_u64) != Some(job_rows.len() as u64) {
        return Err(DeployError(
            "current workflow jobs response was paginated or incomplete".to_string(),
        ));
    }
    let executing = job_rows
        .iter()
        .filter(|job| {
            job.get("runner_name").and_then(Value::as_str) == Some(current_runner.as_str())
                && job.get("status").and_then(Value::as_str) == Some("in_progress")
        })
        .collect::<Vec<_>>();
    if executing.len() != 1 {
        return Err(DeployError(format!(
            "expected one in-progress job on runner {current_runner:?}, found {}",
            executing.len()
        )));
    }
    let current_job = executing[0];
    let current_runner_id = current_job
        .get("runner_id")
        .and_then(Value::as_u64)
        .ok_or_else(|| DeployError("current workflow job omitted runner_id".to_string()))?;
    let current_job_id = current_job
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| DeployError("current workflow job omitted id".to_string()))?;

    let repositories = [
        repository.clone(),
        format!("{owner}/wisent-backend"),
        format!("{owner}/brama"),
    ];
    let mut current_online_busy = false;
    let mut other_busy = Vec::new();
    let mut inventory = Vec::new();
    for repository_name in &repositories {
        let endpoint =
            format!("https://api.github.com/repos/{repository_name}/actions/runners?per_page=100");
        let response = request(endpoint)
            .send()
            .await
            .map_err(|error| {
                DeployError(format!("cannot read runners for {repository_name}: {error}"))
            })?;
        if !response.status().is_success() {
            return Err(DeployError(format!(
                "runner inventory for {repository_name} returned HTTP {}",
                response.status()
            )));
        }
        let body: Value = response.json().await.map_err(|error| {
            DeployError(format!("invalid runner inventory for {repository_name}: {error}"))
        })?;
        let runners = body
            .get("runners")
            .and_then(Value::as_array)
            .ok_or_else(|| DeployError(format!("{repository_name} omitted runners")))?;
        if body.get("total_count").and_then(Value::as_u64) != Some(runners.len() as u64) {
            return Err(DeployError(format!(
                "runner inventory for {repository_name} was paginated or incomplete"
            )));
        }
        for runner_row in runners {
            let id = runner_row.get("id").and_then(Value::as_u64);
            let name = runner_row
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let busy = runner_row.get("busy").and_then(Value::as_bool) == Some(true);
            let online = runner_row.get("status").and_then(Value::as_str) == Some("online");
            if id == Some(current_runner_id) {
                current_online_busy |= online && busy;
            } else if busy {
                other_busy.push(json!({"repository": repository_name, "id": id, "name": name}));
            }
            inventory.push(json!({
                "repository": repository_name,
                "id": id,
                "name": name,
                "online": online,
                "busy": busy,
            }));
        }
    }
    if !current_online_busy || !other_busy.is_empty() {
        return Err(DeployError(format!(
            "fleet runner fence refused: current_online_busy={current_online_busy}, other_busy={other_busy:?}"
        )));
    }
    Ok(Some(json!({
        "repositories": repositories,
        "current_repository": repository,
        "current_run_id": run_id,
        "current_job_id": current_job_id,
        "current_job_name": current_job.get("name"),
        "current_runner": current_runner,
        "current_runner_id": current_runner_id,
        "current_online_busy": true,
        "other_busy": other_busy,
        "inventory": inventory,
        "source_sha": source_sha,
        "checked_at": Utc::now().timestamp(),
    })))
}

#[derive(Debug, Clone)]
struct ServiceCandidate {
    target: crate::targets::ComputeTarget,
    declared: service::ManagedService,
    loaded_domains: Vec<String>,
    observed_command: String,
}

fn command_tokens(command: &str) -> Vec<&str> {
    command
        .split_ascii_whitespace()
        .map(|token| token.trim_matches(|ch| matches!(ch, '{' | '}' | '[' | ']' | ';' | ',' | '"')))
        .filter(|token| !token.is_empty())
        .collect()
}

fn executable_name(token: &str) -> &str {
    token.rsplit('/').next().unwrap_or(token)
}

fn service_role(label: &str, command: &str) -> &'static str {
    const OBJECT_API_LABEL: &str = "com.wisent.always-on.stado-object-api";
    if label == OBJECT_API_LABEL {
        return "object-api";
    }
    let tokens = command_tokens(command);
    if tokens
        .iter()
        .any(|token| executable_name(token) == "Runner.Listener")
    {
        return "runner";
    }
    let executable = tokens
        .iter()
        .position(|token| executable_name(token) == "stado");
    if let Some(index) = executable {
        return match tokens.get(index + 1).copied() {
            Some("resolver") => "transport",
            Some("release-agent") => "release-agent",
            Some("coordinator" | "local-control-plane" | "cloud-control-plane") => "coordinator",
            Some("agent") => "agent",
            Some("disk-cleanup") => "disk-cleanup",
            _ => "writer",
        };
    }
    match tokens
        .first()
        .map(|token| executable_name(token))
        .unwrap_or_default()
    {
        "caddy" | "tailscaled" | "skarbiec" | "skarbiec-control-plane" | "ssh" => "transport",
        "stado-fix" => "agent",
        _ => "writer",
    }
}

fn managed_from_unit(
    target: &crate::targets::ComputeTarget,
    label: &str,
    path: &str,
    kind: &str,
) -> service::ManagedService {
    if kind == service::KIND_SYSTEMD {
        service::systemd_service(
            &target.name,
            label,
            path,
            service::SOURCE_PRODUCT,
            "storage-root-reconcile",
        )
    } else {
        service::launchd_service(
            &target.name,
            label,
            path,
            service::SOURCE_PRODUCT,
            "storage-root-reconcile",
        )
    }
}

fn exact_identity_component(value: &str, identity: &str) -> bool {
    value
        .split(|character: char| matches!(character, '.' | '/' | '\\'))
        .any(|component| component == identity)
}

fn current_runner_candidate(
    candidate: &ServiceCandidate,
    command: &str,
    current_runner: &str,
) -> bool {
    exact_identity_component(candidate.declared.unit_id(), current_runner)
        || exact_identity_component(&candidate.declared.path, current_runner)
        || command_tokens(command)
            .iter()
            .any(|token| *token == current_runner)
}

async fn registry_services(
    storage_target: &crate::targets::ComputeTarget,
    runner: &Runner,
) -> Result<Vec<ServiceCandidate>, DeployError> {
    let mut candidates = BTreeMap::<String, ServiceCandidate>::new();
    for declared in service::declared_services(storage_target) {
        candidates.insert(
            declared.unit_id().to_string(),
            ServiceCandidate {
                target: storage_target.clone(),
                observed_command: std::iter::once(declared.program.as_str())
                    .chain(declared.args.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" "),
                declared,
                loaded_domains: Vec::new(),
            },
        );
    }
    for product in super::products::declared()? {
        if !storage_target.managed_versions.contains_key(&product.name) {
            continue;
        }
        for unit in &product.units {
            let label = unit.label_for(&storage_target.name);
            let Some(path) = unit.path_for(&storage_target.name) else {
                continue;
            };
            let kind = unit.kind.as_deref().unwrap_or(service::KIND_LAUNCHD);
            candidates.entry(label.clone()).or_insert_with(|| ServiceCandidate {
                target: storage_target.clone(),
                declared: managed_from_unit(storage_target, &label, &path, kind),
                loaded_domains: Vec::new(),
                observed_command: String::new(),
            });
        }
    }
    for native in service::loaded_units(storage_target, runner).await? {
        let label = native.label.clone();
        let candidate = candidates.entry(label.clone()).or_insert_with(|| {
            let kind = if native.path.ends_with(".service") {
                service::KIND_SYSTEMD
            } else {
                service::KIND_LAUNCHD
            };
            ServiceCandidate {
                target: storage_target.clone(),
                declared: managed_from_unit(storage_target, &label, &native.path, kind),
                loaded_domains: Vec::new(),
                observed_command: String::new(),
            }
        });
        if candidate.declared.path.is_empty() && !native.path.is_empty() {
            candidate.declared.path.clone_from(&native.path);
        }
        candidate.loaded_domains = native.loaded_domains;
        if !native.running_program.is_empty() {
            candidate.observed_command = native.running_program;
        } else if candidate.observed_command.is_empty() {
            candidate.observed_command = native.program;
        }
    }
    Ok(candidates.into_values().collect())
}

fn command_u16_option(command: &str, option: &str) -> Option<u16> {
    let tokens = command_tokens(command);
    tokens
        .windows(2)
        .find(|pair| pair[0] == option)
        .and_then(|pair| pair[1].parse().ok())
}

fn stop_priority(role: &str) -> u8 {
    match role {
        "runner" => 0,
        "current-runner" => 1,
        "release-agent" => 2,
        "coordinator" => 3,
        "agent" | "disk-cleanup" => 4,
        "object-api" => u8::MAX,
        _ => 5,
    }
}

async fn renew_fence_leases(
    store: &crate::queue::JobStorage,
    fence: &mut LifecycleFence,
) -> Result<(), DeployError> {
    const LEASE_TTL_SECONDS: u64 = 12 * 60 * 60;
    for acquisition in &mut fence.lease_acquisitions {
        let lease = acquisition.lease.as_mut().ok_or_else(|| {
            DeployError(format!(
                "placement lease acquisition for {} has no durable result",
                acquisition.subject_id
            ))
        })?;
        *lease = crate::autonomy::storage::renew_placement_lease(
            store,
            &lease.subject_id,
            &lease.token,
            LEASE_TTL_SECONDS,
            Utc::now(),
        )
        .await
        .map_err(|error| DeployError(format!("cannot renew {}: {error}", lease.subject_id)))?
        .ok_or_else(|| {
            DeployError(format!(
                "placement lease ownership changed for {}",
                lease.subject_id
            ))
        })?;
        acquisition.status = "acquired".to_string();
    }
    Ok(())
}

async fn prove_listener_closed(
    target: &crate::targets::ComputeTarget,
    port: u16,
    runner: &Runner,
) -> Result<(), DeployError> {
    let script = format!(
        "PORT={} /usr/bin/python3 - <<'PY'\n\
import os, socket, time\n\
port = int(os.environ['PORT'])\n\
deadline = time.monotonic() + 30\n\
while True:\n\
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)\n\
    probe.settimeout(0.2)\n\
    result = probe.connect_ex(('127.0.0.1', port))\n\
    probe.close()\n\
    if result != 0:\n\
        print('STADO_LISTENER_CLOSED\\t' + str(port))\n\
        break\n\
    if time.monotonic() >= deadline:\n\
        raise SystemExit('object API listener remained open')\n\
    time.sleep(0.2)\n\
PY",
        port
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    let marker = format!("STADO_LISTENER_CLOSED\t{port}");
    if !output.ok() || !output.stdout.lines().any(|line| line == marker) {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "object API listener did not close",
        )));
    }
    Ok(())
}

fn qualified_copy_required(preflight: &Value) -> Result<bool, DeployError> {
    let backup = preflight
        .get("backup_qualified")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("preflight omitted the backup qualified inventory".to_string()))?;
    let primary = preflight
        .get("primary_qualified")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("preflight omitted the primary qualified inventory".to_string()))?;
    let primary_by_path = primary
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str).map(|path| (path, item)))
        .collect::<BTreeMap<_, _>>();
    Ok(backup.iter().any(|item| {
        item.get("path")
            .and_then(Value::as_str)
            .and_then(|path| primary_by_path.get(path).copied())
            != Some(item)
    }))
}
fn physical_file_identity<'a>(
    preflight: &'a Value,
    inventory: &str,
    path: &str,
) -> Option<&'a Value> {
    preflight
        .get(inventory)?
        .get("files")?
        .as_array()?
        .iter()
        .find(|item| item.get("path").and_then(Value::as_str) == Some(path))
        .and_then(|item| item.get("body"))
}

async fn snapshot_unit_file(
    target: &crate::targets::ComputeTarget,
    path: &str,
    runner: &Runner,
) -> Result<Option<FileSnapshot>, DeployError> {
    let script = format!(
        "STADO_UNIT_PATH={} /usr/bin/python3 - <<'PY'\n\
import base64, hashlib, json, os, stat\n\
path = os.path.expanduser(os.path.expandvars(os.environ['STADO_UNIT_PATH']))\n\
try:\n\
    info = os.lstat(path)\n\
except FileNotFoundError:\n\
    print('STADO_UNIT_SNAPSHOT\\tabsent')\n\
    raise SystemExit(0)\n\
if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):\n\
    raise SystemExit('unit path is not a regular non-symlink file')\n\
with open(path, 'rb') as handle:\n\
    body = handle.read()\n\
print('STADO_UNIT_SNAPSHOT\\t' + json.dumps({{\n\
    'body_base64': base64.b64encode(body).decode('ascii'),\n\
    'sha256': hashlib.sha256(body).hexdigest(),\n\
    'mode': stat.S_IMODE(info.st_mode),\n\
    'uid': info.st_uid,\n\
    'gid': info.st_gid,\n\
}}, sort_keys=True, separators=(',', ':')))\n\
PY",
        shlex_quote(path)
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "unit file could not be snapshotted",
        )));
    }
    let value = output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_UNIT_SNAPSHOT\t"))
        .ok_or_else(|| DeployError("unit snapshot returned no marker".to_string()))?;
    if value == "absent" {
        return Ok(None);
    }
    serde_json::from_str(value)
        .map(Some)
        .map_err(|error| DeployError(format!("unit snapshot is invalid: {error}")))
}
fn unit_declared_environment(
    candidate: &ServiceCandidate,
    snapshot: Option<&FileSnapshot>,
) -> Result<BTreeMap<String, String>, DeployError> {
    let Some(snapshot) = snapshot else {
        return Ok(BTreeMap::new());
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&snapshot.body_base64)
        .map_err(|error| DeployError(format!("unit snapshot base64 is invalid: {error}")))?;
    let content = String::from_utf8(bytes)
        .map_err(|error| DeployError(format!("unit snapshot is not UTF-8: {error}")))?;
    let kind = if candidate.declared.path.ends_with(".service") {
        service::KIND_SYSTEMD
    } else {
        service::KIND_LAUNCHD
    };
    let unit = service::UnitFile {
        host: candidate.target.name.clone(),
        unit: candidate.declared.unit_id().to_string(),
        path: candidate.declared.path.clone(),
        kind,
        content,
    };
    let parsed = service::unit_environment(&unit)?;
    Ok(parsed.env.into_iter().collect())
}

fn prepared_script(body: String) -> PreparedScript {
    PreparedScript {
        sha256: hex::encode(Sha256::digest(body.as_bytes())),
        body,
    }
}
async fn correlate_served_store(
    target: &crate::targets::ComputeTarget,
    port: u16,
    preflight: &Value,
    primary_after_commit: bool,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let payload = serde_json::to_vec(&json!({
        "primary": preflight.get("primary_qualified"),
        "backup": preflight.get("backup_qualified"),
        "primary_physical": preflight.get("primary_physical"),
        "backup_physical": preflight.get("backup_physical"),
        "primary_after_commit": primary_after_commit,
    }))
    .map_err(|error| DeployError(format!("cannot encode served-store inventory: {error}")))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(payload);
    let script = format!(
        "STADO_CORRELATION={} STADO_OBJECT_PORT={} /usr/bin/python3 - <<'PY'\n\
import base64, hashlib, json, os, urllib.parse, urllib.request\n\
payload = json.loads(base64.b64decode(os.environ['STADO_CORRELATION']))\n\
port = int(os.environ['STADO_OBJECT_PORT'])\n\
token_path = os.path.expanduser('~/.stado/queue-object-api-token')\n\
with open(token_path, encoding='utf-8') as handle:\n\
    token = handle.read().strip()\n\
if not token:\n\
    raise SystemExit('object API correlation token is empty')\n\
headers = {{'Authorization': 'Bearer ' + token}}\n\
base = 'http://127.0.0.1:' + str(port)\n\
request = urllib.request.Request(base + '/api/object/list?namespace=probierz&prefix=', headers=headers)\n\
with urllib.request.urlopen(request, timeout=30) as response:\n\
    listed = json.load(response)\n\
keys = sorted(item.get('key') for item in listed.get('objects', []) if isinstance(item.get('key'), str))\n\
def identities(name):\n\
    result = {{}}\n\
    prefix = 'ecosystem/probierz/'\n\
    for item in payload[name]:\n\
        path = item.get('path', '')\n\
        if not path.startswith(prefix):\n\
            continue\n\
        result[path[len(prefix):]] = item.get('body')\n\
    return result\n\
primary_before = identities('primary')\n\
backup = identities('backup')\n\
primary = dict(primary_before)\n\
if payload.get('primary_after_commit'):\n\
    primary.update(backup)\n\
served = {{}}\n\
for key in keys:\n\
    uri = 'stado://probierz/' + key\n\
    url = base + '/api/object?uri=' + urllib.parse.quote(uri, safe='')\n\
    request = urllib.request.Request(url, headers=headers)\n\
    digest = hashlib.sha256()\n\
    size = 0\n\
    with urllib.request.urlopen(request, timeout=60) as response:\n\
        while True:\n\
            chunk = response.read(1024 * 1024)\n\
            if not chunk:\n\
                break\n\
            digest.update(chunk)\n\
            size += len(chunk)\n\
    served[key] = {{'sha256': digest.hexdigest(), 'size': size}}\n\
matches_primary = keys == sorted(primary) and all(served[key] == primary[key] for key in keys)\n\
matches_backup = keys == sorted(backup) and all(served[key] == backup[key] for key in keys)\n\
if not matches_primary and not matches_backup:\n\
    raise SystemExit('object API does not serve either complete physical qualified root')\n\
authority = 'identical' if matches_primary and matches_backup else 'A' if matches_primary else 'B'\n\
def physical_identity(name, path):\n\
    for item in payload[name].get('files', []):\n\
        if item.get('path') == path:\n\
            return item.get('body')\n\
    return None\n\
object_mappings = [{{\n\
    'backend': 'stado-object-api', 'namespace': 'probierz', 'key': key,\n\
    'physical_path': 'ecosystem/probierz/' + key, 'identity': served[key],\n\
}} for key in keys]\n\
registry_mappings = [\n\
    {{'root': 'A', 'backend': 'local', 'namespace': None, 'key': 'registry.json',\n\
      'physical_path': 'registry.json',\n\
      'identity': physical_identity('primary_physical', 'registry.json')}},\n\
    {{'root': 'B', 'backend': 'local', 'namespace': None, 'key': 'registry.json',\n\
      'physical_path': 'registry.json',\n\
      'identity': physical_identity('backup_physical', 'registry.json')}},\n\
    {{'root': 'served', 'backend': 'stado-object', 'namespace': None,\n\
      'key': 'registry.json', 'physical_path': None,\n\
      'observation': 'client namespace was not observable from the object API'}},\n\
]\n\
print('STADO_SERVED_STORE\\t' + json.dumps({{\n\
    'object_authority': authority,\n\
    'endpoint': base,\n\
    'object_store': {{'backend': 'stado-object-api', 'namespace': 'probierz',\n\
                     'objects': object_mappings}},\n\
    'registry_store': {{'mappings': registry_mappings}},\n\
    'primary_root': os.path.expanduser('~/.stado/local-storage'),\n\
    'backup_root': os.path.expanduser('~/.stado/local-backup'),\n\
}}, sort_keys=True, separators=(',', ':')))\n\
PY",
        shlex_quote(&encoded),
        port
    );
    let output = host_channel::run_script_with_timeout(target, &script, TIMEOUT, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "object API physical-store correlation failed",
        )));
    }
    output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_SERVED_STORE\t"))
        .ok_or_else(|| DeployError("object API correlation returned no evidence".to_string()))
        .and_then(|body| {
            serde_json::from_str(body)
                .map_err(|error| DeployError(format!("object API correlation is invalid: {error}")))
        })
}
async fn prepare_lifecycle_fence(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<LifecycleFence, DeployError> {
    let mut fence = match read_fence(storage_target, transaction, runner).await? {
        Some(existing) => existing,
        None => {
            let services = registry_services(storage_target, runner).await?;
            let mut preflight = remote_phase(storage_target, transaction, PREFLIGHT, runner).await?;
            let repository_runner_gate = repository_runner_gate().await?;
            let current_runner = repository_runner_gate
                .as_ref()
                .and_then(|gate| gate.get("current_runner"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let store = crate::queue::JobStorage::new()
                .await
                .map_err(|error| DeployError(format!("cannot read queue before fencing: {error}")))?;
            let prior = crate::queue::control::read(&store)
                .await
                .map_err(|error| DeployError(format!("cannot read prior queue state: {error}")))?;
            let mut writers = Vec::new();
            let mut transport_retained = Vec::new();
            let mut api_already_forward = false;
            let mut owning_runner_found = false;
            let mut object_port = None;
            for candidate in &services {
                let state = super::service_label_print::print_label(
                    &candidate.target,
                    candidate.declared.unit_id(),
                    service::BootoutScope::Any,
                    runner,
                )
                .await?;
                let command = state.runs().unwrap_or(&candidate.observed_command);
                let mut role = service_role(candidate.declared.unit_id(), command).to_string();
                if role == "runner"
                    && current_runner.as_deref().is_some_and(|current| {
                        current_runner_candidate(candidate, command, current)
                    })
                {
                    role = "current-runner".to_string();
                    owning_runner_found = true;
                }
                let autostart =
                    service::label_autostart(&candidate.target, candidate.declared.unit_id(), runner)
                        .await?;
                if role == "object-api" {
                    let backup_backend =
                        state.loaded_environment.get("WC_BACKUP_STORAGE_BACKEND");
                    let backup_path =
                        state.loaded_environment.get("WC_BACKUP_LOCAL_STORAGE_PATH");
                    if state.pid.is_none()
                        || state.process_started_at.is_none()
                        || state.process_executable.is_none()
                        || state.process_device.is_none()
                        || state.process_inode.is_none()
                        || state.process_sha256.is_none()
                    {
                        return Err(DeployError(format!(
                            "{} cannot be fenced without a mapped-inode image identity",
                            candidate.declared.unit_id()
                        )));
                    }
                    let loaded_routing_observed = state
                        .loaded_environment
                        .get("WC_STORAGE_BACKEND")
                        .map(String::as_str)
                        == Some("local")
                        && state
                            .loaded_environment
                            .get("WC_LOCAL_STORAGE_PATH")
                            .is_some_and(|path| !path.is_empty())
                        && state
                            .loaded_environment
                            .get("STADO_CONFIG")
                            .is_some_and(|path| !path.is_empty())
                        && backup_backend.is_some() == backup_path.is_some();
                    if state.loaded_environment.contains_key("WC_STORAGE_BACKEND")
                        && !loaded_routing_observed
                    {
                        return Err(DeployError(format!(
                            "{} reported an incomplete loaded storage route",
                            candidate.declared.unit_id()
                        )));
                    }
                    api_already_forward = loaded_routing_observed
                        && state
                            .loaded_environment
                            .get("WC_LOCAL_STORAGE_PATH")
                            .is_some_and(|path| path.ends_with("/.stado/local-storage"))
                        && backup_backend.map(String::as_str) == Some("local")
                        && backup_path
                            .map(String::as_str)
                            .is_some_and(|path| path.ends_with("/.stado/local-backup"));
                    object_port = command_u16_option(command, "--port");
                }
                if matches!(role.as_str(), "transport" | "current-runner") {
                    if role == "current-runner" && state.pid.is_none() {
                        return Err(DeployError(
                            "Actions runner gate did not map its owning live native process"
                                .to_string(),
                        ));
                    }
                    if state.pid.is_some()
                        && (state.process_started_at.is_none()
                            || state.process_device.is_none()
                            || state.process_inode.is_none()
                            || state.process_sha256.is_none())
                    {
                        return Err(DeployError(format!(
                            "retained transport {} has no mapped-inode image identity",
                            candidate.declared.unit_id()
                        )));
                    }
                    if state.loaded() || state.pid.is_some() {
                        transport_retained.push(json!({
                            "host": candidate.target.name.clone(),
                            "label": candidate.declared.unit_id(),
                            "loaded_domains": candidate.loaded_domains.clone(),
                            "autostart": autostart,
                            "state": state.to_json(),
                        }));
                    }
                    continue;
                }
                let was_loaded = state.loaded() || !candidate.loaded_domains.is_empty();
                let was_runnable = state.pid.is_some();
                if was_runnable
                    && (state.process_started_at.is_none()
                        || state.process_executable.is_none()
                        || state.process_device.is_none()
                        || state.process_inode.is_none()
                        || state.process_sha256.is_none())
                {
                    return Err(DeployError(format!(
                        "{} cannot be fenced without a mapped-inode process identity",
                        candidate.declared.unit_id()

                    )));
                }
                if (was_loaded || was_runnable) && candidate.declared.path.is_empty() {
                    return Err(DeployError(format!(
                        "{} has no unit path from which its exact prior lifecycle can be restored",
                        candidate.declared.unit_id()
                    )));
                }
                let listener_port = (role == "object-api")
                    .then_some(object_port)
                    .flatten();
                if role == "object-api" && listener_port.is_none() {
                    return Err(DeployError(
                        "object API listener port is absent from its loaded argv".to_string(),
                    ));
                }
                let pending =
                    was_loaded || was_runnable || autostart.values().copied().any(|enabled| enabled);
                let unit_snapshot =
                    snapshot_unit_file(&candidate.target, &candidate.declared.path, runner).await?;
                if pending && unit_snapshot.is_none() {
                    return Err(DeployError(format!(
                        "{} has no exact unit bytes for restoration",
                        candidate.declared.unit_id()
                    )));
                }
                let unit_declared_environment =
                    unit_declared_environment(candidate, unit_snapshot.as_ref())?;
                writers.push(WriterFence {
                    target: candidate.target.name.clone(),
                    label: candidate.declared.unit_id().to_string(),
                    role,
                    path: candidate.declared.path.clone(),
                    listener_port,
                    was_loaded,
                    was_runnable,
                    loaded_domains: candidate.loaded_domains.clone(),
                    autostart,
                    prior_pid: state.pid,
                    prior_started_at: state.process_started_at,
                    prior_loaded_environment: state.loaded_environment,
                    registry_declared_environment: candidate.declared.env.clone(),
                    unit_declared_environment,
                    prior_executable: state.process_executable,
                    prior_sha256: state.process_sha256,
                    prior_device: state.process_device,
                    prior_inode: state.process_inode,
                    unit_snapshot,
                    prior_native_state: state.state,
                    prior_last_exit_code: state.last_exit_code,
                    prior_restart: state.restart,
                    prior_triggers: state.triggers,
                    forward_object_recovery: None,
                    rollback_object_recovery: None,
                    status: if pending { "pending" } else { "stopped" }.to_string(),
                    restored_pid: None,
                    restored_started_at: None,
                    restored_loaded_environment: BTreeMap::new(),
                    restored_executable: None,
                    restored_sha256: None,
                    restored_device: None,
                    restored_inode: None,
                    restored_route: None,
                });
            }
            writers.sort_by_key(|writer| stop_priority(&writer.role));
            if !writers.iter().any(|writer| writer.role == "object-api") {
                return Err(DeployError(
                    "fleet service inventory did not resolve the canonical object API".to_string(),
                ));
            }
            if repository_runner_gate.is_some() && !owning_runner_found {
                return Err(DeployError(
                    "runner gate did not map its owning native runner service".to_string(),
                ));
            }
            let correlation = correlate_served_store(
                storage_target,
                object_port.ok_or_else(|| {
                    DeployError("object API listener port is absent from its loaded argv".to_string())
                })?,
                &preflight,
                false,
                runner,
            )
            .await?;
            preflight
                .as_object_mut()
                .ok_or_else(|| DeployError("preflight report is not an object".to_string()))?
                .insert("served_store".to_string(), correlation.clone());
            let staged_runtime = super::host_release::stage_declared_release(
                &storage_target.name,
                "stado",
                storage_target.managed_versions.get("stado").ok_or_else(|| {
                    DeployError("target has no current declared Stado runtime".to_string())
                })?,
                runner,
            )
            .await?;
            let primary_root = correlation
                .get("primary_root")
                .and_then(Value::as_str)
                .ok_or_else(|| DeployError("served-store evidence omitted A root".to_string()))?;
            let backup_root = correlation
                .get("backup_root")
                .and_then(Value::as_str)
                .ok_or_else(|| DeployError("served-store evidence omitted B root".to_string()))?;
            let object_writer = writers
                .iter_mut()
                .find(|writer| writer.role == "object-api")
                .expect("canonical object API writer was required above");
            object_writer.forward_object_recovery = Some(object_recovery_script(
                object_writer,
                primary_root,
                Some(backup_root),
            )?);
            object_writer.rollback_object_recovery =
                Some(object_recovery_script(object_writer, backup_root, None)?);
            let raw_registry_identity =
                physical_file_identity(&preflight, "primary_physical", "registry.json").cloned();
            let configuration_evidence = json!({
                "object_api": {
                    "loaded_environment_status": if object_writer
                        .prior_loaded_environment
                        .contains_key("WC_STORAGE_BACKEND")
                    {
                        "observed"
                    } else {
                        "unavailable"
                    },
                    "observed_loaded_environment": object_writer.prior_loaded_environment.clone(),
                    "unit_declaration": object_writer.unit_declared_environment.clone(),
                    "registry_declaration":
                        object_writer.registry_declared_environment.clone(),
                },
                "dashboard_registry_store": {
                    "backend": if api_already_forward { Value::from("local") } else { Value::Null },
                    "namespace": Value::Null,
                    "key": "registry.json",
                    "physical_root": if api_already_forward { Value::from("A") } else { Value::Null },
                    "identity": raw_registry_identity,
                },
                "remote_registry_store_mapping": {
                    "backend": "stado-object",
                    "namespace": Value::Null,
                    "key": "registry.json",
                    "physical_path": Value::Null,
                    "observation": "client namespace was not observed",
                },
            });
            preflight
                .as_object_mut()
                .expect("preflight report was validated as an object")
                .insert("effective_configuration".to_string(), configuration_evidence);
            let object_runtime_matches = writers
                .iter()
                .find(|writer| writer.role == "object-api")
                .and_then(|writer| writer.prior_sha256.as_deref())
                == Some(staged_runtime.staged_sha256.as_str());
            let already_reconciled = !qualified_copy_required(&preflight)?
                && api_already_forward
                && physical_file_identity(&preflight, "primary_physical", "registry.json").is_some()
                && correlation.get("object_authority").and_then(Value::as_str) == Some("A")
                && object_runtime_matches;
            let staged_runtime = Some(staged_runtime);
            let initial = LifecycleFence {
                schema: FENCE_SCHEMA.to_string(),
                transaction: transaction.to_string(),
                status: if already_reconciled {
                    "already_reconciled"
                } else {
                    "preparing"
                }
                .to_string(),
                queue: QueueFence {
                    was_paused: prior.paused,
                    drained: false,
                    resumed: false,
                },
                writers,
                transport_retained,
                staged_runtime,
                preflight,
                lease_acquisitions: Vec::new(),
                repository_runner_gate,
                prepared_at: Utc::now().timestamp(),
                rechecked_at: 0,
                activated_at: None,
                activation_sha256: None,
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
    if fence.status == "already_reconciled" {
        return Ok(fence);
    }
    if fence.status == "fenced" {
        return recheck_lifecycle_fence(storage_target, transaction, runner).await;
    }
    if fence.status != "preparing" {
        return Err(DeployError(format!(
            "lifecycle fence cannot prepare from {}",
            fence.status
        )));
    }

    let store = crate::queue::JobStorage::new()
        .await
        .map_err(|error| DeployError(format!("cannot open queue for fencing: {error}")))?;
    const LEASE_TTL_SECONDS: u64 = 12 * 60 * 60;
    let subjects = fence
        .writers
        .iter()
        .map(|writer| format!("service:{}:{}", writer.target, writer.label))
        .collect::<Vec<_>>();
    for subject in subjects {
        let index = match fence
            .lease_acquisitions
            .iter()
            .position(|entry| entry.subject_id == subject)
        {
            Some(index) => index,
            None => {
                fence.lease_acquisitions.push(LeaseAcquisition {
                    subject_id: subject.clone(),
                    status: "acquire_intent".to_string(),
                    lease: None,
                });
                write_fence(storage_target, transaction, &fence, runner).await?;
                fence.lease_acquisitions.len() - 1
            }
        };
        if fence.lease_acquisitions[index].lease.is_none() {
            let lease = crate::autonomy::storage::acquire_placement_lease(
                &store,
                &subject,
                transaction,
                "stado storage-root-reconcile",
                LEASE_TTL_SECONDS,
                Utc::now(),
            )
            .await
            .map_err(|error| DeployError(format!("cannot acquire {subject}: {error}")))?
            .ok_or_else(|| DeployError(format!("active placement lease blocks {subject}")))?;
            fence.lease_acquisitions[index].lease = Some(lease);
            fence.lease_acquisitions[index].status = "acquired".to_string();
            write_fence(storage_target, transaction, &fence, runner).await?;
        }
    }
    renew_fence_leases(&store, &mut fence).await?;
    write_fence(storage_target, transaction, &fence, runner).await?;

    if !fence.queue.drained {
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
        let writer = &fence.writers[index];
        let current = super::service_label_print::print_label(
            storage_target,
            &writer.label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        let current_autostart =
            service::label_autostart(storage_target, &writer.label, runner).await?;
        let process_matches_prior = current.loaded() == writer.was_loaded
            && current.pid.as_deref() == writer.prior_pid.as_deref()
            && current.process_started_at.as_deref() == writer.prior_started_at.as_deref()
            && current.process_executable.as_deref() == writer.prior_executable.as_deref()
            && current.process_device == writer.prior_device
            && current.process_inode == writer.prior_inode
            && current.process_sha256.as_deref() == writer.prior_sha256.as_deref()
            && current.loaded_environment == writer.prior_loaded_environment;
        match writer.status.as_str() {
            "pending" if process_matches_prior && current_autostart == writer.autostart => {}
            "stop_intent" if !current.loaded() && current.pid.is_none() => {
                if writer
                    .autostart
                    .iter()
                    .any(|(scope, enabled)| *enabled && current_autostart.get(scope) != Some(&false))
                {
                    return Err(DeployError(format!(
                        "{} stopped after an interrupted fence but remained enabled",
                        writer.label
                    )));
                }
                fence.writers[index].status = "stopped".to_string();
                write_fence(storage_target, transaction, &fence, runner).await?;
            }
            "stop_intent"
                if process_matches_prior
                    && writer.autostart.iter().all(|(scope, prior)| {
                        current_autostart
                            .get(scope)
                            .is_some_and(|current| current == prior || (*prior && !*current))
                    }) => {}
            "stopped" if !current.loaded() && current.pid.is_none() => {}
            state => {
                return Err(DeployError(format!(
                    "{} native state does not match resumable fence state {state:?}",
                    writer.label
                )));
            }
        }
    }

    for index in 0..fence.writers.len() {
        if fence.writers[index].status == "stopped" {
            continue;
        }
        if fence.writers[index].status == "pending" {
            fence.writers[index].status = "stop_intent".to_string();
            write_fence(storage_target, transaction, &fence, runner).await?;
        }
        renew_fence_leases(&store, &mut fence).await?;
        write_fence(storage_target, transaction, &fence, runner).await?;
        let label = fence.writers[index].label.clone();
        for (scope, enabled) in fence.writers[index].autostart.clone() {
            if enabled {
                service::set_label_autostart(storage_target, &label, &scope, false, runner).await?;
            }
        }
        let disabled = service::label_autostart(storage_target, &label, runner).await?;
        if fence.writers[index]
            .autostart
            .iter()
            .any(|(scope, enabled)| *enabled && disabled.get(scope) != Some(&false))
        {
            return Err(DeployError(format!(
                "{label} remained enabled after persistent lifecycle disable"
            )));
        }
        if fence.writers[index].was_loaded || fence.writers[index].was_runnable {
            let (state, detail) =
                service::bootout_label(storage_target, &label, service::BootoutScope::Any, runner)
                    .await?;
            if !matches!(state.as_str(), "booted_out" | "absent") {
                return Err(DeployError(format!(
                    "{label} did not boot out: {detail}"
                )));
            }
        }
        let state = super::service_label_print::print_label(
            storage_target,
            &label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        if state.loaded() || state.pid.is_some() {
            return Err(DeployError(format!(
                "{label} remained loaded after writer fencing"
            )));
        }
        if let Some(port) = fence.writers[index].listener_port {
            prove_listener_closed(storage_target, port, runner).await?;
        }
        fence.writers[index].status = "stopped".to_string();
        write_fence(storage_target, transaction, &fence, runner).await?;
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
    if fence.status != "fenced"
        || !fence.queue.drained
        || fence.writers.iter().any(|writer| writer.status != "stopped")
    {
        return Err(DeployError(
            "durable lifecycle fence is not in the fenced/drained state".to_string(),
        ));
    }
    for writer in &fence.writers {
        let state = super::service_label_print::print_label(
            storage_target,
            &writer.label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        if state.loaded() || state.pid.is_some() {
            return Err(DeployError(format!(
                "writer {} on {} resumed during the storage fence",
                writer.label, writer.target
            )));
        }
        let autostart = service::label_autostart(storage_target, &writer.label, runner).await?;
        if writer
            .autostart
            .iter()
            .any(|(scope, enabled)| *enabled && autostart.get(scope) != Some(&false))
        {
            return Err(DeployError(format!(
                "writer {} became enabled during the storage fence",
                writer.label
            )));
        }
        if let Some(port) = writer.listener_port {
            prove_listener_closed(storage_target, port, runner).await?;
        }
    }
    for retained in &fence.transport_retained {
        let label = retained
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let state = super::service_label_print::print_label(
            storage_target,
            label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        if !state.loaded()
            || state.pid.is_none()
            || state.process_started_at.is_none()
            || state.process_device.is_none()
            || state.process_inode.is_none()
            || state.process_sha256.is_none()
        {
            return Err(DeployError(format!(
                "retained transport {label} is no longer a runnable mapped image"
            )));
        }
        let prior = retained
            .get("state")
            .and_then(Value::as_object)
            .ok_or_else(|| DeployError(format!("retained transport {label} has no prior state")))?;
        let current = state.to_json();
        for field in [
            "pid",
            "process_started_at",
            "process_executable",
            "process_device",
            "process_inode",
            "process_sha256",
        ] {
            if current.get(field) != prior.get(field) {
                return Err(DeployError(format!(
                    "retained transport {label} changed mapped identity field {field}"
                )));
            }
        }
        let autostart = service::label_autostart(storage_target, label, runner).await?;
        if retained.get("autostart") != Some(&json!(autostart)) {
            return Err(DeployError(format!(
                "retained transport {label} changed native autostart state"
            )));
        }
    }
    fence.rechecked_at = Utc::now().timestamp();
    write_fence(storage_target, transaction, &fence, runner).await?;
    Ok(fence)
}

fn restore_priority(role: &str) -> u8 {
    match role {
        "object-api" => 0,
        "coordinator" => 1,
        "agent" | "disk-cleanup" => 2,
        "release-agent" => 3,
        "runner" => 4,
        "current-runner" => 5,
        _ => 2,
    }
}

fn managed_writer(
    target: &crate::targets::ComputeTarget,
    writer: &WriterFence,
) -> service::ManagedService {
    let kind = if writer.path.ends_with(".service") {
        service::KIND_SYSTEMD
    } else {
        service::KIND_LAUNCHD
    };
    managed_from_unit(target, &writer.label, &writer.path, kind)
}

fn object_recovery_script(
    writer: &WriterFence,
    primary: &str,
    backup: Option<&str>,
) -> Result<PreparedScript, DeployError> {
    if primary.is_empty() || backup == Some("") {
        return Err(DeployError(
            "prepared object recovery contains an empty physical root".to_string(),
        ));
    }
    let config = writer
        .prior_loaded_environment
        .get("STADO_CONFIG")
        .or_else(|| writer.unit_declared_environment.get("STADO_CONFIG"))
        .or_else(|| writer.registry_declared_environment.get("STADO_CONFIG"))
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            DeployError(
                "object API has neither observed nor declared STADO_CONFIG for recovery".to_string(),
            )
        })?;
    let port = writer
        .listener_port
        .ok_or_else(|| DeployError("captured object API port is absent".to_string()))?;
    let body = ROLLBACK_OBJECT_API_SCRIPT
        .replace("@PRIMARY@", &shlex_quote(primary))
        .replace(
            "@BACKUP_BACKEND@",
            &shlex_quote(if backup.is_some() { "local" } else { "" }),
        )
        .replace("@BACKUP@", &shlex_quote(backup.unwrap_or("")))
        .replace("@CONFIG@", &shlex_quote(config))
        .replace("@PORT@", &port.to_string());
    Ok(prepared_script(body))
}

fn validate_prepared_fence(fence: &LifecycleFence) -> Result<(), DeployError> {
    for writer in &fence.writers {
        if let Some(snapshot) = &writer.unit_snapshot {
            let body = base64::engine::general_purpose::STANDARD
                .decode(&snapshot.body_base64)
                .map_err(|error| DeployError(format!("{} unit snapshot is invalid: {error}", writer.label)))?;
            if hex::encode(Sha256::digest(&body)) != snapshot.sha256 {
                return Err(DeployError(format!(
                    "{} unit snapshot digest does not match its exact bytes",
                    writer.label
                )));
            }
        }
        for script in [
            writer.forward_object_recovery.as_ref(),
            writer.rollback_object_recovery.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if hex::encode(Sha256::digest(script.body.as_bytes())) != script.sha256 {
                return Err(DeployError(format!(
                    "{} prepared recovery script digest changed",
                    writer.label
                )));
            }
        }
        if writer.role == "object-api"
            && (writer.forward_object_recovery.is_none()
                || writer.rollback_object_recovery.is_none())
        {
            return Err(DeployError(
                "object API has no immutable forward and B-only rollback configurations"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

async fn restore_unit_snapshot(
    target: &crate::targets::ComputeTarget,
    writer: &WriterFence,
    runner: &Runner,
) -> Result<(), DeployError> {
    let snapshot = writer.unit_snapshot.as_ref().ok_or_else(|| {
        DeployError(format!("{} has no captured exact unit bytes", writer.label))
    })?;
    let script = format!(
        "STADO_UNIT_PATH={} STADO_UNIT_BODY={} STADO_UNIT_SHA={} STADO_UNIT_MODE={} STADO_UNIT_UID={} STADO_UNIT_GID={} /usr/bin/python3 - <<'PY'\n\
import base64, hashlib, os, stat, subprocess, tempfile\n\
path = os.path.expanduser(os.path.expandvars(os.environ['STADO_UNIT_PATH']))\n\
body = base64.b64decode(os.environ['STADO_UNIT_BODY'])\n\
expected = os.environ['STADO_UNIT_SHA']\n\
if hashlib.sha256(body).hexdigest() != expected:\n\
    raise SystemExit('captured unit bytes fail their digest')\n\
work = os.path.expanduser('~/.stado/work/storage-root-reconcile-units')\n\
os.makedirs(work, mode=0o700, exist_ok=True)\n\
fd, temporary = tempfile.mkstemp(prefix='unit.', dir=work)\n\
try:\n\
    with os.fdopen(fd, 'wb') as handle:\n\
        handle.write(body)\n\
        handle.flush()\n\
        os.fsync(handle.fileno())\n\
    command = ['/usr/bin/sudo', '-n', '/usr/bin/install',\n\
               '-m', os.environ['STADO_UNIT_MODE'],\n\
               '-o', os.environ['STADO_UNIT_UID'],\n\
               '-g', os.environ['STADO_UNIT_GID'], temporary, path]\n\
    result = subprocess.run(command, stdin=subprocess.DEVNULL,\n\
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,\n\
                            text=True, close_fds=False)\n\
    if result.returncode != 0:\n\
        raise SystemExit((result.stderr or result.stdout).strip())\n\
finally:\n\
    try:\n\
        os.unlink(temporary)\n\
    except FileNotFoundError:\n\
        pass\n\
info = os.lstat(path)\n\
if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):\n\
    raise SystemExit('restored unit is not a regular file')\n\
with open(path, 'rb') as handle:\n\
    if hashlib.sha256(handle.read()).hexdigest() != expected:\n\
        raise SystemExit('restored unit digest mismatch')\n\
print('STADO_UNIT_RESTORED\\t' + expected)\n\
PY",
        shlex_quote(&writer.path),
        shlex_quote(&snapshot.body_base64),
        shlex_quote(&snapshot.sha256),
        snapshot.mode,
        snapshot.uid,
        snapshot.gid,
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    let marker = format!("STADO_UNIT_RESTORED\t{}", snapshot.sha256);
    if !output.ok() || !output.stdout.lines().any(|line| line == marker) {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "exact unit bytes were not restored",
        )));
    }
    Ok(())
}

fn restored_state_matches(
    writer: &WriterFence,
    state: &super::service_label_print::LabelState,
    autostart: &BTreeMap<String, bool>,
    active_sha256: &str,
    forward_primary: &str,
    forward_backup: &str,
    rollback: bool,
) -> bool {
    if autostart != &writer.autostart {
        return false;
    }
    let should_be_loaded = writer.was_loaded || writer.was_runnable;
    if state.loaded() != should_be_loaded {
        return false;
    }
    if !should_be_loaded {
        return state.pid.is_none();
    }
    if let Some(pid) = state.pid.as_deref() {
        if pid == "0"
            || state.process_started_at.is_none()
            || state.process_executable.is_none()
            || state.process_device.is_none()
            || state.process_inode.is_none_or(|inode| inode == 0)
        {
            return false;
        }
        let expected_sha256 = if writer.role == "object-api"
            || writer
                .prior_executable
                .as_deref()
                .is_some_and(|path| executable_name(path) == "stado")
        {
            Some(active_sha256)
        } else {
            writer.prior_sha256.as_deref()
        };
        if state.process_sha256.as_deref() != expected_sha256 {
            return false;
        }
    } else if writer.role == "object-api"
        || (state.state.is_none()
            && state.last_exit_code.is_none()
            && state.restart.is_none()
            && state.triggers.is_none())
    {
        return false;
    }
    if writer.role != "object-api" {
        return true;
    }
    let loaded = &state.loaded_environment;
    if !loaded.contains_key("WC_STORAGE_BACKEND") {
        return true;
    }
    let expected_config = writer
        .prior_loaded_environment
        .get("STADO_CONFIG")
        .or_else(|| writer.unit_declared_environment.get("STADO_CONFIG"))
        .or_else(|| writer.registry_declared_environment.get("STADO_CONFIG"))
        .map(String::as_str);
    if loaded.get("WC_STORAGE_BACKEND").map(String::as_str) != Some("local")
        || loaded.get("STADO_CONFIG").map(String::as_str) != expected_config
    {
        return false;
    }
    if rollback {
        loaded.get("WC_LOCAL_STORAGE_PATH").map(String::as_str) == Some(forward_backup)
            && loaded
                .get("WC_BACKUP_STORAGE_BACKEND")
                .is_none_or(String::is_empty)
            && loaded
                .get("WC_BACKUP_LOCAL_STORAGE_PATH")
                .is_none_or(String::is_empty)
    } else {
        loaded.get("WC_LOCAL_STORAGE_PATH").map(String::as_str) == Some(forward_primary)
            && loaded.get("WC_BACKUP_STORAGE_BACKEND").map(String::as_str) == Some("local")
            && loaded
                .get("WC_BACKUP_LOCAL_STORAGE_PATH")
                .map(String::as_str)
                == Some(forward_backup)
    }
}

async fn activate_lifecycle_fence(
    storage_target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
    rollback: bool,
) -> Result<LifecycleFence, DeployError> {
    let mut fence = read_fence(storage_target, transaction, runner)
        .await?
        .ok_or_else(|| DeployError("durable lifecycle fence is absent".to_string()))?;
    validate_prepared_fence(&fence)?;
    let served = fence
        .preflight
        .get("served_store")
        .and_then(Value::as_object)
        .ok_or_else(|| DeployError("fence has no physical/API correlation evidence".to_string()))?;
    let forward_primary = served
        .get("primary_root")
        .and_then(Value::as_str)
        .ok_or_else(|| DeployError("served-store evidence omitted A root".to_string()))?
        .to_string();
    let forward_backup = served
        .get("backup_root")
        .and_then(Value::as_str)
        .ok_or_else(|| DeployError("served-store evidence omitted B root".to_string()))?
        .to_string();
    let final_status = if rollback { "rolled_back" } else { "activated" };
    let admissible = if rollback {
        matches!(
            fence.status.as_str(),
            "preparing" | "fenced" | "rolling_back" | "restoring" | "rolled_back"
        )
    } else {
        matches!(
            fence.status.as_str(),
            "fenced" | "activating" | "restoring" | "activated"
        )
    };
    if !admissible {
        return Err(DeployError(format!(
            "lifecycle fence cannot {} from {}",
            if rollback { "roll back" } else { "activate" },
            fence.status
        )));
    }
    if fence.status == final_status {
        return Ok(fence);
    }
    if !matches!(fence.status.as_str(), "activating" | "restoring") {
        fence.status = if rollback { "rolling_back" } else { "activating" }.to_string();
        write_fence(storage_target, transaction, &fence, runner).await?;
    }

    let staged_runtime = fence
        .staged_runtime
        .clone()
        .ok_or_else(|| DeployError("lifecycle fence has no staged declared runtime".to_string()))?;
    let active_sha256 =
        super::host_release::activate_staged_program(storage_target, &staged_runtime, runner)
            .await?;
    if fence
        .activation_sha256
        .as_deref()
        .is_some_and(|digest| digest != active_sha256)
    {
        return Err(DeployError(
            "persisted activation digest differs from the adopted active runtime".to_string(),
        ));
    }
    fence.activation_sha256 = Some(active_sha256.clone());
    fence.activated_at.get_or_insert_with(|| Utc::now().timestamp());
    fence.status = "restoring".to_string();
    write_fence(storage_target, transaction, &fence, runner).await?;

    let mut order = (0..fence.writers.len()).collect::<Vec<_>>();
    order.sort_by_key(|index| restore_priority(&fence.writers[*index].role));
    let mut restored_store = None;
    for index in order {
        let label = fence.writers[index].label.clone();
        let mut state = super::service_label_print::print_label(
            storage_target,
            &label,
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        let mut autostart = service::label_autostart(storage_target, &label, runner).await?;
        let unit_matches = if fence.writers[index].role == "object-api" {
            true
        } else {
            snapshot_unit_file(storage_target, &fence.writers[index].path, runner).await?
                == fence.writers[index].unit_snapshot
        };
        let adopted = unit_matches
            && restored_state_matches(
                &fence.writers[index],
                &state,
                &autostart,
                &active_sha256,
                &forward_primary,
                &forward_backup,
                rollback,
            );
        if fence.writers[index].status == "restored" && !adopted {
            return Err(DeployError(format!(
                "{label} drifted after its durable restored result"
            )));
        }
        if !adopted {
            if fence.writers[index].status != "restore_intent" {
                fence.writers[index].status = "restore_intent".to_string();
                write_fence(storage_target, transaction, &fence, runner).await?;
            }
            if fence.writers[index].role != "object-api"
                && fence.writers[index].unit_snapshot.is_some()
            {
                restore_unit_snapshot(storage_target, &fence.writers[index], runner).await?;
            }
            let requires_load =
                fence.writers[index].was_loaded || fence.writers[index].was_runnable;
            if requires_load {
                if fence.writers[index].role == "object-api" {
                    let prepared = if rollback {
                        fence.writers[index].rollback_object_recovery.as_ref()
                    } else {
                        fence.writers[index].forward_object_recovery.as_ref()
                    }
                    .ok_or_else(|| {
                        DeployError(format!("{label} has no prepared recovery configuration"))
                    })?;
                    let recovered = host_channel::run_script_with_timeout(
                        storage_target,
                        &prepared.body,
                        Duration::from_secs(240),
                        runner,
                    )
                    .await?;
                    if !recovered.ok() {
                        return Err(DeployError(format!(
                            "{label} did not restore through its prepared configuration: {}",
                            host_channel::last_error_line(&recovered, "remote command failed")
                        )));
                    }
                } else {
                    let writer = &fence.writers[index];
                    if !writer.autostart.values().copied().any(|enabled| enabled) {
                        let scope = writer
                            .loaded_domains
                            .first()
                            .map(String::as_str)
                            .or_else(|| writer.autostart.keys().next().map(String::as_str))
                            .ok_or_else(|| {
                                DeployError(format!(
                                    "{label} has no captured init-system scope for restoration"
                                ))
                            })?;
                        service::set_label_autostart(storage_target, &label, scope, true, runner)
                            .await?;
                    }
                    let declared = managed_writer(storage_target, writer);
                    let restarted =
                        service::restart_service(storage_target, &declared, runner).await?;
                    if !restarted.succeeded("restarted") {
                        return Err(DeployError(format!(
                            "{label} did not restore: {}",
                            restarted.failure()
                        )));
                    }
                }
            }
            for (scope, enabled) in fence.writers[index].autostart.clone() {
                service::set_label_autostart(storage_target, &label, &scope, enabled, runner)
                    .await?;
            }
            state = super::service_label_print::print_label(
                storage_target,
                &label,
                service::BootoutScope::Any,
                runner,
            )
            .await?;
            autostart = service::label_autostart(storage_target, &label, runner).await?;
            if !restored_state_matches(
                &fence.writers[index],
                &state,
                &autostart,
                &active_sha256,
                &forward_primary,
                &forward_backup,
                rollback,
            ) {
                return Err(DeployError(format!(
                    "{label} does not match its captured lifecycle and prepared runtime"
                )));
            }
            if fence.writers[index].role != "object-api"
                && snapshot_unit_file(storage_target, &fence.writers[index].path, runner).await?
                    != fence.writers[index].unit_snapshot
            {
                return Err(DeployError(format!(
                    "{label} unit definition differs from its captured exact bytes"
                )));
            }
        }
        let restored_route = if fence.writers[index].role == "object-api" {
            let port = fence.writers[index].listener_port.ok_or_else(|| {
                DeployError("object API listener port is absent from its fence".to_string())
            })?;
            let correlation = correlate_served_store(
                storage_target,
                port,
                &fence.preflight,
                !rollback,
                runner,
            )
            .await?;
            let authority = correlation
                .get("object_authority")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let accepted = if rollback {
                matches!(authority, "B" | "identical")
            } else {
                matches!(authority, "A" | "identical")
            };
            if !accepted {
                return Err(DeployError(format!(
                    "{label} serves {authority:?} after {} recovery",
                    if rollback { "B-only" } else { "forward A+B" }
                )));
            }
            let prepared_sha256 = if rollback {
                fence.writers[index]
                    .rollback_object_recovery
                    .as_ref()
            } else {
                fence.writers[index]
                    .forward_object_recovery
                    .as_ref()
            }
            .map(|script| script.sha256.clone());
            Some(json!({
                "configuration": {
                    "prepared_script_sha256": prepared_sha256,
                    "loaded_environment_observed": state
                        .loaded_environment
                        .contains_key("WC_STORAGE_BACKEND"),
                    "observed_loaded_environment": state.loaded_environment.clone(),
                    "unit_declared_environment":
                        fence.writers[index].unit_declared_environment.clone(),
                    "registry_declared_environment":
                        fence.writers[index].registry_declared_environment.clone(),
                },
                "served_store": correlation,
            }))
        } else {
            None
        };
        fence.writers[index].restored_pid = state.pid;
        fence.writers[index].restored_started_at = state.process_started_at;
        fence.writers[index].restored_loaded_environment = state.loaded_environment;
        fence.writers[index].restored_executable = state.process_executable;
        fence.writers[index].restored_sha256 = state.process_sha256;
        fence.writers[index].restored_device = state.process_device;
        fence.writers[index].restored_inode = state.process_inode;
        fence.writers[index].restored_route = restored_route;
        fence.writers[index].status = "restored".to_string();
        write_fence(storage_target, transaction, &fence, runner).await?;
        if fence.writers[index].role == "object-api" {
            let endpoint = fence.writers[index]
                .restored_route
                .as_ref()
                .and_then(|proof| proof.get("served_store"))
                .and_then(|proof| proof.get("endpoint"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    DeployError("recovered object API proof omitted its endpoint".to_string())
                })?;
            let backend = crate::queue::StadoObjectBackend::new(
                endpoint,
                "probierz",
                "~/.stado/queue-object-api-token",
                "",
            )
            .map_err(|error| {
                DeployError(format!("cannot bind lease renewal to recovered object API: {error}"))
            })?;
            let store = crate::queue::JobStorage::with_backend(
                std::sync::Arc::new(backend),
                "recovered-stado-object",
            );
            renew_fence_leases(&store, &mut fence).await?;
            write_fence(storage_target, transaction, &fence, runner).await?;
            restored_store = Some(store);
        } else if restored_store.is_none() {
            return Err(DeployError(
                "a writer would resume before the object API restored A and renewed every lease"
                    .to_string(),
            ));
        }
    }

    let store = restored_store.ok_or_else(|| {
        DeployError("object API did not establish the recovered A queue".to_string())
    })?;
    for acquisition in &fence.lease_acquisitions {
        let lease = acquisition.lease.as_ref().ok_or_else(|| {
            DeployError(format!(
                "placement lease acquisition for {} has no result",
                acquisition.subject_id
            ))
        })?;
        let released = crate::autonomy::storage::release_placement_lease(
            &store,
            &lease.subject_id,
            &lease.token,
        )
        .await
        .map_err(|error| DeployError(format!("cannot release {}: {error}", lease.subject_id)))?;
        if !released {
            return Err(DeployError(format!(
                "placement lease ownership changed for {} before release",
                lease.subject_id
            )));
        }
    }
    if !fence.queue.was_paused {
        crate::queue::control::set_paused(
            &store,
            false,
            &format!(
                "storage reconciliation {transaction} {}",
                if rollback { "rolled back" } else { "activated" }
            ),
            "stado storage-root-reconcile",
        )
        .await
        .map_err(|error| DeployError(format!("cannot resume queue after activation: {error}")))?;
    }
    fence.queue.resumed = true;
    fence.status = final_status.to_string();
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

async fn remote_phase(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    phase: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let output = host_channel::run_script_with_timeout(
        target,
        &bind_remote_script(phase, transaction, ""),
        TIMEOUT,
        runner,
    )
    .await?;
    let mut payload = None;
    for line in output.stdout.lines() {
        if let Some(message) = line.strip_prefix("STADO_STORAGE_RECONCILE_ERROR\t") {
            return Err(DeployError(message.to_string()));
        }
        if let Some(encoded) = line.strip_prefix("STADO_STORAGE_RECONCILE\t") {
            payload = serde_json::from_str::<Value>(encoded).ok();
        }
    }
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "storage-root reconciliation phase failed",
        )));
    }
    payload.ok_or_else(|| {
        DeployError("storage-root reconciliation phase returned no durable receipt".to_string())
    })
}

async fn typed_lifecycle_decisions(
    transaction: &str,
    receipt: &Value,
) -> Result<Vec<Value>, DeployError> {
    let backup_paths = receipt
        .get("backup_objects")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("checkpoint receipt omitted backup objects".to_string()))?
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let primary_only = receipt
        .get("primary_objects")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("checkpoint receipt omitted primary objects".to_string()))?
        .iter()
        .filter_map(|item| item.get("path").and_then(Value::as_str))
        .filter(|path| !backup_paths.contains(path))
        .filter_map(|path| path.strip_prefix("ecosystem/probierz/"))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let snapshot = transaction_directory(transaction)?.join("effective-lifecycle.checkpoint");
    if receipt
        .get("effective_lifecycle_checkpoint")
        .and_then(Value::as_str)
        .map(Path::new)
        != Some(snapshot.as_path())
    {
        return Err(DeployError(
            "checkpoint receipt does not name the resident immutable lifecycle snapshot"
                .to_string(),
        ));
    }
    let backend = crate::queue::LocalBackend::open_existing(&snapshot)
        .map_err(|error| DeployError(format!("cannot open lifecycle checkpoint: {error}")))?;
    let store = crate::queue::JobStorage::with_backend(
        std::sync::Arc::new(backend),
        "immutable-local-snapshot",
    );
    crate::monitor::reap::classify_reconciliation_snapshot(&store, &primary_only)
        .await
        .map_err(|error| DeployError(format!("typed lifecycle snapshot refused: {error}")))
}

async fn record_typed_lifecycle_decisions(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    decisions: &[Value],
    runner: &Runner,
) -> Result<Value, DeployError> {
    let encoded = serde_json::to_string(decisions)
        .map_err(|error| DeployError(format!("cannot encode typed lifecycle decisions: {error}")))?;
    let output = host_channel::run_script_with_timeout(
        target,
        &bind_remote_script(RECORD_LIFECYCLE_DECISIONS, transaction, &encoded),
        TIMEOUT,
        runner,
    )
    .await?;
    parse_remote_payload(&output)
}

fn report(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    phase: &str,
    receipt: Value,
    fence: Option<&LifecycleFence>,
) -> Result<Value, DeployError> {
    let mut report = host_channel::base_report(target);
    report.insert("transaction".to_string(), json!(transaction));
    report.insert("phase".to_string(), json!(phase));
    report.insert("receipt".to_string(), receipt);
    report.insert(
        "lifecycle_fence".to_string(),
        match fence {
            Some(fence) => serde_json::to_value(fence)
                .map_err(|error| DeployError(format!("cannot report lifecycle fence: {error}")))?,
            None => Value::Null,
        },
    );
    report.insert("status".to_string(), json!("ok"));
    Ok(Value::Object(report))
}

async fn reconcile_host_inner(
    target_name: &str,
    transaction: &str,
    phase: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    validate_transaction(transaction)?;
    if !matches!(phase, RUN | RESUME | STATUS | ROLLBACK | FINALIZE) {
        return Err(DeployError(format!(
            "phase must be {RUN}, {RESUME}, {STATUS}, {ROLLBACK}, or {FINALIZE}, not {phase:?}"
        )));
    }
    let target = match RESIDENT_TARGET.get() {
        Some(target) => target.clone(),
        None => host_channel::canonical_target(target_name).await?,
    };
    if target.name != target_name {
        return Err(DeployError(
            "captured resident target does not match the transaction target".to_string(),
        ));
    }
    if phase == STATUS {
        let receipt = remote_phase(&target, transaction, STATUS, runner).await?;
        let fence = read_fence(&target, transaction, runner).await?;
        return report(&target, transaction, phase, receipt, fence.as_ref());
    }
    if phase == FINALIZE {
        let fence = read_fence(&target, transaction, runner)
            .await?
            .ok_or_else(|| DeployError("durable lifecycle fence is absent".to_string()))?;
        if fence.status != "activated" {
            return Err(DeployError(format!(
                "finalize observes lifecycle cleanup only after activation, not {}",
                fence.status
            )));
        }
        let receipt = remote_phase(&target, transaction, FINALIZE, runner).await?;
        return report(&target, transaction, phase, receipt, Some(&fence));
    }
    if phase == ROLLBACK {
        let receipt = remote_phase(&target, transaction, STATUS, runner).await?;
        let receipt_status = receipt
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(receipt_status, "absent" | "checkpointing" | "checkpoint_ready") {
            return Err(DeployError(format!(
                "rollback is only safe before data activation, not receipt state {receipt_status:?}"
            )));
        }
        let fence = activate_lifecycle_fence(&target, transaction, runner, true).await?;
        return report(&target, transaction, phase, receipt, Some(&fence));
    }

    let existing = read_fence(&target, transaction, runner).await?;
    if existing.as_ref().is_some_and(|fence| fence.status == "rolled_back") {
        return Err(DeployError(
            "a rolled-back transaction cannot be reactivated; choose a new transaction id"
                .to_string(),
        ));
    }
    let mut fence = match existing {
        Some(fence)
            if matches!(
                fence.status.as_str(),
                "activating" | "restoring" | "activated"
            ) =>
        {
            fence
        }
        _ => prepare_lifecycle_fence(&target, transaction, runner).await?,
    };
    if fence.status == "already_reconciled" {
        let receipt = json!({
            "schema": "stado.storage-root-reconcile.v1",
            "transaction": transaction,
            "status": "already_reconciled",
            "preflight": fence.preflight.clone(),
        });
        return report(&target, transaction, phase, receipt, Some(&fence));
    }
    if fence.status == "fenced" {
        let checkpoint = remote_phase(&target, transaction, CHECKPOINT, runner).await?;
        let checkpoint_decisions = typed_lifecycle_decisions(transaction, &checkpoint).await?;
        record_typed_lifecycle_decisions(
            &target,
            transaction,
            &checkpoint_decisions,
            runner,
        )
        .await?;
        recheck_lifecycle_fence(&target, transaction, runner).await?;
        let committed = remote_phase(&target, transaction, APPLY, runner).await?;
        let committed_decisions = typed_lifecycle_decisions(transaction, &committed).await?;
        if committed_decisions != checkpoint_decisions {
            return Err(DeployError(
                "typed lifecycle decisions changed between checkpoint and data commit".to_string(),
            ));
        }
        record_typed_lifecycle_decisions(
            &target,
            transaction,
            &committed_decisions,
            runner,
        )
        .await?;
        fence = recheck_lifecycle_fence(&target, transaction, runner).await?;
        verify_resident_lock(transaction)?;
        validate_prepared_fence(&fence)?;
        fence = activate_lifecycle_fence(&target, transaction, runner, false).await?;
    } else if fence.status != "activated" {
        let committed = remote_phase(&target, transaction, APPLY, runner).await?;
        let decisions = typed_lifecycle_decisions(transaction, &committed).await?;
        record_typed_lifecycle_decisions(&target, transaction, &decisions, runner).await?;
        verify_resident_lock(transaction)?;
        validate_prepared_fence(&fence)?;
        fence = activate_lifecycle_fence(&target, transaction, runner, false).await?;
    }
    let receipt = remote_phase(&target, transaction, ACTIVATE, runner).await?;
    report(&target, transaction, phase, receipt, Some(&fence))
}
fn verify_resident_lock(transaction: &str) -> Result<(), DeployError> {
    let fd = RESIDENT_LOCK_FD
        .get()
        .copied()
        .ok_or_else(|| DeployError("resident reconciliation lock descriptor is absent".to_string()))?;
    let lock = transaction_directory(transaction)?
        .parent()
        .and_then(Path::parent)
        .expect("validated transaction directory has a recovery parent")
        .join("storage-root-reconcile.lock");
    let path_metadata = std::fs::metadata(&lock)
        .map_err(|error| DeployError(format!("cannot stat {}: {error}", lock.display())))?;
    let descriptor_path = if cfg!(target_os = "linux") {
        PathBuf::from(format!("/proc/self/fd/{fd}"))
    } else {
        PathBuf::from(format!("/dev/fd/{fd}"))
    };
    let descriptor_metadata = std::fs::metadata(&descriptor_path).map_err(|error| {
        DeployError(format!(
            "resident reconciliation lock descriptor {fd} is invalid: {error}"
        ))
    })?;
    if path_metadata.dev() != descriptor_metadata.dev()
        || path_metadata.ino() != descriptor_metadata.ino()
    {
        return Err(DeployError(
            "resident reconciliation lock no longer maps the canonical transaction lock"
                .to_string(),
        ));
    }
    Ok(())
}

fn transaction_directory(transaction: &str) -> Result<PathBuf, DeployError> {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .ok_or_else(|| DeployError("resident transaction worker has no HOME".to_string()))?;
    Ok(PathBuf::from(home)
        .join(".stado/recovery/storage-root-reconcile")
        .join(transaction))
}

fn sha256_file(path: &Path) -> Result<String, DeployError> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| DeployError(format!("cannot open {}: {error}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| DeployError(format!("cannot hash {}: {error}", path.display())))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn atomic_owner(path: &Path, owner: &Value) -> Result<(), DeployError> {
    let parent = path
        .parent()
        .ok_or_else(|| DeployError("operation owner has no parent directory".to_string()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| DeployError(format!("cannot create {}: {error}", parent.display())))?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| DeployError(format!("cannot protect {}: {error}", parent.display())))?;
    let temporary = parent.join(format!(".operation-owner.{}.new", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| DeployError(format!("cannot create {}: {error}", temporary.display())))?;
    serde_json::to_writer(&mut file, owner)
        .map_err(|error| DeployError(format!("cannot encode operation owner: {error}")))?;
    file.write_all(b"\n")
        .map_err(|error| DeployError(format!("cannot finish operation owner: {error}")))?;
    file.sync_all()
        .map_err(|error| DeployError(format!("cannot sync operation owner: {error}")))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| DeployError(format!("cannot publish operation owner: {error}")))?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| DeployError(format!("cannot sync {}: {error}", parent.display())))?;
    Ok(())
}

pub async fn reconcile_host_worker(
    target: crate::targets::ComputeTarget,
    transaction: &str,
    phase: &str,
    source_revision: &str,
    tool_sha256: &str,
    runner_gate: Option<Value>,
    runner: &Runner,
) -> Result<Value, DeployError> {
    use fs2::FileExt;

    validate_transaction(transaction)?;
    if !matches!(phase, RUN | RESUME | ROLLBACK | FINALIZE) {
        return Err(DeployError(format!(
            "resident worker action must be {RUN}, {RESUME}, {ROLLBACK}, or {FINALIZE}"
        )));
    }
    if !host_channel::target_is_this_host(&target) {
        return Err(DeployError(
            "native reconciliation worker is not resident on its captured target".to_string(),
        ));
    }
    if source_revision != crate::build_identity::SOURCE_REVISION
        || source_revision == crate::build_identity::UNKNOWN_REVISION
        || source_revision.ends_with("-dirty")
    {
        return Err(DeployError(
            "resident transaction tool does not carry one clean exact source revision".to_string(),
        ));
    }
    let executable = std::env::current_exe()
        .map_err(|error| DeployError(format!("cannot locate transaction tool: {error}")))?;
    let actual_sha256 = sha256_file(&executable)?;
    if actual_sha256 != tool_sha256 {
        return Err(DeployError(
            "resident transaction tool digest differs from launch request".to_string(),
        ));
    }
    let directory = transaction_directory(transaction)?;
    let lock_path = directory
        .parent()
        .and_then(Path::parent)
        .expect("validated transaction directory has a recovery parent")
        .join("storage-root-reconcile.lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| DeployError(format!("cannot create {}: {error}", parent.display())))?;
    }
    let operation_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(|error| DeployError(format!("cannot open native transaction lock: {error}")))?;
    operation_lock
        .try_lock_exclusive()
        .map_err(|error| DeployError(format!("another reconciliation owns the native lock: {error}")))?;
    let descriptor = operation_lock.as_raw_fd();
    // SAFETY: `descriptor` is owned by `operation_lock`; F_GETFD/F_SETFD do
    // not consume it. Clearing CLOEXEC deliberately carries the same locked
    // open-file description through every locally spawned lifecycle effect.
    let flags = unsafe { nix::libc::fcntl(descriptor, nix::libc::F_GETFD) };
    if flags < 0
        || unsafe {
            nix::libc::fcntl(
                descriptor,
                nix::libc::F_SETFD,
                flags & !nix::libc::FD_CLOEXEC,
            )
        } < 0
    {
        return Err(DeployError(format!(
            "cannot make native transaction lock inheritable: {}",
            std::io::Error::last_os_error()
        )));
    }
    let lock_metadata = std::fs::metadata(&lock_path)
        .map_err(|error| DeployError(format!("cannot stat native lock path: {error}")))?;
    let descriptor_metadata = operation_lock
        .metadata()
        .map_err(|error| DeployError(format!("cannot stat native lock descriptor: {error}")))?;
    if lock_metadata.dev() != descriptor_metadata.dev()
        || lock_metadata.ino() != descriptor_metadata.ino()
    {
        return Err(DeployError(
            "opened descriptor is not the canonical reconciliation lock".to_string(),
        ));
    }
    RESIDENT_LOCK_FD
        .set(descriptor)
        .map_err(|_| DeployError("resident lock descriptor was already initialized".to_string()))?;
    RESIDENT_TARGET
        .set(target.clone())
        .map_err(|_| DeployError("resident target was already initialized".to_string()))?;
    let token = uuid::Uuid::new_v4().to_string();
    RESIDENT_OWNER_TOKEN
        .set(token.clone())
        .map_err(|_| DeployError("resident owner token was already initialized".to_string()))?;
    if let Some(gate) = runner_gate {
        RESIDENT_RUNNER_GATE
            .set(gate)
            .map_err(|_| DeployError("resident runner gate was already initialized".to_string()))?;
    }
    let owner_path = directory.join("operation-owner.json");
    let revision = std::fs::read(&owner_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|owner| owner.get("revision").and_then(Value::as_u64))
        .unwrap_or_default()
        .saturating_add(1);
    let mut owner = json!({
        "schema": "stado.storage-root-owner.v1",
        "transaction": transaction,
        "target": target.name.clone(),
        "action": phase,
        "status": "executing",
        "pid": std::process::id(),
        "token": token,
        "source_revision": source_revision,
        "tool_path": executable,
        "tool_sha256": actual_sha256,
        "lock_device": descriptor_metadata.dev(),
        "lock_inode": descriptor_metadata.ino(),
        "native_service": format!("com.wisent.stado-storage-root-reconcile.{transaction}"),
        "revision": revision,
        "started_at": Utc::now().to_rfc3339(),
        "updated_at": Utc::now().to_rfc3339(),
    });
    atomic_owner(&owner_path, &owner)?;
    let outcome = reconcile_host_inner(&target.name, transaction, phase, runner).await;
    let fields = owner
        .as_object_mut()
        .expect("resident operation owner is an object");
    fields.insert("updated_at".to_string(), json!(Utc::now().to_rfc3339()));
    match &outcome {
        Ok(result) => {
            fields.insert("status".to_string(), json!("succeeded"));
            fields.insert("result".to_string(), result.clone());
        }
        Err(error) => {
            fields.insert("status".to_string(), json!("failed"));
            fields.insert("error".to_string(), json!(error.to_string()));
        }
    }
    atomic_owner(&owner_path, &owner)?;
    drop(operation_lock);
    outcome
}

fn launch_worker_script(
    transaction: &str,
    staged_tool: &str,
    canonical_tool: &str,
    tool_sha256: &str,
    arguments: &[String],
) -> Result<String, DeployError> {
    let arguments = serde_json::to_vec(arguments)
        .map_err(|error| DeployError(format!("cannot encode worker arguments: {error}")))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(arguments);
    Ok(r##"set -euo pipefail
STADO_WORKER_ARGS=@ARGS@ STADO_STAGED_TOOL=@STAGED@ STADO_CANONICAL_TOOL=@TOOL@ STADO_TOOL_SHA256=@SHA@ STADO_TRANSACTION=@TX@ /usr/bin/python3 - <<'PY'
import base64, hashlib, json, os, platform, plistlib, re, shlex, stat, subprocess, time
tx = os.environ["STADO_TRANSACTION"]
staged = os.path.expanduser(os.path.expandvars(os.environ["STADO_STAGED_TOOL"]))
tool = os.path.expanduser(os.path.expandvars(os.environ["STADO_CANONICAL_TOOL"]))
expected = os.environ["STADO_TOOL_SHA256"]
work = os.path.dirname(tool)
home = os.path.expanduser("~")
owner_path = os.path.join(work, "operation-owner.json")
label = "com.wisent.stado-storage-root-reconcile." + tx
log_path = os.path.join(work, "transaction-worker.log")
system = platform.system()
os.makedirs(work, mode=0o700, exist_ok=True)
arguments = json.loads(base64.b64decode(os.environ["STADO_WORKER_ARGS"]))
argv = [tool] + arguments

def checked(argv, accepted=(0,)):
    result = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, close_fds=True)
    if result.returncode not in accepted:
        detail = (result.stderr or result.stdout).strip().splitlines()
        raise SystemExit(detail[-1] if detail else "native service command failed")
    return result

def manager_pid():
    if system == "Darwin":
        result = subprocess.run(
            ["/usr/bin/sudo", "-n", "/bin/launchctl", "print", "system/" + label],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, close_fds=True)
        if result.returncode != 0:
            return None
        match = re.search(r"(?m)^\s*pid = ([1-9][0-9]*)\s*$", result.stdout)
        return int(match.group(1)) if match else None
    if system == "Linux":
        result = subprocess.run(
            ["/usr/bin/sudo", "-n", "/bin/systemctl", "show",
             "--property=MainPID", "--value", label + ".service"],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, close_fds=True)
        value = result.stdout.strip()
        return int(value) if result.returncode == 0 and value.isdigit() and int(value) > 0 else None
    raise SystemExit("native reconciliation worker requires Darwin launchd or Linux systemd")

try:
    with open(owner_path, encoding="utf-8") as handle:
        prior_owner = json.load(handle)
    prior_pid = int(prior_owner.get("pid", 0))
    if (prior_owner.get("schema") == "stado.storage-root-owner.v1"
            and prior_owner.get("transaction") == tx
            and prior_owner.get("status") == "executing"
            and prior_owner.get("tool_sha256") == expected
            and manager_pid() == prior_pid):
        prior_owner.pop("token", None)
        print("STADO_RECONCILE_OWNER\t" + json.dumps(
            prior_owner, sort_keys=True, separators=(",", ":")))
        raise SystemExit(0)
except (FileNotFoundError, ValueError, json.JSONDecodeError):
    pass

info = os.lstat(staged)
if not stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
    raise SystemExit("staged transaction tool is not a regular file")
with open(staged, "rb") as handle:
    if hashlib.sha256(handle.read()).hexdigest() != expected:
        raise SystemExit("staged transaction tool digest mismatch")
os.chmod(staged, 0o700)
os.replace(staged, tool)
directory_fd = os.open(work, os.O_RDONLY)
os.fsync(directory_fd)
os.close(directory_fd)

if system == "Darwin":
    unit = {
        "Label": label,
        "ProgramArguments": argv,
        "EnvironmentVariables": {
            "HOME": home,
            "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        },
        "WorkingDirectory": home,
        "RunAtLoad": True,
        "KeepAlive": False,
        "ProcessType": "Background",
        "UserName": checked(["/usr/bin/id", "-un"]).stdout.strip(),
        "StandardOutPath": log_path,
        "StandardErrorPath": log_path,
    }
    prepared = os.path.join(work, "native-worker.plist")
    with open(prepared + ".new", "wb") as handle:
        plistlib.dump(unit, handle, fmt=plistlib.FMT_XML, sort_keys=False)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(prepared + ".new", prepared)
    unit_path = "/Library/LaunchDaemons/" + label + ".plist"
    checked(["/usr/bin/sudo", "-n", "/usr/bin/install", "-m", "644",
             "-o", "root", "-g", "wheel", prepared, unit_path])
    checked(["/usr/bin/sudo", "-n", "/bin/launchctl", "bootout", "system/" + label],
            accepted=(0, 3, 113))
    checked(["/usr/bin/sudo", "-n", "/bin/launchctl", "enable", "system/" + label])
    checked(["/usr/bin/sudo", "-n", "/bin/launchctl", "bootstrap", "system", unit_path])
    checked(["/usr/bin/sudo", "-n", "/bin/launchctl", "kickstart", "system/" + label])
else:
    wrapper = os.path.join(work, "native-worker")
    with open(wrapper + ".new", "w", encoding="utf-8") as handle:
        handle.write("#!/bin/sh\nexec " + shlex.join(argv) + "\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.chmod(wrapper + ".new", 0o700)
    os.replace(wrapper + ".new", wrapper)
    unit_path = "/etc/systemd/system/" + label + ".service"
    prepared = os.path.join(work, "native-worker.service")
    unit = "\n".join([
        "[Unit]",
        "Description=Stado storage authority reconciliation " + tx,
        "After=network-online.target",
        "[Service]",
        "Type=simple",
        "User=" + checked(["/usr/bin/id", "-un"]).stdout.strip(),
        "Environment=HOME=" + home,
        "WorkingDirectory=" + home,
        "ExecStart=" + wrapper,
        "Restart=no",
        "[Install]",
        "WantedBy=multi-user.target",
        "",
    ])
    with open(prepared + ".new", "w", encoding="utf-8") as handle:
        handle.write(unit)
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(prepared + ".new", prepared)
    checked(["/usr/bin/sudo", "-n", "/usr/bin/install", "-m", "644",
             "-o", "root", "-g", "root", prepared, unit_path])
    checked(["/usr/bin/sudo", "-n", "/bin/systemctl", "daemon-reload"])
    checked(["/usr/bin/sudo", "-n", "/bin/systemctl", "enable", label + ".service"])
    checked(["/usr/bin/sudo", "-n", "/bin/systemctl", "restart", label + ".service"])

deadline = time.monotonic() + 30
while time.monotonic() < deadline:
    try:
        with open(owner_path, encoding="utf-8") as handle:
            owner = json.load(handle)
        owner_pid = int(owner.get("pid", 0))
        if (owner.get("schema") == "stado.storage-root-owner.v1"
                and owner.get("transaction") == tx
                and owner.get("status") == "executing"
                and owner.get("tool_sha256") == expected
                and manager_pid() == owner_pid):
            owner.pop("token", None)
            print("STADO_RECONCILE_OWNER\t" + json.dumps(
                owner, sort_keys=True, separators=(",", ":")))
            raise SystemExit(0)
    except (FileNotFoundError, ValueError, json.JSONDecodeError):
        pass
    time.sleep(0.1)
raise SystemExit("native reconciliation worker did not record manager-bound ownership")
PY"##
        .replace("@ARGS@", &shlex_quote(&encoded))
        .replace("@STAGED@", &shlex_quote(staged_tool))
        .replace("@TOOL@", &shlex_quote(canonical_tool))
        .replace("@SHA@", &shlex_quote(tool_sha256))
        .replace("@TX@", &shlex_quote(transaction)))
}

async fn read_operation_owner(
    target: &crate::targets::ComputeTarget,
    transaction: &str,
    runner: &Runner,
) -> Result<Option<Value>, DeployError> {
    let script = format!(
        "STADO_TRANSACTION={} /usr/bin/python3 - <<'PY'\n\
import json, os, stat\n\
tx = os.environ['STADO_TRANSACTION']\n\
path = os.path.expanduser('~/.stado/recovery/storage-root-reconcile/' + tx + '/operation-owner.json')\n\
try:\n\
    info = os.lstat(path)\n\
    if not stat.S_ISREG(info.st_mode):\n\
        raise SystemExit('operation owner is not a regular file')\n\
    with open(path, encoding='utf-8') as handle:\n\
        owner = json.load(handle)\n\
except FileNotFoundError:\n\
    print('STADO_RECONCILE_OWNER\\tabsent')\n\
    raise SystemExit(0)\n\
if owner.get('schema') != 'stado.storage-root-owner.v1' or owner.get('transaction') != tx:\n\
    raise SystemExit('operation owner identity is invalid')\n\
owner.pop('token', None)\n\
print('STADO_RECONCILE_OWNER\\t' + json.dumps(owner, sort_keys=True, separators=(',', ':')))\n\
PY",
        shlex_quote(transaction)
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "operation owner could not be read",
        )));
    }
    for line in output.stdout.lines() {
        let Some(encoded) = line.strip_prefix("STADO_RECONCILE_OWNER\t") else {
            continue;
        };
        if encoded == "absent" {
            return Ok(None);
        }
        return serde_json::from_str(encoded)
            .map(Some)
            .map_err(|error| DeployError(format!("operation owner is invalid: {error}")));
    }
    Err(DeployError(
        "operation owner reader returned no marker".to_string(),
    ))
}

pub async fn reconcile_host(
    target_name: &str,
    transaction: &str,
    phase: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    validate_transaction(transaction)?;
    let target = host_channel::canonical_target(target_name).await?;
    if phase == STATUS {
        let mut status = reconcile_host_inner(target_name, transaction, STATUS, runner).await?;
        let owner = read_operation_owner(&target, transaction, runner).await?;
        status
            .as_object_mut()
            .expect("storage-root status report is an object")
            .insert("operation_owner".to_string(), owner.unwrap_or(Value::Null));
        return Ok(status);
    }
    if !matches!(phase, RUN | RESUME | ROLLBACK | FINALIZE) {
        return Err(DeployError(format!(
            "action must be {RUN}, {RESUME}, {STATUS}, {ROLLBACK}, or {FINALIZE}"
        )));
    }
    let runner_gate = if matches!(phase, RUN | RESUME) {
        repository_runner_gate().await?
    } else {
        None
    };
    if runner_gate.as_ref().is_some_and(|gate| {
        gate.get("source_sha").and_then(Value::as_str)
            != Some(crate::build_identity::SOURCE_REVISION)
    }) {
        return Err(DeployError(
            "current GitHub job source differs from the transaction tool source".to_string(),
        ));
    }
    let executable = std::env::current_exe()
        .map_err(|error| DeployError(format!("cannot locate transaction tool: {error}")))?;
    let tool_bytes = std::fs::read(&executable)
        .map_err(|error| DeployError(format!("cannot read transaction tool: {error}")))?;
    let tool_sha256 = hex::encode(Sha256::digest(&tool_bytes));
    let work = format!("$HOME/.stado/recovery/storage-root-reconcile/{transaction}");
    let staged_tool = format!("{work}/transaction-tool.{tool_sha256}");
    let canonical_tool = format!("{work}/transaction-tool");
    let staged =
        service::sync_service_file(&target, &staged_tool, &tool_bytes, 0o700, runner).await?;
    if !staged.succeeded("file_synced") {
        return Err(DeployError(format!(
            "transaction tool staging failed: {}",
            staged.failure()
        )));
    }
    let runner_gate = runner_gate
        .map(|gate| serde_json::to_vec(&gate))
        .transpose()
        .map_err(|error| DeployError(format!("cannot encode runner gate: {error}")))?
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
        .unwrap_or_default();
    let target_config = base64::engine::general_purpose::STANDARD.encode(
        serde_json::to_vec(&target)
            .map_err(|error| DeployError(format!("cannot encode resident target: {error}")))?,
    );
    let arguments = vec![
        "host".to_string(),
        "storage-root-reconcile-worker".to_string(),
        target_name.to_string(),
        "--target-config".to_string(),
        target_config,
        "--transaction".to_string(),
        transaction.to_string(),
        "--phase".to_string(),
        phase.to_string(),
        "--source-revision".to_string(),
        crate::build_identity::SOURCE_REVISION.to_string(),
        "--tool-sha256".to_string(),
        tool_sha256.clone(),
        "--runner-gate".to_string(),
        runner_gate,
    ];
    let launched = host_channel::run_script(
        &target,
        &launch_worker_script(
            transaction,
            &staged_tool,
            &canonical_tool,
            &tool_sha256,
            &arguments,
        )?,
        runner,
    )
    .await?;
    if !launched.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &launched,
            "resident reconciliation worker did not launch",
        )));
    }
    let owner = launched
        .stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("STADO_RECONCILE_OWNER\t")
                .and_then(|value| serde_json::from_str::<Value>(value).ok())
        })
        .ok_or_else(|| DeployError("resident reconciliation worker reported no owner".to_string()))?;
    let mut report = host_channel::base_report(&target);
    report.insert("transaction".to_string(), json!(transaction));
    report.insert("phase".to_string(), json!(phase));
    report.insert("status".to_string(), json!("accepted"));
    report.insert("operation_owner".to_string(), owner);
    Ok(Value::Object(report))
}
