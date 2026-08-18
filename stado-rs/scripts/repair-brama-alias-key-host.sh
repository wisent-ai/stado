#!/bin/sh
# Repair the corrupted Brama alias key that keeps the gateway crash-looping.
#
# Brama validates its control config against a closed alias set and refuses to
# start on any difference:
#   services.brama.allowed_models must contain the exact closed Brama alias set
# The set requires `best`; the control config carries `-best`, in both
# `allowed_models` and `model_aliases`, with an otherwise correct route
# (`codex/gpt-5.3-codex-spark`). One stray hyphen therefore takes down every
# model call in the company: Weles browser automation, Wisent Backend chat,
# evaluation, embeddings and moderation all route through this gateway.
#
# Brama had been serving with an older configuration already loaded, so the fault
# stayed invisible until the unit restarted and could not come back.
#
# Renames the key in place, keeps a timestamped backup, restarts in place, and
# reports whether the gateway answers again.
set -eu

CONTROL=${BRAMA_CONTROL_CONFIG:-$HOME/.stado/brama-28b-control.json}
LABEL=com.wisent.always-on.brama
WRONG=-best
RIGHT=best

[ -f "$CONTROL" ] || { printf '%s\n' "missing $CONTROL" >&2; exit 1; }

stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
/bin/cp -p "$CONTROL" "$CONTROL.bak-$stamp"

CONTROL="$CONTROL" WRONG="$WRONG" RIGHT="$RIGHT" /usr/bin/python3 - <<'PY'
import json
import os

path = os.environ["CONTROL"]
wrong = os.environ["WRONG"]
right = os.environ["RIGHT"]
with open(path, encoding="utf-8") as source:
    document = json.load(source)
policy = document["services"]["brama"]

allowed = policy.get("allowed_models")
if isinstance(allowed, list) and wrong in allowed:
    policy["allowed_models"] = [right if value == wrong else value for value in allowed]

aliases = policy.get("model_aliases")
if isinstance(aliases, dict) and wrong in aliases:
    aliases[right] = aliases.pop(wrong)

with open(path, "w", encoding="utf-8") as target:
    json.dump(document, target, indent=2, sort_keys=False)
    target.write("\n")

print(json.dumps({
    "allowed_models": policy.get("allowed_models"),
    "alias_best": (policy.get("model_aliases") or {}).get(right),
}, separators=(",", ":")))
PY

/usr/bin/sudo -n /bin/launchctl kickstart -k "system/$LABEL" >/dev/null 2>&1 || true
attached=no
for _ in 1 2 3 4 5 6 7 8 9 10; do
    /bin/sleep 5
    code=$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 6 http://127.0.0.1:8080/healthz || true)
    case "$code" in
        200|401|403) attached="$code"; break ;;
    esac
done
pid=$(/usr/bin/sudo -n /bin/launchctl print "system/$LABEL" 2>/dev/null | /usr/bin/awk '$1=="pid"{print $3; exit}')
printf 'backup=%s healthz=%s pid=%s\n' "$CONTROL.bak-$stamp" "$attached" "${pid:-none}"
/usr/bin/tail -3 "$HOME/.stado/logs/brama-always-on.err" 2>/dev/null || true
[ "$attached" != no ]
