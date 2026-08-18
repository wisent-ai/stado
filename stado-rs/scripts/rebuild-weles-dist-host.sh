#!/bin/sh
# Install the Weles worker `dist` built from published main, then restart the
# worker in place.
#
# The tree the units run is a delivered build with no sources and no compiler,
# so the build happens where the sources and node_modules are and arrives here
# as one checksummed `host install-file` payload. Weles is not release-managed
# in the registry and `release/current-production.json` is `uninitialized`, so
# there is no immutable release to promote for this host.
#
# The payload is unpacked beside the running build and only swapped in after it
# is verified to carry the model-alias fallback; the replaced build is kept, so
# going back is one rename.
set -u

R=/Users/charles/weles
SRC="$HOME/.stado/files/weles-dist-main.tgz"
STAGE="$R/.stado-dist-stage"
stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)

[ -f "$SRC" ] || { printf 'missing delivered dist payload: %s\n' "$SRC" >/dev/stderr; exit 1; }

printf 'alias_before=%s\n' \
  "$(/usr/bin/grep -m1 -o "WELES_AGENT_MODEL = '[a-z/]*'" "$R/dist/agent/jeden.js" 2>/dev/null)"

/bin/rm -rf "$STAGE"
/bin/mkdir -p "$STAGE"
/usr/bin/tar -xzf "$SRC" -C "$STAGE" || { printf 'unpack failed\n' >/dev/stderr; exit 1; }
[ -f "$STAGE/dist/agent/jeden.js" ] || { printf 'payload has no dist/agent/jeden.js\n' >/dev/stderr; exit 1; }

# The browser client asks Brama for `best` and for nothing else. A payload that
# names a second alias is refused here rather than discovered later in a run.
built=$(/usr/bin/grep -m1 -o "WELES_AGENT_MODEL = '[a-z/]*'" "$STAGE/dist/agent/jeden.js" 2>/dev/null)
printf 'alias_built=%s\n' "$built"
case "$built" in
  *"'best'"*) : ;;
  *) printf 'delivered dist does not ask for best; refusing to swap\n' >/dev/stderr; exit 1 ;;
esac
if /usr/bin/grep -q 'WELES_AGENT_FALLBACK_MODEL' "$STAGE/dist/agent/jeden.js"; then
  printf 'delivered dist carries a fallback alias; refusing to swap\n' >/dev/stderr
  exit 1
fi

/bin/mv "$R/dist" "$R/dist.before-$stamp"
/bin/mv "$STAGE/dist" "$R/dist"
/bin/rm -rf "$STAGE"
printf 'kept_previous=%s\n' "$R/dist.before-$stamp"

# `kickstart -k` restarts a loaded job: there is no window in which the unit
# does not exist, which an unload-then-bootstrap pair would open.
/usr/bin/sudo -n /bin/launchctl kickstart -k system/com.wisent.always-on.weles >/dev/null 2>&1 || true
/bin/sleep 20

printf 'alias_live=%s\n' \
  "$(/usr/bin/grep -m1 -o "WELES_AGENT_MODEL = '[a-z/]*'" "$R/dist/agent/jeden.js" 2>/dev/null)"
printf 'worker_pid=%s weles_api=%s\n' \
  "$(/usr/bin/sudo -n /bin/launchctl print system/com.wisent.always-on.weles 2>/dev/null | /usr/bin/awk '$1=="pid"{print $3;exit}')" \
  "$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 8 http://127.0.0.1:8788/healthz || true)"
