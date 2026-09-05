#!/usr/bin/env bash
# Restore the Stado object API without trusting a successful read alone.
#
# launchd's loaded job and its plist are separate facts. Recovery proves the
# loaded storage route and authenticated reads before reporting readiness.
# A different authority requires `host storage-root-reconcile`, which owns the
# snapshots, writer fence, namespace-qualified copy, and rollback. This helper
# repairs only the same-root listener using the host's canonical delivered Stado.
set -euo pipefail

label="com.wisent.always-on.stado-object-api"
plist="/Library/LaunchDaemons/$label.plist"
program="$HOME/.stado/bin/stado"
config="${STADO_CONFIG:-$HOME/.config/stado/config.json}"
work="$HOME/.stado/work/object-api-recovery"
log="$HOME/.stado/logs/$label.log"

if [ "$(/usr/bin/uname -s)" != "Darwin" ]; then
  printf 'unsupported_os %s\n' "$(/usr/bin/uname -s)" >&2
  exit 65
fi
if [ ! -x "$program" ]; then
  printf 'program_missing %s\n' "$program" >&2
  exit 66
fi

store="${WC_LOCAL_STORAGE_PATH:-$HOME/.stado/local-storage}"
backup_store="${WC_BACKUP_LOCAL_STORAGE_PATH:-$HOME/.stado/local-backup}"
if [ -r "$config" ]; then
  configured=$(/usr/bin/python3 - "$config" <<'PY'
import json, os, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
value = ((document.get("storage") or {}).get("local") or {}).get("path") or ""
print(os.path.realpath(os.path.abspath(os.path.expanduser(value))) if value else "")
PY
)
  if [ -z "${WC_LOCAL_STORAGE_PATH:-}" ] && [ -n "$configured" ]; then
    store="$configured"
  fi
  configured_backup=$(/usr/bin/python3 - "$config" <<'PY'
import json, os, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
value = ((((document.get("storage") or {}).get("backup") or {}).get("local") or {}).get("path") or "")
print(os.path.realpath(os.path.abspath(os.path.expanduser(value))) if value else "")
PY
)
  if [ -z "${WC_BACKUP_LOCAL_STORAGE_PATH:-}" ] &&
    [ -n "$configured_backup" ]; then
    backup_store="$configured_backup"
  fi
fi
store=$(/usr/bin/python3 - "$store" <<'PY'
import os, sys
print(os.path.realpath(os.path.abspath(os.path.expanduser(sys.argv[1]))))
PY
)
backup_store=$(/usr/bin/python3 - "$backup_store" <<'PY'
import os, sys
print(os.path.realpath(os.path.abspath(os.path.expanduser(sys.argv[1]))))
PY
)
object_url="http://127.0.0.1:8765"
object_namespace="probierz"
object_token_file="$HOME/.stado/queue-object-api-token"
if [ -r "$config" ]; then
  configured_url=$(/usr/bin/python3 - "$config" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
print((((document.get("storage") or {}).get("stado") or {}).get("url") or ""))
PY
)
  configured_namespace=$(/usr/bin/python3 - "$config" <<'PY'
import json, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
print((((document.get("storage") or {}).get("stado") or {}).get("namespace") or ""))
PY
)
  configured_token_file=$(/usr/bin/python3 - "$config" <<'PY'
import json, os, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
value = (((document.get("storage") or {}).get("stado") or {}).get("token_file") or "")
print(os.path.abspath(os.path.expanduser(value)) if value else "")
PY
)
  if [ -n "$configured_url" ]; then object_url="$configured_url"; fi
  if [ -n "$configured_namespace" ]; then object_namespace="$configured_namespace"; fi
  if [ -n "$configured_token_file" ]; then object_token_file="$configured_token_file"; fi
fi
if [ ! -d "$store" ] || [ ! -r "$store/registry.json" ]; then
  printf 'local_store_missing %s\n' "$store" >&2
  exit 67
fi
if [ "$backup_store" = "$store" ]; then
  printf 'local_backup_matches_primary %s\n' "$store" >&2
  exit 68
