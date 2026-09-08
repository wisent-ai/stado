pub(super) const LAUNCH_WORKER_SCRIPT_TAIL: &str = r##"
def manager_bound_owner(state):
    owner = read_json(owner_path)
    if not isinstance(owner, dict):
        return None
    native = owner.get("native_manager")
    if (owner.get("schema") != "stado.storage-root-owner.v1"
            or owner.get("transaction") != tx
            or owner.get("status") != "executing"
            or not isinstance(native, dict)
            or native.get("service") != state.get("service")
            or int(owner.get("pid", 0)) != state.get("pid")
            or native.get("pid") != state.get("pid")):
        return None
    owner.pop("token", None)
    return owner


def launch_observation(state):
    intent = read_json(intent_path)
    if not isinstance(intent, dict) or intent.get("transaction") != tx:
        raise SystemExit("active native worker has no recorded launch intent")
    intent["native_manager"] = state
    intent.pop("worker_arguments", None)
    return intent


def acknowledge_owner(observation):
    action = observation.get("action")
    forward = ("run", "resume")
    if action != requested_action and not (action in forward and requested_action in forward):
        raise SystemExit("native reconciliation is already executing "
                         + str(action) + "; cannot accept " + requested_action)
    print("STADO_RECONCILE_OWNER\t" + json.dumps(
        observation, sort_keys=True, separators=(",", ":")))


launch_lock_path = os.path.join(
    os.path.dirname(work), "..", "storage-root-reconcile.launch.lock")
launch_lock_path = os.path.normpath(launch_lock_path)
flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
launch_lock = os.open(launch_lock_path, flags, 0o600)
fcntl.flock(launch_lock, fcntl.LOCK_EX)

state = manager_state()
if state["active"] or state["starting"]:
    owner = manager_bound_owner(state)
    observation = owner if owner is not None else launch_observation(state)
    acknowledge_owner(observation)
    raise SystemExit(0)

operation_lock_path = os.path.normpath(os.path.join(
    os.path.dirname(work), "..", "storage-root-reconcile.lock"))
operation_lock = os.open(operation_lock_path, flags, 0o600)
try:
    fcntl.flock(operation_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:
    state = manager_state()
    if state["active"] or state["starting"]:
        owner = manager_bound_owner(state)
        observation = owner if owner is not None else launch_observation(state)
        acknowledge_owner(observation)
        raise SystemExit(0)
    raise SystemExit("native reconciliation lock is held without a manager-bound owner")

# Manager visibility and the operation lock are one observation. A unit may
# enter activating/running after the first manager read but before this
# launcher wins the lock; that transition still forbids replacement.
state = manager_state()
if state["active"] or state["starting"]:
    fcntl.flock(operation_lock, fcntl.LOCK_UN)
    os.close(operation_lock)
    owner = manager_bound_owner(state)
    observation = owner if owner is not None else launch_observation(state)
    acknowledge_owner(observation)
    raise SystemExit(0)

lock_info = os.fstat(operation_lock)
release_api = captured_release_api(captured_target)
intent = {
    "schema": "stado.storage-root-launch.v1",
    "transaction": tx,
    "target": captured_target.get("name"),
    "target_config": captured_target,
    "action": requested_action,
    "status": "launch_intent",
    "source_revision": requested_revision,
    "tool_sha256": expected,
    "release_api": release_api,
    "native_manager": state,
    "lock_device": lock_info.st_dev,
    "lock_inode": lock_info.st_ino,
}
atomic_json(intent_path, intent)

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
            "STADO_API_URL": release_api,
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
    checked(["/usr/bin/sudo", "-n", "/bin/launchctl", "bootout", "system/" + label],
            accepted=(0, 3, 113))
    checked(["/usr/bin/sudo", "-n", "/usr/bin/install", "-m", "644",
             "-o", "root", "-g", "wheel", prepared, unit_path])
    checked(["/usr/bin/sudo", "-n", "/bin/launchctl", "enable", "system/" + label])
    fcntl.flock(operation_lock, fcntl.LOCK_UN)
    os.close(operation_lock)
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
        "Environment=STADO_API_URL=" + release_api,
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
    fcntl.flock(operation_lock, fcntl.LOCK_UN)
    os.close(operation_lock)
    checked(["/usr/bin/sudo", "-n", "/bin/systemctl", "start", label + ".service"])

deadline = time.monotonic() + 30
while time.monotonic() < deadline:
    state = manager_state()
    owner = manager_bound_owner(state)
    if owner is not None:
        intent["status"] = "worker_adopted"
        intent["native_manager"] = state
        atomic_json(intent_path, intent)
        print("STADO_RECONCILE_OWNER\t" + json.dumps(
            owner, sort_keys=True, separators=(",", ":")))
        raise SystemExit(0)
    if not state["active"] and not state["starting"]:
        break
    time.sleep(0.1)
raise SystemExit("native reconciliation worker did not record manager-bound ownership")
PY"##;
