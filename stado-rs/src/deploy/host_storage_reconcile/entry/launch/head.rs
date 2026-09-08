pub(super) const LAUNCH_WORKER_SCRIPT_HEAD: &str = r##"set -euo pipefail
STADO_WORKER_ARGS=@ARGS@ STADO_STAGED_TOOL=@STAGED@ STADO_CANONICAL_TOOL=@TOOL@ STADO_TOOL_SHA256=@SHA@ STADO_TRANSACTION=@TX@ /usr/bin/python3 - <<'PY'
import base64, fcntl, hashlib, json, os, platform, plistlib, re, shlex, stat, subprocess, time
tx = os.environ["STADO_TRANSACTION"]
staged = os.path.expanduser(os.path.expandvars(os.environ["STADO_STAGED_TOOL"]))
tool = os.path.expanduser(os.path.expandvars(os.environ["STADO_CANONICAL_TOOL"]))
expected = os.environ["STADO_TOOL_SHA256"]
work = os.path.dirname(tool)
home = os.path.expanduser("~")
owner_path = os.path.join(work, "operation-owner.json")
intent_path = os.path.join(work, "launch-intent.json")
label = "com.wisent.stado-storage-root-reconcile." + tx
log_path = os.path.join(work, "transaction-worker.log")
system = platform.system()
os.makedirs(work, mode=0o700, exist_ok=True)
arguments = json.loads(base64.b64decode(os.environ["STADO_WORKER_ARGS"]))
argv = [tool] + arguments


def argument(name):
    try:
        return arguments[arguments.index(name) + 1]
    except (ValueError, IndexError):
        raise SystemExit("native worker arguments omit " + name)


captured_target = json.loads(base64.b64decode(argument("--target-config")))
requested_action = argument("--phase")
requested_revision = argument("--source-revision")

def exact_option(values, name):
    found = [
        values[index + 1]
        for index, value in enumerate(values[:-1])
        if value == name
    ]
    if len(found) != 1 or not isinstance(found[0], str):
        raise SystemExit("captured object API command must declare exactly one " + name)
    return found[0]


def native_object_arguments(service):
    if system == "Darwin":
        path = service.get("path")
        if not isinstance(path, str) or not path:
            raise SystemExit("captured object API has no native unit path")
        path = os.path.expanduser(path.replace("$HOME", home))
        with open(path, "rb") as handle:
            unit = plistlib.load(handle)
        values = unit.get("ProgramArguments")
        if not isinstance(values, list) or not values:
            raise SystemExit("captured object API unit has no ProgramArguments")
        return values[1:]
    unit = service.get("unit") or service.get("label")
    result = checked(["/bin/systemctl", "show", unit, "--property=ExecStart", "--value"])
    commands = re.findall(r"argv\[\] = (.*?); (?:ignore_errors|flags)=", result.stdout)
    if len(commands) != 1:
        raise SystemExit("captured object API has no single observed ExecStart")
    return shlex.split(commands[0])[1:]


def captured_release_api(target):
    fence = read_json(os.path.join(work, "lifecycle-fence.json"))
    if fence is not None:
        if (not isinstance(fence, dict)
                or fence.get("schema") != "@FENCE_SCHEMA@"
                or fence.get("transaction") != tx):
            raise SystemExit("captured lifecycle fence has the wrong transaction identity")
        staged_runtime = fence.get("staged_runtime")
        if staged_runtime is not None:
            request = staged_runtime.get("request", {})
            origin = request.get("release_api")
            if not isinstance(origin, str) or not origin:
                raise SystemExit("captured staged runtime has no release origin")
            return origin
    services = target.get("services")
    if not isinstance(services, list):
        raise SystemExit("captured target declares no service inventory")
    object_apis = [
        service for service in services
        if isinstance(service, dict)
        and service.get("label") == "com.wisent.always-on.stado-object-api"
    ]
    if len(object_apis) != 1:
        raise SystemExit("captured target must declare exactly one canonical object API")
    values = object_apis[0].get("args")
    if values is None or values == []:
        values = native_object_arguments(object_apis[0])
    if (not isinstance(values, list)
            or not all(isinstance(value, str) for value in values)
            or not values
            or values[0] != "dashboard"):
        raise SystemExit("captured object API command is not the dashboard")
    bind = exact_option(values, "--bind")
    port_text = exact_option(values, "--port")
    try:
        port = int(port_text)
    except ValueError:
        raise SystemExit("captured object API port is not numeric")
    if port < 1 or port > 65535:
        raise SystemExit("captured object API port is outside 1..65535")
    if bind == "::1":
        host = "[::1]"
    elif bind in ("127.0.0.1", "localhost"):
        host = bind
    else:
        raise SystemExit("captured object API release origin is not loopback")
    return "http://" + host + ":" + str(port)


