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
