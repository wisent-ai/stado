//! The two programs that run on the host: one reads the cache, the other
//! completes it.

/// The remote program that reports which cache directories are complete.
///
/// One `test -f` per marker, and nothing else: this reads presence and never
/// downloads, so a verify pass on a healthy host costs one round trip and
/// changes nothing.
pub(super) const REMOTE_VERIFY_BODY: &str = r##"set -eu
LC_ALL=C
export LC_ALL
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
markers=$(printf '%s' '@MARKERS_B64@' | /usr/bin/base64 "$decode")
printf '{"components":['
first=1
printf '%s\n' "$markers" | while IFS= read -r entry; do
  [ -n "$entry" ] || continue
  name=${entry%%|*}
  path=${entry#*|}
  case "$path" in
    '$HOME'/*) resolved="$HOME/${path#\$HOME/}" ;;
    *) resolved="$path" ;;
  esac
  if [ -f "$resolved" ]; then state=present; else state=missing; fi
  if [ "$first" = 1 ]; then first=0; else printf ','; fi
  printf '{"name":"%s","path":"%s","state":"%s"}' "$name" "$resolved" "$state"
done
printf ']}\n'
"##;

/// The remote program that completes the runtime.
///
/// `npx playwright install <component>` run from the installed release, so the
/// Playwright that resolves the download is the one the release pins rather
/// than whatever a global npm happens to hold. The node that runs it is the
/// worker's own.
pub(super) const REMOTE_REPAIR_BODY: &str = r##"set -eu
LC_ALL=C
export LC_ALL
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
components=$(printf '%s' '@COMPONENTS_B64@' | /usr/bin/base64 "$decode")
release="$HOME/weles"
if [ ! -d "$release" ]; then
  printf 'STADO_RUNTIME\tfailed\tno release checkout at %s\n' "$release"
  exit 0
fi
node_bin=""
for candidate in /opt/homebrew/bin/node /usr/local/bin/node /usr/bin/node; do
  if [ -x "$candidate" ]; then node_bin="$candidate"; break; fi
done
if [ -z "$node_bin" ]; then
  printf 'STADO_RUNTIME\tfailed\tno node on this host to run the installer\n'
  exit 0
fi
cli="$release/node_modules/playwright-core/cli.js"
if [ ! -f "$cli" ]; then
  printf 'STADO_RUNTIME\tfailed\tthe release carries no playwright-core cli at %s\n' "$cli"
  exit 0
fi
cd "$release"
for component in $components; do
  if out=$("$node_bin" "$cli" install "$component" 2>&1); then
    printf 'STADO_RUNTIME\tinstalled\t%s\n' "$component"
  else
    printf 'STADO_RUNTIME\tfailed\t%s: %s\n' "$component" "$(printf '%s' "$out" | /usr/bin/tail -n 3 | /usr/bin/tr '\n\t' '  ')"
  fi
done
"##;