fi
/bin/mkdir -p "$backup_store"
/bin/chmod 700 "$backup_store"

/bin/mkdir -p "$work" "$HOME/.stado/logs"
/bin/chmod 700 "$work" "$HOME/.stado/logs"
/usr/bin/touch "$log"
/bin/chmod 600 "$log"
staged=$(/usr/bin/mktemp "$work/$label.plist.XXXXXX")
trap '/bin/rm -f "$staged"' EXIT HUP INT TERM
account=$(/usr/bin/id -un)

/usr/bin/python3 - "$staged" "$label" "$program" "$store" "$backup_store" "$account" "$log" "$HOME" "$config" <<'PY'
import plistlib, sys

path, label, program, store, backup_store, account, log, home, config = sys.argv[1:]
document = {
    "Label": label,
    "ProgramArguments": [
        program,
        "dashboard",
        "--bind",
        "127.0.0.1",
        "--port",
        "8765",
    ],
    "EnvironmentVariables": {
        "HOME": home,
        "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "STADO_CONFIG": config,
        "GNUPGHOME": f"{home}/.gnupg",
        "SKARBIEC_VAULT_FILE": f"{home}/.stado/skarbiec.vault.json",
        "WC_OBJECT_SKARBIEC_TOKEN_FILE": f"{home}/.stado/stado-object-api-verifier-skarbiec-token",
        "WC_RELEASE_SKARBIEC_TOKEN_FILE": f"{home}/.stado/stado-release-api-verifier-skarbiec-token",
        "WC_STORAGE_BACKEND": "local",
        "WC_LOCAL_STORAGE_PATH": store,
        "WC_BACKUP_STORAGE_BACKEND": "local",
        "WC_BACKUP_LOCAL_STORAGE_PATH": backup_store,
    },
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


reconcile_ingress() {
  tailscale_bin=''
  if /usr/bin/which tailscale >/dev/null 2>&1; then
    tailscale_bin=$(/usr/bin/which tailscale)
  else
    for candidate in \
      /Applications/Tailscale.app/Contents/MacOS/Tailscale \
      /usr/local/bin/tailscale \
      /opt/homebrew/bin/tailscale
    do
      if [ -x "$candidate" ]; then
        tailscale_bin="$candidate"
        break
      fi
    done
  fi
  if [ -z "$tailscale_bin" ]; then
    printf 'ingress_unmanaged tailscale_absent\n'
    return 0
  fi

  status="$work/tailscale-serve-status.json"
  "$tailscale_bin" serve status --json > "$status"
  route_state=$(/usr/bin/python3 - "$status" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
declared = any(
    key.endswith(":443") and value is True
    for key, value in (document.get("AllowFunnel") or {}).items()
)
if not declared:
    print("undeclared")
    raise SystemExit
routes = (
    "/api/object",
    "/api/object/compose",
    "/api/object/list",
    "/api/object/stat",
    "/api/release/object",
)
matched = any(
    key.endswith(":443")
    and all(
        ((value.get("Handlers") or {}).get(route) or {}).get("Proxy")
        == f"http://127.0.0.1:8765{route}"
        for route in routes
    )
    for key, value in (document.get("Web") or {}).items()
)
print("matched" if matched else "drifted")
PY
)
  if [ "$route_state" = undeclared ]; then
    printf 'ingress_unmanaged https=443 object_routes\n'
    return 0
  fi
  if [ "$route_state" = drifted ]; then
    for route in /api/object /api/object/compose /api/object/list /api/object/stat /api/release/object; do
      "$tailscale_bin" funnel --bg --yes --https=443 \
        --set-path "$route" "http://127.0.0.1:8765$route"
    done
    "$tailscale_bin" serve status --json > "$status"
  fi
  /usr/bin/python3 - "$status" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)
routes = (
    "/api/object",
    "/api/object/compose",
    "/api/object/list",
    "/api/object/stat",
    "/api/release/object",
)
assert any(
    key.endswith(":443") and value is True
    for key, value in (document.get("AllowFunnel") or {}).items()
)
assert any(
    key.endswith(":443")
    and all(
        ((value.get("Handlers") or {}).get(route) or {}).get("Proxy")
        == f"http://127.0.0.1:8765{route}"
        for route in routes
    )
    for key, value in (document.get("Web") or {}).items()
)
PY
  printf 'ingress_reconciled https=443 object_routes prior=%s\n' "$route_state"
}

