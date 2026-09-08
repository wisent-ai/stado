import hashlib, os, stat, sys, time
backup = os.environ["STADO_BACKUP_ROOT"]
primary = os.environ["STADO_PRIMARY_ROOT"]
namespace = os.environ["STADO_NAMESPACE"]
deadline = time.monotonic() + float(os.environ["STADO_HASH_DEADLINE"])
reclaim = os.environ["STADO_RECLAIM"] == "yes"
apply = os.environ["STADO_APPLY"] == "yes"
selected = [
    bytes.fromhex(value).decode("utf-8")
    for value in os.environ["STADO_OBJECTS_HEX"].split(",")
    if value
]
inventory_namespaces = [
    bytes.fromhex(value).decode("utf-8")
    for value in os.environ["STADO_INVENTORY_NAMESPACES_HEX"].split(",")
    if value
]



def digest(path, stop_at=None):
    h = hashlib.sha256()
    with open(path, "rb") as handle:
        while True:
            if stop_at is not None and time.monotonic() >= stop_at:
                return None
            chunk = handle.read(1024 * 1024)
            if not chunk:
                return h.hexdigest()
            h.update(chunk)

out = sys.stdout
def identity(path):
    try:
        entry = os.lstat(path)
    except FileNotFoundError:
        return ("absent", "", "")
    except OSError:
        return ("unreadable", "", "")
    if not stat.S_ISREG(entry.st_mode):
        return ("not_regular", str(entry.st_size), "")
    try:
        value = digest(path, deadline)
        if value is None:
            return ("deadline_unproven", str(entry.st_size), "")
        return ("present", str(entry.st_size), value)
    except OSError:
        return ("unreadable", str(entry.st_size), "")

def emit_namespaces(label, root):
    ecosystem = os.path.join(root, "ecosystem")
    names = []
    if time.monotonic() >= deadline:
        out.write("STADO_BACKUP_NAMESPACES_ERROR\t%s\tdeadline exhausted\n" % label)
        return
    try:
        with os.scandir(ecosystem) as entries:
            names = sorted(
                entry.name
                for entry in entries
                if entry.is_dir(follow_symlinks=False)
            )
    except OSError as error:
        out.write(
            "STADO_BACKUP_NAMESPACES_ERROR\t%s\t%s\n"
            % (label, str(error).replace("\t", " ").replace("\n", " "))
        )
        return
    if time.monotonic() >= deadline:
        out.write("STADO_BACKUP_NAMESPACES_ERROR\t%s\tdeadline exhausted\n" % label)
        return
    for name in names:
        out.write(
            "STADO_BACKUP_NAMESPACE\t%s\t%s\n"
            % (label, name.encode("utf-8").hex())
        )
    out.write("STADO_BACKUP_NAMESPACES_END\t%s\t%d\n" % (label, len(names)))

emit_namespaces("local_storage", primary)
emit_namespaces("local_backup", backup)

def metadata_path(root, relative):
    name = relative if relative.endswith(".json") else relative + ".json"
    return os.path.join(root, ".metadata", name)

def stat_identity(path):
    try:
        entry = os.lstat(path)
    except FileNotFoundError:
        return ("absent", "", "")
    except OSError:
        return ("unreadable", "", "")
    if not stat.S_ISREG(entry.st_mode):
        return ("not_regular", str(entry.st_size), "")
    return ("present", str(entry.st_size), "")

inventory_complete = True
def inventory_error(scope, detail):
    global inventory_complete
    inventory_complete = False
    out.write(
        "STADO_BACKUP_AUDIT_UNAVAILABLE\t%s inventory: %s\n"
        % (scope, str(detail).replace("\t", " ").replace("\n", " "))
    )

inventory_timed_out = False
for scope in inventory_namespaces:
    if time.monotonic() >= deadline:
        inventory_error(scope, "deadline exhausted before namespace enumeration")
        inventory_timed_out = True
        break
    backup_scope = os.path.join(backup, "ecosystem", scope)
    try:
        scope_entry = os.lstat(backup_scope)
    except OSError as error:
        inventory_error(scope, error)
        continue
    if not stat.S_ISDIR(scope_entry.st_mode):
        inventory_error(scope, "backup namespace root is not a directory")
        continue
    walk_errors = []
    def walk_error(error):
        walk_errors.append(error)
    for root, dirs, files in os.walk(
        backup_scope,
        followlinks=False,
        onerror=walk_error,
    ):
        if time.monotonic() >= deadline:
            inventory_error(scope, "deadline exhausted during namespace enumeration")
            inventory_timed_out = True
            break
        retained_dirs = []
        for name in sorted(dirs):
            directory = os.path.join(root, name)
            if os.path.islink(directory):
                inventory_error(scope, "non-directory entry omitted: " + directory)
            else:
                retained_dirs.append(name)
        dirs[:] = retained_dirs
        for name in sorted(files):
            if time.monotonic() >= deadline:
                inventory_error(scope, "deadline exhausted during namespace enumeration")
                inventory_timed_out = True
                break
            path = os.path.join(root, name)
            relative = os.path.relpath(path, backup)
            b_state, b_size, _ = stat_identity(path)
            p_state, p_size, _ = stat_identity(os.path.join(primary, relative))
            p_meta_state, p_meta_size, _ = stat_identity(metadata_path(primary, relative))
            b_meta_state, b_meta_size, _ = stat_identity(metadata_path(backup, relative))
            out.write(
                "STADO_BACKUP_INVENTORY_OBJECT\t%s\t%s\t%s\t\t%s\t%s\t\t%s\t%s\t\t%s\t%s\t\n"
                % (
                    relative.encode("utf-8").hex(),
                    p_state,
                    p_size,
                    b_state,
                    b_size,
                    p_meta_state,
                    p_meta_size,
                    b_meta_state,
                    b_meta_size,
                )
            )
            if b_state != "present":
                inventory_error(scope, "backup object is " + b_state + ": " + relative)
            for label, state in (
                ("local-storage object", p_state),
                ("local-storage metadata", p_meta_state),
                ("local-backup metadata", b_meta_state),
            ):
                if state == "unreadable":
                    inventory_error(scope, label + " is unreadable: " + relative)
        if inventory_timed_out:
            break
    for error in walk_errors:
        inventory_error(scope, error)
    if inventory_timed_out:
        break

