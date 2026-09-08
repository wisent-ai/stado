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



