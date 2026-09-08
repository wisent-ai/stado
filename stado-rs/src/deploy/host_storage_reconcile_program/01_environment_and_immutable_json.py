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