# Break the one dependency cycle the installed release agent cannot repair
# itself: an interrupted handoff may leave Stado's exact Skarbiec proxy alive,
# with no recorded release owner and a dead candidate upstream. The host's
# last-good registry (or its physical canonical registry when no cache exists)
# supplies every coordinate below. This helper never guesses a bind, state
# path, plist, label, or readiness contract.
reconcile_skarbiec_bootstrap() {
  host=$(/bin/hostname -s | /usr/bin/tr '[:upper:]' '[:lower:]')
  release_registry="$HOME/.stado/cache/registry-last-good.json"
  if [ ! -r "$release_registry" ]; then
    release_registry="$store/registry.json"
  fi
  plan=$(
    /usr/bin/python3 - "$release_registry" "$host" "$account" "$HOME" <<'PY'
import json, os, sys

path, host, account, home = sys.argv[1:]
with open(path, encoding="utf-8") as handle:
    document = json.load(handle)
products = ((document.get("release_control") or {}).get("products") or {})
policy = products.get("skarbiec")
if not isinstance(policy, dict):
    print("absent")
    raise SystemExit
strategy = policy.get("strategy") or {}
if strategy.get("kind") != "blue-green":
    raise SystemExit("skarbiec bootstrap refused: release strategy is not blue-green")
targets = policy.get("targets") or {}
target_name = host if host in targets else None
if target_name is None:
    matches = [
        name
        for name, target in targets.items()
        if isinstance(target, dict)
        and target.get("run_as_user") == account
        and os.path.abspath(os.path.expanduser(target.get("home") or "")) == home
    ]
    if len(matches) != 1:
        raise SystemExit(
            "skarbiec bootstrap refused: registry does not identify this host exactly"
        )
    target_name = matches[0]
target = targets[target_name]
state_dir = target.get("state_dir") or ""
stable_bind = target.get("stable_bind") or ""
readiness_path = target.get("readiness_path") or ""
legacy_plist = target.get("legacy_launchd_plist") or ""
legacy_label = target.get("legacy_launchd_label") or ""
candidate_ports = target.get("candidate_ports") or []
timeout = strategy.get("readiness_timeout_seconds")
required = (state_dir, stable_bind, readiness_path)
if not all(isinstance(value, str) and value for value in required):
    raise SystemExit("skarbiec bootstrap refused: release target is incomplete")
legacy_values = (legacy_plist, legacy_label)
if not all(isinstance(value, str) for value in legacy_values):
    raise SystemExit("skarbiec bootstrap refused: legacy plist and label must be strings")
legacy_configured = all(bool(value) for value in legacy_values)
if legacy_configured != any(bool(value) for value in legacy_values):
    raise SystemExit(
        "skarbiec bootstrap refused: legacy plist and label must be declared together"
    )
checked = required + (legacy_values if legacy_configured else ())
if any(any(character in value for character in "\t\r\n") for value in checked):
    raise SystemExit("skarbiec bootstrap refused: release target contains control characters")
host_part, separator, port_text = stable_bind.partition(":")
if host_part != "127.0.0.1" or separator != ":":
    raise SystemExit("skarbiec bootstrap refused: stable bind is not loopback")
try:
    stable_port = int(port_text)
except ValueError:
    stable_port = 0
if not 1 <= stable_port <= 65535:
    raise SystemExit("skarbiec bootstrap refused: stable bind port is invalid")
if not readiness_path.startswith("/") or any(character.isspace() for character in readiness_path):
    raise SystemExit("skarbiec bootstrap refused: readiness path is invalid")
if legacy_configured and (
    not all(
        character.isascii() and (character.isalnum() or character in ".-_")
        for character in legacy_label
    )
    or legacy_plist != f"/Library/LaunchDaemons/{legacy_label}.plist"
):
    raise SystemExit("skarbiec bootstrap refused: legacy launchd identity is invalid")
if (
    not isinstance(candidate_ports, list)
    or len(candidate_ports) != 2
    or any(not isinstance(port, int) or not 1 <= port <= 65535 for port in candidate_ports)
):
    raise SystemExit("skarbiec bootstrap refused: candidate ports are invalid")
if not isinstance(timeout, int) or not 1 <= timeout <= 600:
    raise SystemExit("skarbiec bootstrap refused: readiness timeout is invalid")
state_dir = os.path.abspath(os.path.expanduser(state_dir))
if legacy_configured:
    legacy_plist = os.path.abspath(os.path.expanduser(legacy_plist))
else:
    legacy_plist = legacy_label = "-"
fields = (
    "managed",
    target_name,
    os.path.join(state_dir, "skarbiec.json"),
    os.path.join(state_dir, "skarbiec-proxy.json"),
    stable_bind,
    ",".join(str(port) for port in candidate_ports),
    readiness_path,
    legacy_plist,
    legacy_label,
    str(timeout),
)
print("\t".join(fields))
PY
  )
  IFS=$'\t' read -r managed target_name release_state proxy_state stable_bind \
    candidate_ports readiness_path legacy_plist legacy_label readiness_timeout <<< "$plan"
  if [ "$managed" = absent ]; then
    printf 'skarbiec_bootstrap unmanaged\n'
    return 0
  fi
  if [ "$managed" != managed ]; then
    printf 'skarbiec_bootstrap refused invalid_registry_plan\n' >&2
    return 1
  fi

  ownership=$(
    /usr/bin/python3 - "$release_state" "$target_name" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    state = json.load(handle)
if state.get("product") != "skarbiec" or state.get("target") != sys.argv[2]:
    raise SystemExit("skarbiec bootstrap refused: release state identity differs")
owned = [state.get(field) for field in ("active", "candidate", "previous")]
print("owned" if any(record is not None for record in owned) or state.get("proxy_pid") is not None else "unowned")
PY
  )
  if [ "$ownership" = owned ]; then
    if /usr/bin/curl --silent --show-error --fail --max-time 3 \
      "http://$stable_bind$readiness_path" >/dev/null 2>&1; then
      printf 'skarbiec_bootstrap active_release_owner stable=%s\n' "$stable_bind"
      return 0
    fi
    printf 'skarbiec bootstrap refused: release state owns an unavailable stable proxy\n' >&2
    return 1
  fi

  upstream=$(
    /usr/bin/python3 - "$proxy_state" "$candidate_ports" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    state = json.load(handle)
upstream = state.get("upstream")
allowed = {f"127.0.0.1:{port}" for port in sys.argv[2].split(",")}
if upstream not in allowed:
    raise SystemExit("skarbiec bootstrap refused: proxy upstream is not a declared candidate")
print(upstream)
PY
  )
  if /usr/bin/curl --silent --show-error --fail --max-time 3 \
    "http://$upstream$readiness_path" >/dev/null 2>&1; then
    printf 'skarbiec_bootstrap active_handoff upstream=%s\n' "$upstream"
    return 0
  fi

  processes="$work/skarbiec-release-proxies.txt"
  /bin/ps axww -o pid= -o command= > "$processes"
  match=$(
    /usr/bin/python3 - "$processes" "$proxy_state" "$stable_bind" <<'PY'
import os, shlex, sys

matches = []
with open(sys.argv[1], encoding="utf-8", errors="replace") as handle:
    for line in handle:
        fields = line.strip().split(maxsplit=1)
        if len(fields) != 2 or not fields[0].isdigit():
            continue
        try:
            argv = shlex.split(fields[1])
        except ValueError:
            continue
        expected = [
            "release",
            "proxy",
            "--state",
            sys.argv[2],
            "--bind",
            sys.argv[3],
        ]
        if (
            len(argv) == 7
            and argv[1:] == expected
            and os.path.basename(argv[0]) == "stado"
            and os.path.isfile(argv[0])
            and os.access(argv[0], os.X_OK)
        ):
            matches.append((int(fields[0]), argv[0]))
if not matches:
    print("none")
elif len(matches) == 1:
    print(f"exact\t{matches[0][0]}\t{matches[0][1]}")
else:
    raise SystemExit(
        f"skarbiec bootstrap refused: {len(matches)} exact release proxies found"
    )
PY
  )
  IFS=$'\t' read -r match_kind proxy_pid proxy_executable <<< "$match"
  if [ "$match_kind" = none ]; then
    printf 'skarbiec_bootstrap no_exact_orphan\n'
    return 0
  fi
  if [ "$match_kind" != exact ] || [ -z "$proxy_pid" ] || [ -z "$proxy_executable" ]; then
    printf 'skarbiec_bootstrap refused invalid_process_match\n' >&2
    return 1
  fi
  expected_command="$proxy_executable release proxy --state $proxy_state --bind $stable_bind"
  observed_command=$(/bin/ps -p "$proxy_pid" -o command= 2>/dev/null || true)
  if [ "$observed_command" != "$expected_command" ]; then
    printf 'skarbiec_bootstrap refused proxy_changed_before_term\n' >&2
    return 1
  fi
  proxy_owner=$(
    /bin/ps -p "$proxy_pid" -o user= 2>/dev/null | /usr/bin/tr -d '[:space:]'
  )
  proxy_version=$("$proxy_executable" --version 2>/dev/null || true)
  if [ "$proxy_owner" != "$account" ] || [[ "$proxy_version" != stado\ * ]]; then
    printf 'skarbiec_bootstrap refused proxy_identity_mismatch\n' >&2
    return 1
  fi
  if [ "$legacy_plist" = "-" ] || [ "$legacy_label" = "-" ]; then
    if [ "$legacy_plist" != "-" ] || [ "$legacy_label" != "-" ]; then
      printf 'skarbiec_bootstrap refused partial_legacy_restore_plan\n' >&2
      return 1
    fi
    printf 'skarbiec_bootstrap refused exact_orphan_has_no_legacy_restore target=%s\n' \
      "$target_name" >&2
    return 1
  fi
  if [ ! -f "$legacy_plist" ]; then
    printf 'skarbiec_bootstrap refused legacy_plist_missing=%s\n' "$legacy_plist" >&2
    return 1
  fi
  plist_label=$(
    /usr/bin/plutil -extract Label raw -o - "$legacy_plist" 2>/dev/null || true
  )
  if [ "$plist_label" != "$legacy_label" ]; then
    printf 'skarbiec_bootstrap refused legacy_plist_label_mismatch\n' >&2
    return 1
  fi
  stable_port=${stable_bind##*:}
  listener_pid=$(
    /usr/sbin/lsof -nP -a -p "$proxy_pid" -iTCP:"$stable_port" \
      -sTCP:LISTEN -t 2>/dev/null | /usr/bin/sort -u || true
  )
  if [ "$listener_pid" != "$proxy_pid" ]; then
    printf 'skarbiec_bootstrap refused exact_proxy_does_not_own_bind\n' >&2
    return 1
  fi

  /bin/kill -TERM "$proxy_pid"
  attempt=0
  while /bin/kill -0 "$proxy_pid" >/dev/null 2>&1; do
    if [ "$attempt" -ge 50 ]; then
      printf 'skarbiec_bootstrap refused proxy_pid_did_not_exit pid=%s\n' "$proxy_pid" >&2
      return 1
    fi
    attempt=$((attempt + 1))
    /bin/sleep 0.1
  done

  # Every restoration prerequisite was proved before the exact orphan was
  # signalled. From here the declared legacy unit is the only actor allowed to
  # reclaim the stable bind.
  set +e
  bootstrap_detail=$(
    /usr/bin/sudo -n /bin/launchctl bootstrap system "$legacy_plist" 2>&1
  )
  bootstrap_rc=$?
  set -e
  if [ "$bootstrap_rc" -ne 0 ] && [ "$bootstrap_rc" -ne 5 ]; then
    bootstrap_detail=$(printf '%s' "$bootstrap_detail" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-160)
    printf 'skarbiec_bootstrap refused bootstrap_%s:%s\n' \
      "$bootstrap_rc" "${bootstrap_detail:-launchctl said nothing}" >&2
    return 1
  fi
  /usr/bin/sudo -n /bin/launchctl enable "system/$legacy_label" >/dev/null 2>&1 || true
  attempt=0
  while [ "$attempt" -lt "$readiness_timeout" ]; do
    if /usr/bin/sudo -n /bin/launchctl print "system/$legacy_label" >/dev/null 2>&1 &&
      /usr/bin/curl --silent --show-error --fail --max-time 3 \
        "http://$stable_bind$readiness_path" >/dev/null 2>&1; then
      printf 'skarbiec_bootstrap restored target=%s bind=%s pid=%s\n' \
        "$target_name" "$stable_bind" "$proxy_pid"
      return 0
    fi
    attempt=$((attempt + 1))
    /bin/sleep 1
  done
  printf 'skarbiec_bootstrap refused legacy_not_ready bind=%s\n' "$stable_bind" >&2
  return 1
}

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
    if any(character in field for character in "\t\r\n"):
        raise SystemExit("object API recovery refused: route contains control characters")
print("\t".join(field if field else "-" for field in fields))
PY
}