if inventory_namespaces and not selected:
    if inventory_complete:
        out.write("STADO_BACKUP_AUDIT_END\tinventory\n")
    sys.exit(0)


if selected:
    for relative in selected:
        normalized = os.path.normpath(relative)
        if os.path.isabs(relative) or normalized != relative or normalized.startswith("../"):
            out.write("STADO_BACKUP_AUDIT_UNAVAILABLE\tinvalid exact object path\n")
            continue
        p_state, p_size, p_digest = identity(os.path.join(primary, relative))
        b_state, b_size, b_digest = identity(os.path.join(backup, relative))
        p_meta_state, p_meta_size, p_meta_digest = identity(metadata_path(primary, relative))
        b_meta_state, b_meta_size, b_meta_digest = identity(metadata_path(backup, relative))
        out.write(
            "STADO_BACKUP_OBJECT\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n"
            % (
                relative,
                p_state,
                p_size,
                p_digest,
                b_state,
                b_size,
                b_digest,
                p_meta_state,
                p_meta_size,
                p_meta_digest,
                b_meta_state,
                b_meta_size,
                b_meta_digest,
            )
        )
    if inventory_complete:
        out.write("STADO_BACKUP_AUDIT_END\texact\n")
    sys.exit(0)
deleted = 0
deleted_bytes = 0
refused = 0
for root, _, files in os.walk(backup):
    for name in files:
        path = os.path.join(root, name)
        relative = os.path.relpath(path, backup)
        if relative.startswith("ecosystem/"):
            candidate = os.path.join(primary, relative)
        else:
            candidate = os.path.join(primary, "ecosystem", namespace, relative)
        try:
            entry = os.lstat(path)
        except OSError:
            continue
        size = entry.st_size
        try:
            other = os.lstat(candidate)
        except OSError:
            out.write("STADO_BACKUP_AUDIT\tabsent\t%d\t%s\n" % (size, relative))
            continue
        # Only a plain file on BOTH sides can be a twin. A symlink, a socket or
        # a directory that happens to match a size is not the object, and the
        # one thing this pass may never do is unlink something whose primary
        # counterpart it did not actually read.
        if not stat.S_ISREG(entry.st_mode) or not stat.S_ISREG(other.st_mode):
            out.write("STADO_BACKUP_AUDIT\tdiffers\t%d\t%s\n" % (size, relative))
            continue
        if other.st_size != size:
            out.write("STADO_BACKUP_AUDIT\tdiffers\t%d\t%s\n" % (size, relative))
            continue
        if time.monotonic() >= deadline:
            out.write("STADO_BACKUP_AUDIT\tsame_size_unproven\t%d\t%s\n" % (size, relative))
            continue
        try:
            same = digest(path) == digest(candidate)
        except OSError:
            out.write("STADO_BACKUP_AUDIT\tsame_size_unproven\t%d\t%s\n" % (size, relative))
            continue
        if not same:
            out.write("STADO_BACKUP_AUDIT\tdiffers\t%d\t%s\n" % (size, relative))
            continue
        out.write("STADO_BACKUP_AUDIT\ttwin\t%d\t%s\n" % (size, relative))
        # The proof and the deletion are the same event. Nothing here reads a
        # verdict recorded by an earlier run: the two hashes above were computed
        # from these two files moments ago, and only that proves this unlink.
        if not reclaim:
            continue
        if not apply:
            out.write("STADO_BACKUP_RECLAIM\twould_delete\t%d\t%s\n" % (size, relative))
            continue
        try:
            os.remove(path)
        except OSError:
            refused += 1
            out.write("STADO_BACKUP_RECLAIM\tdelete_failed\t%d\t%s\n" % (size, relative))
            continue
        deleted += 1
        deleted_bytes += size
        out.write("STADO_BACKUP_RECLAIM\tdeleted\t%d\t%s\n" % (size, relative))
out.write(
    "STADO_BACKUP_RECLAIM_END\t%d\t%d\t%d\n" % (deleted, deleted_bytes, refused)
)
out.write("STADO_BACKUP_AUDIT_END\tclassified\n")
