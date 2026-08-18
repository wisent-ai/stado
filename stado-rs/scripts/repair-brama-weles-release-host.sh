#!/bin/sh
# Move Brama to the release that entitles the weles client correctly.
#
# The unit starts `current/darwin-arm/bin/start-with-skarbiec`, and the script at
# `current` assigns the weles client `allowed_models = ["best"]`, so every weles
# request answers 403 authorization_error. Newer release trees already carry
# `weles_models = ["weles/agent/primary"]`.
#
# Repointing `current` and repinning the trust registry must happen together:
# the registry pins the executable by SHA-256, so a moved link without a moved
# pin is the failure this host just had.
set -eu

SERVICES="$HOME/.stado/services/brama"
CURRENT="$SERVICES/current"
REGISTRY="$HOME/.config/brama/trust/registry.json"
LABEL=com.wisent.always-on.brama

target=""
for tree in $(/bin/ls -td "$SERVICES"/sha256-* 2>/dev/null); do
    script="$tree/darwin-arm/bin/start-with-skarbiec"
    binary="$tree/darwin-arm/bin/brama"
    [ -f "$script" ] && [ -x "$binary" ] || continue
    if /usr/bin/grep -q 'weles_models = \["weles/agent/primary"\]' "$script"; then
        target="$tree"
        break
    fi
done
[ -n "$target" ] || { printf '%s\n' "no release tree carries weles/agent/primary for weles" >&2; exit 1; }

old_target=$(/usr/bin/readlink -f "$CURRENT" 2>/dev/null || true)
printf 'old_current=%s new_current=%s\n' "${old_target:-none}" "$target"

stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
/bin/cp -p "$REGISTRY" "$REGISTRY.bak-$stamp"

tmp_link="$SERVICES/.current.tmp.$$"
/bin/ln -sfn "$target" "$tmp_link"
/bin/mv "$tmp_link" "$CURRENT"

REGISTRY="$REGISTRY" BIN="$target/darwin-arm/bin/brama" /usr/bin/python3 - <<'PY'
import hashlib
import json
import os
import subprocess

registry_path = os.environ["REGISTRY"]
binary = os.environ["BIN"]
with open(binary, "rb") as handle:
    digest = hashlib.sha256(handle.read()).hexdigest()
requirement = subprocess.run(
    ["codesign", "-d", "-r-", binary], capture_output=True, text=True, check=True
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
print(json.dumps({"pinned": binary, "sha256": digest[:12], "sequence": registry["sequence"]}))
PY

/usr/bin/sudo -n /bin/launchctl kickstart -k "system/$LABEL" >/dev/null 2>&1 || true
pid=$(/usr/bin/sudo -n /bin/launchctl print "system/$LABEL" 2>/dev/null | /usr/bin/awk '$1=="pid"{print $3;exit}')
running=$(/usr/bin/sudo -n /bin/ps -o comm= -p "${pid:-0}" 2>/dev/null)
if [ "$running" != "$target/darwin-arm/bin/brama" ]; then
    # launchd resolves the program path once, at bootstrap, and caches it: a
    # kickstart re-runs the OLD tree forever when the unit points through a
    # symlink that has since moved. The process world has to be rebuilt for the
    # new link to matter. bootout must be proven finished before bootstrap, or
    # launchctl answers 5 (Input/output error) and nothing is left running.
    printf 'launchd cached the old tree (%s); re-bootstrapping\n' "$running"
    /usr/bin/sudo -n /bin/launchctl bootout "system/$LABEL" >/dev/null 2>&1 || true
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12; do
        /usr/bin/sudo -n /bin/launchctl print "system/$LABEL" >/dev/null 2>&1 || break
        /bin/sleep 3
    done
    err=$(/usr/bin/sudo -n /bin/launchctl bootstrap system "/Library/LaunchDaemons/$LABEL.plist" 2>&1) || {
        printf 'bootstrap failed: %s\n' "$err" >&2
        exit 1
    }
    /usr/bin/sudo -n /bin/launchctl kickstart -k "system/$LABEL" >/dev/null 2>&1 || true
fi
health=000
for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16; do
    /bin/sleep 5
    health=$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 6 http://127.0.0.1:8080/healthz || true)
    case "$health" in 200|401|403) break ;; esac
done

pid=$(/usr/bin/sudo -n /bin/launchctl print "system/$LABEL" 2>/dev/null | /usr/bin/awk '$1=="pid"{print $3;exit}')
running=$(/usr/bin/sudo -n /bin/ps -o comm= -p "${pid:-0}" 2>/dev/null)
allowed=$(/usr/bin/sudo -n /bin/ps -Eww -o command= -p "${pid:-0}" 2>/dev/null \
    | /usr/bin/python3 -c 'import json,re,sys
blob=sys.stdin.read()
m=re.search(r"BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES=(\[\{.*?\}\])",blob)
allowed="?"
if m:
    for ident in json.loads(m.group(1)):
        if ident.get("client_id")=="weles":
            allowed=ident.get("allowed_models"); break
print(allowed)')

printf 'backup=%s healthz=%s pid=%s running=%s weles_allowed=%s\n' \
    "$REGISTRY.bak-$stamp" "$health" "${pid:-none}" "$running" "$allowed"
/usr/bin/tail -3 "$HOME/.stado/logs/brama-always-on.err" 2>/dev/null | /usr/bin/cut -c1-160
case "$health" in 200|401|403) exit 0 ;; *) exit 1 ;; esac