capture_loaded_route() {
  loaded=0
  loaded_backend=-
  loaded_primary_root=-
  loaded_backup_backend=-
  loaded_backup_root=-
  loaded_served_backend=-
  loaded_served_root=-
  loaded_legacy=-
  loaded_env_matches=-
  loaded_pid=-
  loaded_state=-
  loaded_explicit_backend=-
  loaded_explicit_root=-
  loaded_print="$work/$label.launchctl-print"
  if /usr/bin/sudo -n /bin/launchctl print "system/$label" \
    > "$loaded_print.tmp" 2>/dev/null; then
    /bin/mv "$loaded_print.tmp" "$loaded_print"
    /bin/chmod 600 "$loaded_print"
    runtime_state="$work/$label.runtime-state.json"
    if /usr/bin/curl --silent --show-error --fail --max-time 5 \
      "${object_url%/}/api/state.json" > "$runtime_state.tmp" 2>/dev/null; then
      /bin/mv "$runtime_state.tmp" "$runtime_state"
    else
      /bin/rm -f "$runtime_state.tmp" "$runtime_state"
    fi
    route=$(inspect_route launchctl "$loaded_print")
    IFS=$'\t' read -r loaded_backend loaded_primary_root \
      loaded_backup_backend loaded_backup_root loaded_served_backend \
      loaded_served_root loaded_legacy loaded_env_matches loaded_pid \
      loaded_state loaded_explicit_backend loaded_explicit_root <<< "$route"
    loaded=1
  else
    /bin/rm -f "$loaded_print.tmp"
  fi
}

