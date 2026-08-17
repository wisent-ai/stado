#!/bin/sh
# Print the resolver policy this host's registry copy declares for one target.
#
# The service directory names one authority target, and every other host reads
# the registry through it. When two copies disagree, a resolver binds ports its
# own machine's copy never mentions, and the failure names a port that cannot be
# found in the registry anyone reads locally. Comparing the two copies is the
# only way to see that.
set -eu
TARGET="${1:-lukasz-macbook}"
"$HOME/.stado/bin/stado" registry pull 2>/dev/null | /usr/bin/python3 -c '
import json, sys

target = sys.argv[1]
document = json.load(sys.stdin)
found = next((t for t in (document.get("targets") or []) if t.get("name") == target), None)
if not found:
    print(target + ": not in this copy of the registry")
    raise SystemExit(0)
policy = found.get("service_resolver") or {}
print(target + " api_bind: " + str(policy.get("api_bind")))
for adapter in policy.get("adapters") or []:
    bind = str(adapter.get("bind"))
    service = str(adapter.get("service"))
    consumer = str(adapter.get("consumer"))
    print("  " + bind + "  " + service + "  consumer=" + consumer)
directory = document.get("service_directory") or {}
print("authority: " + str((directory.get("authority") or {}).get("target")))
print("generation: " + str(directory.get("generation")))
' "$TARGET"