def checked(argv, accepted=(0,)):
    result = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, close_fds=True)
    if result.returncode not in accepted:
        detail = (result.stderr or result.stdout).strip().splitlines()
        raise SystemExit(detail[-1] if detail else "native service command failed")
    return result


def atomic_json(path, value):
    temporary = path + "." + str(os.getpid()) + ".new"
    with open(temporary, "x", encoding="utf-8") as handle:
        json.dump(value, handle, sort_keys=True, separators=(",", ":"))
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)
    descriptor = os.open(os.path.dirname(path), os.O_RDONLY)
    os.fsync(descriptor)
    os.close(descriptor)


def read_json(path):
    try:
        info = os.lstat(path)
        if not stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
            raise SystemExit(path + " is not a regular file")
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except FileNotFoundError:
        return None


def manager_state():
    if system == "Darwin":
        result = subprocess.run(
            ["/usr/bin/sudo", "-n", "/bin/launchctl", "print", "system/" + label],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, close_fds=True)
        if result.returncode != 0:
            return {"manager": "launchd", "service": label, "domain": "system",
                    "loaded": False, "active": False, "starting": False, "pid": None,
                    "state": None}
        pid_match = re.search(r"(?m)^\s*pid = ([1-9][0-9]*)\s*$", result.stdout)
        state_match = re.search(r"(?m)^\s*state = (.+?)\s*$", result.stdout)
        pid = int(pid_match.group(1)) if pid_match else None
        completed = re.search(r"(?m)^\s*last exit code = -?[0-9]+\s*$", result.stdout)
        runs = re.search(r"(?m)^\s*runs = ([1-9][0-9]*)\s*$", result.stdout)
        state = state_match.group(1).strip() if state_match else None
        terminal = ((state or "").lower() in ("exited", "not running")
                    or (pid is None and completed is not None and runs is not None))
        return {"manager": "launchd", "service": label, "domain": "system",
                "loaded": True, "active": pid is not None,
                "starting": pid is None and not terminal, "pid": pid, "state": state}
    if system == "Linux":
        unit = label + ".service"
        result = subprocess.run(
            ["/usr/bin/sudo", "-n", "/bin/systemctl", "show",
             "--property=LoadState,ActiveState,SubState,MainPID", unit],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, close_fds=True)
        properties = {}
        if result.returncode == 0:
            for line in result.stdout.splitlines():
                if "=" in line:
                    key, value = line.split("=", 1)
                    properties[key] = value
        value = properties.get("MainPID", "")
        pid = int(value) if value.isdigit() and int(value) > 0 else None
        active_state = properties.get("ActiveState")
        active = pid is not None or active_state in ("active", "activating", "reloading")
        return {"manager": "systemd", "service": unit,
                "loaded": properties.get("LoadState") == "loaded",
                "active": active, "starting": active_state == "activating",
                "pid": pid, "load_state": properties.get("LoadState"),
                "active_state": active_state, "sub_state": properties.get("SubState")}
    raise SystemExit("native reconciliation worker requires Darwin launchd or Linux systemd")
"##;