loaded_ready_for_root() {
  expected_root=$1
  require_expected_environment=$2
  capture_loaded_route
  [ "$loaded" -eq 1 ] || return 1
  [ "$loaded_state" = running ] || return 1
  [[ "$loaded_pid" =~ ^[0-9]+$ ]] || return 1
  [ "$loaded_served_backend" = local ] || return 1
  [ "$loaded_served_root" = "$expected_root" ] || return 1
  [ "$loaded_legacy" = no ] || return 1
  if [ "$require_expected_environment" = yes ]; then
    [ "$loaded_env_matches" = yes ] || return 1
  fi
  listener_pids=$(
    /usr/bin/sudo -n /usr/sbin/lsof -nP -a -iTCP:8765 -sTCP:LISTEN -t \
      2>/dev/null | /usr/bin/sort -u || true
  )
  [ "$listener_pids" = "$loaded_pid" ] || return 1
  authenticated_object_ready
}

same=0
if /usr/bin/python3 - "$staged" "$plist" <<'PY'
import plistlib, sys
try:
    with open(sys.argv[1], "rb") as expected, open(sys.argv[2], "rb") as actual:
        same = plistlib.load(expected) == plistlib.load(actual)
except (OSError, plistlib.InvalidFileException):
    same = False
raise SystemExit(0 if same else 1)
PY
then same=1; fi

