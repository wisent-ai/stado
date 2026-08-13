#!/bin/sh
# Report the non-secret environment the control-plane process was started with.
#
# Read-only, and deliberately narrow: a process that has to be restarted must
# come back with the same wiring, and guessing that wiring is how a control
# plane comes back pointing at the wrong store. Only keys that name endpoints,
# paths and modes are printed; anything whose name suggests a secret VALUE is
# reported as present without its value. A `*_TOKEN_FILE` is a path, not a
# secret, and is printed so the restart can be verified against it.
set -eu
# The helper takes no operator words -- `run-helper` passes UUIDs only -- so it
# resolves the control-plane listener itself.
pid=$(
  /usr/sbin/lsof -nP -iTCP:8765 -sTCP:LISTEN -Fpc 2>/dev/null \
    | /usr/bin/awk '/^p/ {pid=substr($0,2)} /^cstado$/ {print pid; exit}'
)
[ -n "$pid" ] || {
  printf '%s\n' "no stado listener on the control-plane port" >&2
  exit 1
}

/bin/ps eww -p "$pid" -o command= 2>/dev/null \
  | /usr/bin/tr ' ' '\n' \
  | /usr/bin/grep -E '^(WC_|STADO_|SKARBIEC_)[A-Z0-9_]+=' \
  | while IFS='=' read -r name value; do
      case "$name" in
        *TOKEN_FILE|*CONFIG|*PATH|*URL|*BACKEND|*CONSUMER|*NAMESPACE|*BIND|*PORT|*CA_FILE)
          printf '%s=%s\n' "$name" "$value"
          ;;
        *TOKEN|*SECRET|*KEY|*PASSWORD)
          printf '%s=[REDACTED]\n' "$name"
          ;;
        *)
          printf '%s=%s\n' "$name" "$value"
          ;;
      esac
    done
