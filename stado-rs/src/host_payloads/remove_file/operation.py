"""The fixed host-side implementation of Stado's guarded single-file removal.

Like retirement, it holds directory descriptors and never follows a path
component. The caller supplies a path and the target account's home, not code.
"""
import errno
import json
import os
import stat
import sys


def report(status, detail=""):
    print("STADO_REMOVE_FILE\t" + json.dumps([status, detail]), flush=True)
    raise SystemExit(0)


def remove_file(path, home):
    user_roots = (
        os.path.join(home, "Library", "LaunchAgents"),
        os.path.join(home, ".stado"),
        os.path.join(home, ".config", "systemd", "user"),
    )
    user_owned = any(path.startswith(root + "/") for root in user_roots)
    parent, name = os.path.split(path)
    privileged = name.startswith("com.wisent.") and (
        (parent == "/Library/LaunchDaemons" and name.endswith(".plist"))
        or (parent == "/etc/systemd/system" and name.endswith(".service"))
    )
    if not user_owned and not privileged:
        report("refused", "outside the managed areas")
    if privileged and os.geteuid() != 0:
        report("refused", "the declared privileged file operation requires the target's sudo authority")

    descriptor = None
    operation = "open parent directory without following links"
    try:
        flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
        descriptor = os.open("/", flags)
        for component in (piece for piece in parent.split("/") if piece):
            following = os.open(component, flags, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = following
        operation = "inspect the selected file without following links"
        before = os.stat(name, dir_fd=descriptor, follow_symlinks=False)
        if stat.S_ISLNK(before.st_mode):
            report("refused", "a symbolic link is not removed by the managed regular-file operation")
        if not stat.S_ISREG(before.st_mode):
            report("refused", "not a regular file")
        if user_owned and before.st_uid != os.geteuid():
            report("refused", "not owned by this account")
        operation = "unlink the selected file through its held parent directory"
        os.unlink(name, dir_fd=descriptor)
        operation = "verify file absence through the held parent directory"
        try:
            os.stat(name, dir_fd=descriptor, follow_symlinks=False)
        except FileNotFoundError:
            report("removed")
        report("failed", "the selected file was recreated after removal")
    except FileNotFoundError:
        report("absent")
    except OSError as error:
        if error.errno in (errno.ELOOP, errno.ENOTDIR):
            report("refused", f"{operation}: a parent is a symbolic link or is not a directory: {error}")
        report("failed", f"{operation}: {error}")
    finally:
        if descriptor is not None:
            os.close(descriptor)


if __name__ == "__main__":
    remove_file(*sys.argv[1:])