declared_backend=-
declared_primary_root=-
declared_backup_backend=-
declared_backup_root=-
declared_served_backend=-
declared_served_root=-
declared_legacy=-
declared_env_matches=-
declared_pid=-
declared_state=-
declared_explicit_backend=-
declared_explicit_root=-
if /usr/bin/sudo -n /bin/test -f "$plist"; then
  declared_route=$(inspect_route plist "$plist")
  IFS=$'\t' read -r declared_backend declared_primary_root \
    declared_backup_backend declared_backup_root declared_served_backend \
    declared_served_root declared_legacy declared_env_matches declared_pid \
    declared_state declared_explicit_backend declared_explicit_root \
    <<< "$declared_route"
fi

# Root movement is a different operation from listener recovery. In particular,
# copying a whole backup over the primary can restore stale queue and registry
# state. The resident transaction qualifies the namespace and captures writers;
# this helper must neither duplicate it nor re-enable a listener during it.
reconcile_skarbiec_bootstrap
capture_loaded_route
source_backend=local
source_root=$store
source_legacy=no
if [ "$loaded" -eq 1 ]; then
  source_backend=$loaded_served_backend
  source_root=$loaded_served_root
  source_legacy=$loaded_legacy
elif [ "$declared_served_backend" != "-" ]; then
  source_backend=$declared_served_backend
  source_root=$declared_served_root
  source_legacy=$declared_legacy
