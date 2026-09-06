import ctypes, datetime, errno, fcntl, hashlib, json, os, stat, subprocess, sys, time

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
checkpoint_evidence_path = os.path.join(work, "checkpoint-evidence.json")
lifecycle_decisions_path = os.path.join(work, "lifecycle-decisions.json")
final_lifecycle_observations_path = os.path.join(
    work, "final-lifecycle-observations.json")
lock_path = os.path.join(home, ".stado", "recovery", "storage-root-reconcile.lock")
schema = "stado.storage-root-reconcile.v2"
staging = os.path.join(work, ".clone-staging")
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


def path_within(path, roots):
    candidate = os.path.abspath(path)
    return any(
        candidate != os.path.abspath(root)
        and os.path.commonpath((candidate, os.path.abspath(root))) == os.path.abspath(root)
        for root in roots
    )


def confined_path(path, roots, label):
    candidate = os.path.abspath(path)
    if path_within(candidate, roots):
        return candidate
    fail(label + " escaped its captured transaction roots")


def noninteractive_privileged(arguments, label):
    inherited_lock = globals().get("lock_fd", -1)
    if inherited_lock < 0:
        fail(label + ": resident transaction lock descriptor is unavailable")
    result = subprocess.run(
        ["/usr/bin/sudo", "-n"] + arguments,
        stdin=inherited_lock,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        close_fds=True,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip().splitlines()
        fail(label + ": " + (detail[-1] if detail else "privileged command failed"))
    return result


def privileged_digest(path):
    if path_within(path, (primary, backup)):
        source = confined_path(
            path, (primary, backup), "privileged physical-root digest")
        label = "cannot hash unreadable physical-root file"
    elif path_within(path, (staging,)):
        source = confined_path(
            path, (staging,), "privileged transaction-staging digest")
        label = "cannot hash interrupted privileged clone"
    else:
        fail("privileged digest escaped the live roots and transaction staging")
    result = noninteractive_privileged(
        ["/usr/bin/openssl", "dgst", "-sha256", "-r", source],
        label,
    )
    encoded = result.stdout.strip().split(None, 1)
    if (not encoded
            or len(encoded[0]) != 64
            or any(character not in "0123456789abcdef" for character in encoded[0])):
        fail("privileged confined digest has invalid output")
    return encoded[0]


def recover_privileged_clone(destination):
    destination = confined_path(
        destination, (staging,), "privileged clone recovery destination")
    info = os.lstat(destination)
    if not stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
        fail("privileged clone recovery found a non-regular staging entry")
    noninteractive_privileged(
        ["/usr/bin/chflags", "nouchg,noschg", destination],
        "cannot clear immutable flags on privileged clone",
    )
    noninteractive_privileged(
        ["/usr/sbin/chown", str(os.getuid()) + ":" + str(os.getgid()), destination],
        "cannot transfer privileged clone ownership",
    )


def privileged_clone(source, destination):
    source = confined_path(
        source, (primary, backup), "privileged copy-on-write clone source")
    destination = confined_path(
        destination, (staging,), "privileged copy-on-write clone destination")
    noninteractive_privileged(
        ["/bin/cp", "-c", "-p", source, destination],
        "privileged copy-on-write clone failed",
    )
    recover_privileged_clone(destination)


def digest(path):
    value = hashlib.sha256()
    try:
        with open(path, "rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                value.update(chunk)
        return value.hexdigest()
    except PermissionError:
        return privileged_digest(path)


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


def immutable_json_file(path, label):
    try:
        info = os.lstat(path)
        if not stat.S_ISREG(info.st_mode):
            fail(label + " is not a regular file: " + path)
        with open(path, "rb") as handle:
            encoded = handle.read()
        value = json.loads(encoded)
    except Exception as error:
        fail(label + " is absent or invalid: " + str(error))
    reference = {
        "path": path,
        "sha256": hashlib.sha256(encoded).hexdigest(),
        "bytes": len(encoded),
    }
    return value, reference


def load_immutable_json(reference, path, label):
    if not isinstance(reference, dict) or reference.get("path") != path:
        fail(label + " reference does not name its canonical transaction file")
    value, observed = immutable_json_file(path, label)
    if observed != reference:
        fail(label + " bytes differ from their durable reference")
    return value


def persist_immutable_json(path, value, label):
    encoded = (json.dumps(
        value, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
    if os.path.lexists(path):
        try:
            info = os.lstat(path)
            if not stat.S_ISREG(info.st_mode):
                fail(label + " collides with a non-regular file: " + path)
            with open(path, "rb") as handle:
                existing = handle.read()
        except Exception as error:
            fail("cannot inspect " + label + ": " + str(error))
        if existing != encoded:
            fail(label + " changed after its immutable publication")
    else:
        atomic_json(path, value)
    _, reference = immutable_json_file(path, label)
    return reference


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
        temporary_identity = regular_identity(temporary)
        temporary_info = os.lstat(temporary)
        temporary_flags = getattr(temporary_info, "st_flags", 0)
        immutable = (
            getattr(stat, "UF_IMMUTABLE", 0)
            | getattr(stat, "SF_IMMUTABLE", 0)
        )
        if (temporary_info.st_uid != os.getuid()
                or temporary_flags & immutable):
            recover_privileged_clone(temporary)
        if temporary_identity != regular_identity(source):
            os.unlink(temporary)
    if not os.path.exists(temporary):
        libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
        clone = libc.clonefile
        clone.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_int]
        clone.restype = ctypes.c_int
        if clone(os.fsencode(source), os.fsencode(temporary), 0) != 0:
            error = ctypes.get_errno()
            if error not in (errno.EACCES, errno.EPERM):
                fail("clonefile refused copy-on-write clone: " + os.strerror(error))
            if os.path.lexists(temporary):
                fail("clonefile left a partial privileged clone destination")
            privileged_clone(source, temporary)
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
if phase not in ("read-fence", "read-owner", "status"):
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
        fence = {"schema": "stado.storage-root-fence.v5", "transaction": tx,
                 "status": "absent", "writers": []}
    print("STADO_STORAGE_RECONCILE\t" +
          json.dumps(fence, sort_keys=True, separators=(",", ":")))
    raise SystemExit(0)

if phase == "read-owner":
    try:
        info = os.lstat(owner_path)
        if not stat.S_ISREG(info.st_mode):
            fail("operation owner is not a regular file")
        with open(owner_path, encoding="utf-8") as handle:
            owner = json.load(handle)
    except FileNotFoundError:
        print("STADO_RECONCILE_OWNER\tabsent")
        raise SystemExit(0)
    if (owner.get("schema") != "stado.storage-root-owner.v1"
            or owner.get("transaction") != tx):
        fail("operation owner identity is invalid")
    owner.pop("token", None)
    print("STADO_RECONCILE_OWNER\t" +
          json.dumps(owner, sort_keys=True, separators=(",", ":")))
    raise SystemExit(0)



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
            or evidence.get("destination") != primary):
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
    return backup_objects, primary_objects, backup_physical, primary_physical


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
    receipt = load_receipt() if os.path.exists(receipt_path) else None
    if receipt is not None and receipt.get("status") in (
        "checkpoint_ready", "applying", "data_committed_pending_activation",
        "activation_effects_armed", "rollback_effects_armed",
        "activated_pending_lifecycle", "complete"
    ):
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
        backup_objects, primary_objects, backup_physical, primary_physical = (
            load_checkpoint_evidence(receipt))
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
    checkpoint_effective_lifecycle(primary_objects, backup_objects)
    receipt["status"] = "checkpoint_ready"
    receipt["checkpointed_at"] = time.time()
    atomic_json(receipt_path, receipt)
    emit(receipt)
    raise SystemExit(0)

receipt = load_receipt()
if receipt.get("status") == "complete" and phase != "finalize":
    emit(receipt)
    raise SystemExit(0)
backup_objects, primary_objects, backup_physical, primary_physical = (
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


def primary_is_winner():
    try:
        with open(fence_path, "r", encoding="utf-8") as handle:
            fence = json.load(handle)
    except Exception as error:
        fail("additive-union direction cannot be read: " + str(error))
    roots = fence.get("roots") or {}
    prior_primary = roots.get("prior_primary")
    if (fence.get("schema") != "stado.storage-root-fence.v5"
            or fence.get("transaction") != tx
            or prior_primary not in (primary, backup)):
        fail("additive-union direction is invalid")
    return prior_primary == primary


def prove_live_additive_union(label):
    backup_by_path = {item["path"]: item for item in backup_objects}
    primary_by_path = {item["path"]: item for item in primary_objects}
    primary_wins = primary_is_winner()
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
    if (fence.get("status") != "fenced"
            or not fence.get("queue", {}).get("drained")
            or any(item.get("status") != "stopped" for item in fence.get("writers", []))):
        fail("rollback effects require every writer to remain stopped")
    validate_complete_inventory(backup, backup_objects, "live B before rollback")
    validate_physical_checkpoint(backup, backup_physical, "live physical B before rollback")
    if (fence.get("roots") or {}).get("prior_primary") == primary:
        validate_complete_inventory(primary, primary_objects, "live A before rollback")
        validate_physical_checkpoint(primary, primary_physical, "live physical A before rollback")
    receipt["status"] = "rollback_effects_armed"
    receipt["rollback_effect_boundary_at"] = time.time()
    atomic_json(receipt_path, receipt)
    emit(receipt)
    raise SystemExit(0)


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
        fail("A-only lifecycle state blocks activation: " +
             ", ".join(item.get("path", "?") for item in blockers))
    backup_by_path = {item["path"]: item for item in backup_objects}
    primary_by_path = {item["path"]: item for item in primary_objects}
    primary_wins = primary_is_winner()
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
    receipt["conflict_winner"] = "primary" if primary_wins else "backup"
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
