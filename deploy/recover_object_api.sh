#!/usr/bin/env bash
# Restore the Stado object API without trusting a successful read alone.
#
# launchd's loaded job and its plist are separate facts. Recovery therefore
# proves the loaded storage route, fences a serving old root before using the
# canonical metadata-preserving storage copier, and retains both the previous
# definition and a clone/copy of the destination until the corrected root is
# serving authenticated reads.
set -euo pipefail

label="com.wisent.always-on.stado-object-api"
plist="/Library/LaunchDaemons/$label.plist"
program="$HOME/.stado/services/$label/current/darwin-arm/stado"
config="${STADO_CONFIG:-$HOME/.config/stado/config.json}"
work="$HOME/.stado/work/object-api-recovery"
log="$HOME/.stado/logs/$label.log"
copy_program="${STADO_RECOVERY_COPIER:-$HOME/.stado/bin/stado}"
case "$copy_program" in
  \$HOME/*) copy_program="$HOME/${copy_program#\$HOME/}" ;;
esac
copier_ready=0
copier_digest=-
copier_version=-

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
declared_correct=0
if [ "$declared_explicit_backend" = local ] &&
  [ "$declared_explicit_root" = "$store" ]; then
  declared_correct=1
fi

active_record="$work/$label.transition-active.json"
transition_id=-
transition_started=-
transition_kind=-
source_root=-
destination_root="$store"
destination_snapshot=-
rollback_plist=-
definition_backup=-
copy_log=-
destination_exposed=0
snapshot_ready=0
transition_phase=-
rollback_needed=0

persist_record() {
  transition_phase=$1
  transition_detail=$2
  /usr/bin/python3 - "$active_record" "$transition_id" "$transition_started" \
    "$transition_phase" "$transition_kind" "$source_root" "$destination_root" \
    "$destination_snapshot" "$rollback_plist" "$definition_backup" "$copy_log" \
    "$destination_exposed" "$snapshot_ready" "$transition_detail" \
    "$copy_program" "$copier_digest" "$copier_version" <<'PY'
import datetime, json, os, sys

(path, transition_id, started_at, phase, kind, source_root, destination_root,
 destination_snapshot, rollback_plist, definition_backup, copy_log,
 destination_exposed, snapshot_ready, detail, copier, copier_digest, copier_version) = sys.argv[1:]
document = {
    "schema": "stado.object-api-storage-transition.v1",
    "transition_id": transition_id,
    "started_at": started_at,
    "updated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "phase": phase,
    "kind": kind,
    "source_root": source_root,
    "destination_root": destination_root,
    "destination_snapshot": None if destination_snapshot == "-" else destination_snapshot,
    "rollback_plist": rollback_plist,
    "definition_backup": None if definition_backup == "-" else definition_backup,
    "copy_log": copy_log,
    "destination_exposed": destination_exposed == "1",
    "snapshot_ready": snapshot_ready == "1",
    "detail": detail,
    "copier": {
        "path": copier,
        "sha256": None if copier_digest == "-" else copier_digest,
        "version": None if copier_version == "-" else copier_version,
    },
}
temporary = f"{path}.tmp-{os.getpid()}"
with open(temporary, "w", encoding="utf-8") as handle:
    json.dump(document, handle, indent=2, sort_keys=True)
    handle.write("\n")
    handle.flush()
    os.fsync(handle.fileno())
os.replace(temporary, path)
directory = os.open(os.path.dirname(path), os.O_RDONLY)
try:
    os.fsync(directory)
finally:
    os.close(directory)
PY
  /bin/chmod 600 "$active_record"
}

load_record() {
  record_fields=$(/usr/bin/python3 - "$active_record" "$work" "$label" <<'PY'
import json, os, re, sys
path, work, label = sys.argv[1:]
work = os.path.realpath(work)
with open(path, encoding="utf-8") as handle:
    document = json.load(handle)
if document.get("schema") != "stado.object-api-storage-transition.v1":
    raise SystemExit("object API recovery refused: unknown active transition record")
required = (
    "transition_id",
    "started_at",
    "phase",
    "kind",
    "source_root",
    "destination_root",
    "rollback_plist",
    "copy_log",
)
if any(not isinstance(document.get(field), str) or not document[field] for field in required):
    raise SystemExit("object API recovery refused: incomplete active transition record")
transition_id = document["transition_id"]
if not re.fullmatch(r"[0-9]{8}T[0-9]{6}Z-[0-9]+", transition_id):
    raise SystemExit("object API recovery refused: invalid active transition identity")
if document["kind"] not in ("reload", "backing-root"):
    raise SystemExit("object API recovery refused: invalid active transition kind")

def managed_path(value, basename):
    if value is None:
        return "-"
    if not isinstance(value, str):
        raise SystemExit("object API recovery refused: invalid managed recovery path")
    resolved = os.path.realpath(value)
    if os.path.dirname(resolved) != work or os.path.basename(resolved) != basename:
        raise SystemExit("object API recovery refused: managed recovery path escaped work directory")
    return resolved

snapshot = managed_path(
    document.get("destination_snapshot"),
    f"local-store.before-{transition_id}",
)
rollback = managed_path(
    document["rollback_plist"],
    f"{label}.plist.rollback-{transition_id}",
)
definition = managed_path(
    document.get("definition_backup"),
    f"{label}.plist.before-{transition_id}",
)
copy_log = managed_path(
    document["copy_log"],
    f"{label}.copy-{transition_id}.log",
)
fields = (
    transition_id,
    document["started_at"],
    document["phase"],
    document["kind"],
    os.path.realpath(document["source_root"]),
    os.path.realpath(document["destination_root"]),
    snapshot,
    rollback,
    definition,
    copy_log,
    "1" if document.get("destination_exposed") is True else "0",
    "1" if document.get("snapshot_ready") is True else "0",
)
for field in fields:
    if any(character in field for character in "\t\r\n"):
        raise SystemExit("object API recovery refused: active transition contains control characters")
print("\t".join(fields))
PY
  )
  IFS=$'\t' read -r transition_id transition_started transition_phase \
    transition_kind source_root destination_root destination_snapshot \
    rollback_plist definition_backup copy_log destination_exposed snapshot_ready \
    <<< "$record_fields"
}

fence_loaded_job() {
  capture_loaded_route
  fenced_pid=$loaded_pid
  if [ "$loaded" -eq 1 ]; then
    /usr/bin/sudo -n /bin/launchctl bootout "system/$label" >/dev/null 2>&1 || true
  fi
  fence_deadline=$((SECONDS + 30))
  while [ "$SECONDS" -lt "$fence_deadline" ]; do
    still_loaded=0
    if /usr/bin/sudo -n /bin/launchctl print "system/$label" \
      >/dev/null 2>&1; then
      still_loaded=1
    fi
    pid_alive=0
    if [[ "$fenced_pid" =~ ^[0-9]+$ ]] &&
      /bin/kill -0 "$fenced_pid" >/dev/null 2>&1; then
      pid_alive=1
    fi
    listeners=$(
      /usr/bin/sudo -n /usr/sbin/lsof -nP -a -iTCP:8765 -sTCP:LISTEN -t \
        2>/dev/null || true
    )
    if [ "$still_loaded" -eq 0 ] && [ "$pid_alive" -eq 0 ] &&
      [ -z "$listeners" ]; then
      return 0
    fi
    /bin/sleep 1
  done
  return 1
}

start_definition() {
  definition=$1
  expected_root=$2
  require_expected_environment=$3
  /usr/bin/sudo -n /bin/launchctl enable "system/$label" >/dev/null 2>&1 || true
  /usr/bin/sudo -n /bin/launchctl bootstrap system "$definition" \
    >/dev/null 2>&1 || return 1
  ready_deadline=$((SECONDS + 180))
  while [ "$SECONDS" -lt "$ready_deadline" ]; do
    if loaded_ready_for_root "$expected_root" "$require_expected_environment"; then
      return 0
    fi
    /bin/sleep 1
  done
  return 1
}

snapshot_destination() {
  if ! /usr/bin/python3 - "$destination_root" "$destination_snapshot" "$work" <<'PY'
import os, sys
root, snapshot, work = map(os.path.realpath, sys.argv[1:])
def inside(path, parent):
    try:
        return os.path.commonpath((path, parent)) == parent
    except ValueError:
        return False
if root == snapshot or inside(snapshot, root) or inside(work, root):
    raise SystemExit(1)
PY
  then
    persist_record preparation_failed \
      "recovery work or snapshot path is inside the destination root"
    printf 'unsafe_snapshot_location destination=%s snapshot=%s work=%s\n' \
      "$destination_root" "$destination_snapshot" "$work" >&2
    return 1
  fi
  snapshot_partial="$destination_snapshot.partial"
  persist_record snapshotting "preserving the pre-transition destination"
  /usr/bin/sudo -n /bin/rm -rf "$snapshot_partial"
  snapshot_mode=clone
  if ! /usr/bin/sudo -n /bin/cp -cRp "$destination_root" "$snapshot_partial"; then
    snapshot_mode=copy
    /usr/bin/sudo -n /bin/rm -rf "$snapshot_partial"
    if ! /usr/bin/sudo -n /bin/cp -Rp "$destination_root" "$snapshot_partial"; then
      persist_record preparation_failed \
        "destination clone and full-copy fallback both failed"
      printf 'destination_snapshot_failed %s\n' "$destination_snapshot" >&2
      return 1
    fi
  fi
  /usr/bin/sudo -n /bin/mv "$snapshot_partial" "$destination_snapshot"
  /bin/sync
  snapshot_ready=1
  persist_record prepared \
    "durable pre-transition destination snapshot mode=$snapshot_mode"
}

prepare_copy_program() {
  [ "$copier_ready" -eq 0 ] || return 0
  copier_identity=$(/usr/bin/python3 - "$copy_program" \
    "${STADO_RECOVERY_COPIER_SHA256:-}" <<'PY'
import hashlib, os, re, stat, struct, subprocess, sys
path, expected = sys.argv[1:]
metadata = os.stat(path)
if not stat.S_ISREG(metadata.st_mode) or not os.access(path, os.X_OK):
    raise SystemExit("recovery copier is not an executable file")
if expected and (
    metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) != 0o700
):
    raise SystemExit("staged recovery copier must be owned by this account with mode 0700")
with open(path, "rb") as handle:
    header = handle.read(8)
    if len(header) != 8:
        raise SystemExit("recovery copier has no native executable header")
    magic, cpu = struct.unpack("<II", header)
    if magic == 0xFEEDFACF:
        architectures = [cpu]
    else:
        magic, count = struct.unpack(">II", header)
        if magic not in (0xCAFEBABE, 0xCAFEBABF) or count > 64:
            raise SystemExit("recovery copier is not a Mach-O executable")
        entry_size = 20 if magic == 0xCAFEBABE else 32
        architectures = []
        for _ in range(count):
            entry = handle.read(entry_size)
            if len(entry) != entry_size:
                raise SystemExit("recovery copier has an incomplete architecture table")
            architectures.append(struct.unpack(">I", entry[:4])[0])
    wanted = {"arm64": 0x0100000C, "x86_64": 0x01000007}.get(os.uname().machine)
    if wanted not in architectures:
        raise SystemExit("recovery copier does not carry this host's native architecture")
    handle.seek(0)
    digest = hashlib.sha256()
    for block in iter(lambda: handle.read(1024 * 1024), b""):
        digest.update(block)
actual = digest.hexdigest()
if expected and actual != expected:
    raise SystemExit("recovery copier digest differs from the invoking Stado binary")
result = subprocess.run([path, "--version"], capture_output=True, text=True, timeout=10, check=True)
match = re.search(r"(\d+)\.(\d+)\.(\d+)", result.stdout)
if not match or tuple(map(int, match.groups())) < (0, 16, 5):
    raise SystemExit("storage recovery requires Stado 0.16.5 or newer for byte-exact copying")
print(f"{match.group(0)}\t{actual}")
PY
  ) || return 1
  IFS=$'\t' read -r copier_version copier_digest <<< "$copier_identity"
  copier_ready=1
}

copy_store() {
  from_root=$1
  to_root=$2
  output=$3
  # A prior interrupted run may resume after its last clean prefix. Finish that
  # suffix, then run once more from the exhausted cursor so writes accepted by
  # a restored source before this fence cannot hide in an earlier prefix.
  "$copy_program" storage copy --source-offline \
    --from local --from-path "$from_root" \
    --to local --to-path "$to_root" > "$output" 2>&1 &&
    "$copy_program" storage copy --source-offline \
      --from local --from-path "$from_root" \
      --to local --to-path "$to_root" >> "$output" 2>&1
}

rollback_to_source() {
  reason=$1
  rollback_needed=0
  reverse_ok=1
  persist_record rollback_started "$reason" || true
  if ! /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel \
    "$rollback_plist" "$plist"; then
    persist_record rollback_failed "could not install rollback definition" || true
    return 1
  fi
  if ! fence_loaded_job; then
    persist_record rollback_failed "could not fence destination API" || true
    return 1
  fi
  if [ "$destination_exposed" -eq 1 ] && [ "$source_root" != "$destination_root" ]; then
    reverse_log="$work/$label.reverse-$transition_id.log"
    persist_record reverse_copying \
      "merging any destination writes back into the old root" || true
    if copy_store "$destination_root" "$source_root" "$reverse_log"; then
      destination_exposed=0
      persist_record reverse_copied "destination writes merged into old root" || true
    else
      reverse_ok=0
      persist_record reverse_copy_failed \
        "destination retained; reverse copy is resumable before the next transition" || true
    fi
  fi
  if start_definition "$plist" "$source_root" no; then
    if [ "$reverse_ok" -eq 1 ]; then
      persist_record rollback_serving "old root restored and authenticated" || true
      return 0
    fi
    persist_record rollback_serving_pending_reverse \
      "old root restored; durable destination may contain writes pending reverse copy" || true
    return 1
  fi
  persist_record rollback_failed "old root did not become ready within 180 seconds" || true
  return 1
}

cleanup() {
  rc=$?
  trap - EXIT HUP INT TERM
  if [ "$rollback_needed" -eq 1 ]; then
    rollback_to_source "interrupted recovery exit=$rc" || true
  fi
  /bin/rm -f "$staged"
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

reconcile_skarbiec_bootstrap
has_active_record=0
if [ -f "$active_record" ]; then
  load_record
  has_active_record=1
  if [ "$transition_kind" = backing-root ]; then
    prepare_copy_program || exit 73
  fi
  if [ "$destination_root" != "$store" ]; then
    printf 'active_transition_destination_mismatch recorded=%s declared=%s\n' \
      "$destination_root" "$store" >&2
    exit 70
  fi
fi

# A completed read is acceptable only when launchd's loaded process owns the
# listener and its loaded route explicitly names the declared local root. If an
# interrupted copy had already restored the old root, an external reload of the
# destination does not prove the old root's later writes crossed the fence:
# reverse it first, then resume the recorded transition.
runtime_correct=0
if loaded_ready_for_root "$store" yes; then
  runtime_correct=1
fi
if [ "$runtime_correct" -eq 1 ] && [ "$has_active_record" -eq 1 ] &&
  [ "$transition_kind" = backing-root ] && [ "$destination_exposed" -eq 0 ]; then
  destination_exposed=1
  rollback_needed=1
  persist_record externally_exposed_destination \
    "correct root was reloaded before the recorded copy committed"
  if ! rollback_to_source "externally exposed destination needs recorded copy resumed"; then
    printf 'rollback_incomplete record=%s\n' "$active_record" >&2
    exit 71
  fi
  runtime_correct=0
fi
if [ "$runtime_correct" -eq 1 ]; then
  if [ "$same" -eq 0 ] || [ "$declared_correct" -eq 0 ]; then
    /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel "$staged" "$plist"
    same=1
  fi
  if [ "$has_active_record" -eq 1 ]; then
    destination_exposed=0
    persist_record committed \
      "corrected root already served when interrupted recovery resumed"
    completed_record="$work/$label.transition-$transition_id.completed.json"
    /bin/mv "$active_record" "$completed_record"
  fi
  reconcile_ingress
  printf 'already_healthy %s backend=local store=%s loaded_environment=matched\n' \
    "$label" "$store"
  exit 0
fi

resuming=0
if [ "$has_active_record" -eq 1 ]; then
  resuming=1
  if [ ! -f "$rollback_plist" ]; then
    printf 'active_transition_rollback_missing %s\n' "$rollback_plist" >&2
    exit 70
  fi
  if [ "$destination_exposed" -eq 1 ]; then
    rollback_needed=1
    if ! rollback_to_source "resuming an interrupted exposed destination"; then
      printf 'rollback_incomplete record=%s\n' "$active_record" >&2
      exit 71
    fi
  fi
else
  capture_loaded_route
  if [ "$loaded" -eq 1 ]; then
    source_backend=$loaded_served_backend
    source_root=$loaded_served_root
    source_legacy=$loaded_legacy
  elif [ "$declared_served_backend" != "-" ]; then
    source_backend=$declared_served_backend
    source_root=$declared_served_root
    source_legacy=$declared_legacy
  else
    source_backend=local
    source_root=$store
    source_legacy=no
  fi
  if [ "$source_backend" != local ] || [ "$source_root" = "-" ] ||
    [ -z "$source_root" ]; then
    printf 'source_route_unsupported backend=%s root=%s legacy=%s\n' \
      "$source_backend" "$source_root" "$source_legacy" >&2
    exit 72
  fi
  source_root=$(
    /usr/bin/python3 - "$source_root" <<'PY'
import os, sys
print(os.path.realpath(os.path.abspath(sys.argv[1])))
PY
  )
  transition_id="$(/bin/date -u +%Y%m%dT%H%M%SZ)-$$"
  transition_started=$(/bin/date -u +%Y-%m-%dT%H:%M:%SZ)
  copy_log="$work/$label.copy-$transition_id.log"
  rollback_plist="$work/$label.plist.rollback-$transition_id"
  definition_backup="$work/$label.plist.before-$transition_id"
  transition_kind=reload
  if [ "$source_root" != "$destination_root" ]; then
    transition_kind=backing-root
    destination_snapshot="$work/local-store.before-$transition_id"
  fi

  /usr/bin/python3 - "$staged" "$rollback_plist" "$source_root" <<'PY'
import os, plistlib, sys
source, destination, root = sys.argv[1:]
with open(source, "rb") as handle:
    document = plistlib.load(handle)
environment = document["EnvironmentVariables"]
environment["WC_STORAGE_BACKEND"] = "local"
environment["WC_LOCAL_STORAGE_PATH"] = root
with open(destination, "wb") as handle:
    plistlib.dump(document, handle, fmt=plistlib.FMT_XML, sort_keys=False)
    handle.flush()
    os.fsync(handle.fileno())
PY
  /bin/chmod 600 "$rollback_plist"
  if /usr/bin/sudo -n /bin/test -f "$plist"; then
    /usr/bin/sudo -n /bin/cp -p "$plist" "$definition_backup"
    /usr/bin/sudo -n /usr/sbin/chown "$account" "$definition_backup"
    /bin/chmod 600 "$definition_backup"
  else
    definition_backup=-
  fi

  persist_record preparing \
    "loaded_backend=$source_backend loaded_root=$source_root legacy_implicit_backup=$source_legacy"
  if [ "$transition_kind" = reload ]; then
    persist_record prepared "no backing-root change; storage copy not required"
  fi
fi

if [ "$transition_kind" = backing-root ]; then
  prepare_copy_program || exit 73
  if [ ! -d "$source_root" ] || [ ! -r "$source_root/registry.json" ]; then
    persist_record preparation_failed "source root or registry is unreadable"
    rollback_to_source "source root or registry is unreadable" || true
    printf 'transition_source_missing %s record=%s\n' \
      "$source_root" "$active_record" >&2
    exit 73
  fi
  if ! /usr/bin/python3 - "$source_root" "$destination_root" <<'PY'
import os, sys
source, destination = map(os.path.realpath, sys.argv[1:])
try:
    overlap = (
        os.path.commonpath((source, destination)) == source
        or os.path.commonpath((source, destination)) == destination
    )
except ValueError:
    overlap = False
raise SystemExit(1 if overlap else 0)
PY
  then
    persist_record preparation_failed \
      "source and destination roots overlap physically"
    rollback_to_source "source and destination roots overlap physically" || true
    printf 'transition_roots_overlap source=%s destination=%s record=%s\n' \
      "$source_root" "$destination_root" "$active_record" >&2
    exit 73
  fi
  if [ ! -d "$destination_snapshot" ]; then
    if [ "$snapshot_ready" -eq 1 ]; then
      persist_record preparation_failed \
        "completed pre-transition destination snapshot is missing"
      rollback_to_source "completed destination snapshot is missing" || true
      printf 'active_transition_snapshot_missing %s phase=%s record=%s\n' \
        "$destination_snapshot" "$transition_phase" "$active_record" >&2
      exit 75
    fi
    if ! snapshot_destination; then
      rollback_to_source "destination snapshot failed before transition" || true
      exit 74
    fi
  fi
fi

# Persist the corrected definition before unloading. A crash after this point
# leaves either the old loaded job plus a corrected next definition, or the
# durable transition record needed to resume the fenced copy.
rollback_needed=1
persist_record installing_corrected "corrected plist will be installed before unload"
if ! /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel "$staged" "$plist"; then
  rollback_to_source "corrected plist installation failed" || true
  printf 'corrected_definition_install_failed record=%s\n' "$active_record" >&2
  exit 76
fi
persist_record corrected_installed "corrected plist persisted before source fence"
if ! fence_loaded_job; then
  rollback_to_source "source API did not fence within 30 seconds" || true
  printf 'source_fence_timeout record=%s\n' "$active_record" >&2
  exit 77
fi
persist_record source_fenced "loaded source API and port 8765 are stopped"

if [ "$transition_kind" = backing-root ]; then
  # Repair only the selected product store. The snapshot above preserves its
  # prior ownership and contents; no parent or unrelated host path is touched.
  if ! /usr/bin/sudo -n /usr/sbin/chown -Rh "$account" "$destination_root" ||
    ! /usr/bin/sudo -n /bin/chmod -R u+rwX "$destination_root"; then
    rollback_to_source "destination ownership repair failed" || true
    printf 'destination_ownership_repair_failed %s\n' "$destination_root" >&2
    exit 78
  fi
  persist_record copying \
    "metadata-preserving non-deleting copy from old root to declared root"
  if ! copy_store "$source_root" "$destination_root" "$copy_log"; then
    rollback_to_source "storage copy failed; destination retained for resume" || true
    printf 'storage_copy_failed record=%s log=%s\n' "$active_record" "$copy_log" >&2
    exit 79
  fi
  persist_record copied "old-root writes copied and verified by stado storage copy"
  destination_exposed=1
  persist_record starting_destination \
    "destination may accept writes; rollback must reverse-copy before restoring source"
fi

if ! start_definition "$plist" "$destination_root" yes; then
  rollback_to_source "corrected root did not become ready within 180 seconds" || true
  printf 'corrected_root_not_ready record=%s\n' "$active_record" >&2
  exit 80
fi
rollback_needed=0
destination_exposed=0
persist_record committed "corrected local root serves authenticated reads"
completed_record="$work/$label.transition-$transition_id.completed.json"
/bin/mv "$active_record" "$completed_record"
reconcile_ingress
printf 'recovered %s backend=local store=%s prior_root=%s kind=%s record=%s snapshot=%s\n' \
  "$label" "$destination_root" "$source_root" "$transition_kind" \
  "$completed_record" "$destination_snapshot"