fi
if [ "$source_backend" != local ] || [ "$source_root" != "$store" ] ||
  [ "$source_legacy" != no ]; then
  printf 'storage_root_handoff_required backend=%s source=%s declared=%s; use stado host storage-root-reconcile for the authority transaction\n' \
    "$source_backend" "$source_root" "$store" >&2
  exit 70
fi

if loaded_ready_for_root "$store" yes; then
  if [ "$same" -eq 0 ]; then
    /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel "$staged" "$plist"
  fi
  reconcile_ingress
  printf 'already_healthy %s backend=local store=%s loaded_environment=matched\n' \
    "$label" "$store"
  exit 0
fi

healthy=0
if loaded_ready_for_root "$store" no; then healthy=1; fi
stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
backup="$work/$label.plist.before-$stamp"
if /usr/bin/sudo -n /bin/test -f "$plist"; then
  /usr/bin/sudo -n /bin/cp "$plist" "$backup"
  /usr/bin/sudo -n /usr/sbin/chown "$account" "$backup"
  /bin/chmod 600 "$backup"
  printf 'backup %s\n' "$backup"
fi
if [ "$healthy" -eq 1 ]; then
  /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel "$staged" "$plist"
  reconcile_ingress
  printf 'persisted_while_healthy %s store=%s backup=%s loaded_job=unchanged\n' \
    "$label" "$store" "${backup:-none}"
  exit 0
fi

# Preserve the corrected definition before unload so an interrupted invocation
# can bootstrap the same root on its next run.
if [ "$same" -eq 1 ] && [ "$loaded" -eq 1 ]; then
  /usr/bin/sudo -n /bin/launchctl kickstart -k "system/$label"
  action=kickstarted
else
  /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel "$staged" "$plist"
  if [ "$loaded" -eq 1 ]; then
    /usr/bin/sudo -n /bin/launchctl bootout "system/$label"
  fi
  /usr/bin/sudo -n /bin/launchctl enable "system/$label" >/dev/null 2>&1 || true
  /usr/bin/sudo -n /bin/launchctl bootstrap system "$plist"
  action=reinstalled
fi

deadline=$((SECONDS + 180))
while [ "$SECONDS" -lt "$deadline" ]; do
  if loaded_ready_for_root "$store" yes; then
    reconcile_ingress
    printf '%s %s store=%s backup=%s\n' "$action" "$label" "$store" "${backup:-none}"
    exit 0
  fi
  /bin/sleep 1
done
printf 'authorization_timeout %s declared root did not serve authenticated reads after 180 seconds; backup=%s\n' \
  "$label" "${backup:-none}" >&2
exit 69
