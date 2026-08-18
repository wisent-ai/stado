#!/bin/sh
# Repin Brama's trust registry to the binary the service actually starts.
#
# Skarbiec redeems Brama's capabilities only when the running process is exactly
# the workload the registry pins: executable path, SHA-256, code signature,
# uid and gid. The registry was written at 2026-08-17 23:33 with
# `releases/0.2.25` while `current` already pointed at a newer tree, so every
# provider credential redemption failed with "authorization id does not match"
# and Brama answered every client with 403.
#
# The pin is taken from the resolved `current` link, computed live, and after
# the restart the running pid's own binary must hash to the same value - the
# proof is the process, not the file we wrote.
set -eu

REGISTRY="$HOME/.config/brama/trust/registry.json"
CURRENT="$HOME/.stado/services/brama/current"
LABEL=com.wisent.always-on.brama

[ -f "$REGISTRY" ] || { printf '%s\n' "missing $REGISTRY" >&2; exit 1; }
[ -L "$CURRENT" ] || [ -e "$CURRENT" ] || { printf '%s\n' "missing $CURRENT" >&2; exit 1; }

resolved=$(/usr/bin/readlink -f "$CURRENT")
BIN="$resolved/bin/brama"
[ -x "$BIN" ] || { printf '%s\n' "no executable at $BIN" >&2; exit 1; }

stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
/bin/cp -p "$REGISTRY" "$REGISTRY.bak-$stamp"

REGISTRY="$REGISTRY" BIN="$BIN" /usr/bin/python3 - <<'PY'
import hashlib
import json
import os
import subprocess

registry_path = os.environ["REGISTRY"]
binary = os.environ["BIN"]

with open(binary, "rb") as handle:
    digest = hashlib.sha256(handle.read()).hexdigest()
requirement = subprocess.run(
    ["codesign", "-d", "-r-", binary],
    capture_output=True, text=True, check=True,
).stdout.strip().splitlines()[-1]

with open(registry_path, encoding="utf-8") as source:
    registry = json.load(source)
workload = registry["workloads"]["brama-service"]
workload["executable_path"] = binary
workload["executable_sha256"] = digest
workload["macos_code_signing_requirement"] = requirement
registry["sequence"] = int(registry.get("sequence", 0)) + 1

with open(registry_path, "w", encoding="utf-8") as target:
    json.dump(registry, target, indent=2, sort_keys=False)
    target.write("\n")

print(json.dumps({
    "executable_path": binary,
    "executable_sha256": digest,
    "signing": requirement[:80],
    "sequence": registry["sequence"],
}, indent=2))
PY

/usr/bin/sudo -n /bin/launchctl kickstart -k "system/$LABEL" >/dev/null 2>&1 || true
health=000
for _ in 1 2 3 4 5 6 7 8 9 10 11 12; do
    /bin/sleep 5
    health=$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 6 http://127.0.0.1:8080/healthz || true)
    case "$health" in 200|401|403) break ;; esac
done
pid=$(/usr/bin/sudo -n /bin/launchctl print "system/$LABEL" 2>/dev/null \
    | /usr/bin/awk '$1=="pid"{print $3; exit}')
running_bin=$(/usr/bin/sudo -n /bin/ps -o comm= -p "${pid:-0}" 2>/dev/null)
running_sha=none
[ -n "$running_bin" ] && [ -f "$running_bin" ] \
    && running_sha=$(/usr/bin/shasum -a 256 "$running_bin" | /usr/bin/cut -d' ' -f1)
printf 'backup=%s healthz=%s pid=%s\n' "$REGISTRY.bak-$stamp" "$health" "${pid:-none}"
printf 'pinned_binary=%s\nrunning_binary=%s\n' "$BIN" "$running_bin"
printf 'pin_matches_process=%s\n' \
    "$(REGISTRY="$REGISTRY" RUN="$running_sha" /usr/bin/python3 -c 'import json,os,sys
r=json.load(open(os.environ["REGISTRY"]))
sys.exit(0 if r["workloads"]["brama-service"]["executable_sha256"]==os.environ["RUN"] else 1)' && echo yes || echo NO)"
case "$health" in 200|401|403) exit 0 ;; *) exit 1 ;; esac
