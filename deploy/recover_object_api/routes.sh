
# `/healthz` is a startup snapshot. The object route revalidates its Skarbiec
# grant per request, so that snapshot can remain true while every protected
# read returns 503. Prove the boundary with the host's existing owner-only
# queue client bearer, passed to curl on stdin so it never appears in argv,
# then require the operator state to carry no object-boundary error.
authenticated_object_ready() {
  [ -r "$object_token_file" ] || return 1
  token=$(/bin/cat "$object_token_file")
  [ -n "$token" ] || return 1
  response="$work/protected-object.json"
  code=$(
    printf 'header = "Authorization: Bearer %s"\n' "$token" |
      /usr/bin/curl --config - --silent --show-error --max-time 5 \
        --output "$response" --write-out '%{http_code}' \
        "${object_url%/}/api/object?uri=stado%3A%2F%2F${object_namespace}%2Fregistry.json"
  ) || return 1
  [ "$code" = 200 ] || return 1
  state="$work/object-state.json"
  /usr/bin/curl --silent --show-error --fail --max-time 5 \
    "${object_url%/}/api/state.json" > "$state" || return 1
  /usr/bin/python3 - "$state" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
boundary = ((document.get("boundaries") or {}).get("object") or {})
raise SystemExit(0 if boundary.get("ready") is True and not boundary.get("last_error") else 1)
PY
}

# Resolve the route a loaded job was given, rather than the route in the file
# launchd may read next time. The legacy server selected the configured backup
# whenever its client profile selected `stado`; that promotion is made explicit
# here so a healthy read from local-backup cannot certify local-storage.
inspect_route() {
  mode=$1
  source=$2
  /usr/bin/python3 - "$mode" "$source" "$config" "$HOME" "$staged" \
    "$work/$label.runtime-state.json" <<'PY'
import json, os, plistlib, re, sys

mode, source, default_config, default_home, expected_path, runtime_path = sys.argv[1:]
with open(expected_path, "rb") as handle:
    expected = plistlib.load(handle).get("EnvironmentVariables") or {}

state = "-"
pid = "-"
if mode == "launchctl":
    with open(source, encoding="utf-8", errors="strict") as handle:
        lines = handle.readlines()
    environment = {}
    in_environment = False
    for line in lines:
        if re.match(r"^\s*environment = \{\s*$", line):
            in_environment = True
            continue
        if in_environment:
            if re.match(r"^\s*\}\s*$", line):
                in_environment = False
                continue
            match = re.match(r"^\s*([^=\s]+)\s+=>\s+(.*?)\s*$", line)
            if match:
                environment[match.group(1)] = match.group(2)
            continue
        match = re.match(r"^\s*state = (.*?)\s*$", line)
        if match and state == "-":
            state = match.group(1)
        match = re.match(r"^\s*pid = ([0-9]+)\s*$", line)
        if match and pid == "-":
            pid = match.group(1)
else:
    with open(source, "rb") as handle:
        document = plistlib.load(handle)
    environment = document.get("EnvironmentVariables") or {}

if not isinstance(environment, dict) or any(
    not isinstance(key, str) or not isinstance(value, str)
    for key, value in environment.items()
):
    raise SystemExit("object API recovery refused: invalid environment dictionary")

home = environment.get("HOME") or default_home
config_path = environment.get("STADO_CONFIG") or default_config

def expand_path(value, fallback=""):
    value = value or fallback
    if not value:
        return ""
    if value == "~":
        value = home
    elif value.startswith("~/"):
        value = os.path.join(home, value[2:])
    return os.path.realpath(os.path.abspath(value))

try:
    with open(expand_path(config_path), encoding="utf-8") as handle:
        configuration = json.load(handle)
except (OSError, ValueError) as error:
    raise SystemExit(f"object API recovery refused: cannot read loaded host config: {error}")

def configured(*parts):
    value = configuration
    for part in parts:
        value = value.get(part) if isinstance(value, dict) else None
    return value if isinstance(value, str) else ""

def resolve(env_name, config_parts, fallback=""):
    return environment.get(env_name) or configured(*config_parts) or fallback

def canonical_backend(value):
    return "stado" if value == "stado-object" else value

primary_backend = canonical_backend(
    resolve("WC_STORAGE_BACKEND", ("storage", "backend"))
)
primary_root = expand_path(
    resolve(
        "WC_LOCAL_STORAGE_PATH",
        ("storage", "local", "path"),
        os.path.join(home, ".stado", "local-storage"),
    )
)
backup_backend = canonical_backend(
    resolve("WC_BACKUP_STORAGE_BACKEND", ("storage", "backup", "backend"))
)
backup_root = expand_path(
    resolve(
        "WC_BACKUP_LOCAL_STORAGE_PATH",
        ("storage", "backup", "local", "path"),
    )
)

legacy_implicit_backup = primary_backend == "stado"
if primary_backend in ("", "local"):
    served_backend = "local"
    served_root = primary_root
elif legacy_implicit_backup:
    served_backend = backup_backend
    served_root = backup_root if backup_backend == "local" else ""
else:
    served_backend = primary_backend
    served_root = ""

if mode == "launchctl":
    try:
        with open(runtime_path, encoding="utf-8") as handle:
            runtime = json.load(handle)
    except (OSError, ValueError):
        runtime = None
    identity = runtime.get("storage") if isinstance(runtime, dict) else None
    if isinstance(identity, dict):
        if str(identity.get("pid")) != pid:
            raise SystemExit("object API recovery refused: runtime identity changed during inspection")
        served_backend = identity.get("backend") or ""
        served_root = expand_path(identity.get("local_path") or "")
        legacy_implicit_backup = False
    elif legacy_implicit_backup and runtime is None:
        raise SystemExit("object API recovery refused: legacy storage route is unavailable")

expected_matches = all(environment.get(key) == value for key, value in expected.items())
explicit_backend = environment.get("WC_STORAGE_BACKEND") or ""
explicit_root = expand_path(environment.get("WC_LOCAL_STORAGE_PATH") or "")

fields = (
    primary_backend,
    primary_root,
    backup_backend,
    backup_root,
    served_backend,
    served_root,
    "yes" if legacy_implicit_backup else "no",
    "yes" if expected_matches else "no",
    pid,
    state,
    explicit_backend,
    explicit_root,
)
for field in fields:
