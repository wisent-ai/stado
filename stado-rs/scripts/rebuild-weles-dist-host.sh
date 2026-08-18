#!/bin/sh
# Rebuild the Weles worker's delivered `dist` from published main.
#
# The tree the units run is a delivered build with no sources, and its
# `dist/agent/jeden.js` still carries the pre-`best` model alias, so every
# browser job dies in Brama with `403 authorization_error`. Weles is not
# release-managed in the registry, and the production pointer in
# `release/current-production.json` is `uninitialized`, so there is no
# immutable release to promote here: the sources arrive as one checksummed
# `host install-file` payload and are compiled against the node_modules the
# tree already carries.
#
# The build runs in a staging directory. A failed compile therefore leaves the
# running `dist` untouched, and a successful one is swapped in beside a kept
# copy of what it replaced, so the previous build is one rename away.
set -u

R=/Users/charles/weles
SRC="$HOME/.stado/files/weles-src-main.tgz"
NPM=/opt/homebrew/bin/npm
NODE_DIR=/opt/homebrew/bin
STAGE="$R/.stado-rebuild"
stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)

[ -f "$SRC" ] || { printf 'missing delivered sources: %s\n' "$SRC" >/dev/stderr; exit 1; }
[ -x "$NPM" ] || { printf 'missing npm: %s\n' "$NPM" >/dev/stderr; exit 1; }
[ -d "$R/node_modules" ] || { printf 'delivered tree has no node_modules\n' >/dev/stderr; exit 1; }

printf 'alias_before=%s\n' \
  "$(/usr/bin/grep -m1 -o "WELES_AGENT_MODEL = '[a-z/]*'" "$R/dist/agent/jeden.js" 2>/dev/null)"

/bin/rm -rf "$STAGE"
/bin/mkdir -p "$STAGE"
/usr/bin/tar -xzf "$SRC" -C "$STAGE" || { printf 'unpack failed\n' >/dev/stderr; exit 1; }

# node_modules and the compiler config come from the tree that already runs;
# only src/ and the build script come from main.
/bin/ln -s "$R/node_modules" "$STAGE/node_modules"
PATH="$NODE_DIR:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH
( cd "$STAGE" && "$NPM" run build >"$STAGE/build.log" 2>&1 )
rc=$?
printf 'build_rc=%s\n' "$rc"
if [ "$rc" -ne 0 ]; then
  printf '== build log tail ==\n'
  /usr/bin/tail -25 "$STAGE/build.log" | /usr/bin/cut -c1-200
  exit 1
fi

built_alias=$(/usr/bin/grep -m1 -o "WELES_AGENT_MODEL = '[a-z/]*'" "$STAGE/dist/agent/jeden.js" 2>/dev/null)
printf 'alias_built=%s\n' "$built_alias"
case "$built_alias" in
  *"'best'"*) : ;;
  *) printf 'built dist does not carry the best alias; refusing to swap\n' >/dev/stderr; exit 1 ;;
esac

/bin/mv "$R/dist" "$R/dist.before-$stamp"
/bin/mv "$STAGE/dist" "$R/dist"
printf 'kept_previous=%s\n' "$R/dist.before-$stamp"

for L in com.wisent.always-on.weles com.wisent.always-on.weles-api; do
  /usr/bin/sudo -n /bin/launchctl kickstart -k "system/$L" >/dev/null 2>&1 \
    || /bin/launchctl kickstart -k "gui/$(/usr/bin/id -u)/$L" >/dev/null 2>&1 || true
done
/bin/sleep 20

printf 'alias_live=%s\n' \
  "$(/usr/bin/grep -m1 -o "WELES_AGENT_MODEL = '[a-z/]*'" "$R/dist/agent/jeden.js" 2>/dev/null)"
printf 'weles_api=%s\n' \
  "$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 8 http://127.0.0.1:8788/healthz || true)"
/bin/rm -rf "$STAGE"
