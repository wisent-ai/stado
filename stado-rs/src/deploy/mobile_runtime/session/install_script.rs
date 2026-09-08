//! The shell program the host runs to install the runtime it declared.

/// Install exactly what the declaration asks for, into the login user's home.
///
/// Every step is idempotent: `npm install -g` at a pinned version is a no-op
/// on a host already at it, `appium driver install` declines a driver already
/// present, and platform-tools is re-unpacked over its own tree. So a
/// provisioned host pays a no-op and a fresh one provisions itself, which is
/// the property [`crate::cli::release_submit`]'s toolchain provisioning argues
/// for.
pub(super) const REMOTE_REPAIR_BODY: &str = r##"set -eu
LC_ALL=C
export LC_ALL
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
# The reason, not the epilogue. npm ends a failed install with three lines
# naming its own log files, so a blind `tail` reports where the answer was
# written on a host nobody may read files on, and hides the answer. Prefer the
# lines that carry the diagnosis, and resort to the tail only when none do.
diagnose() {
  said=$(printf '%s' "$1" | /usr/bin/grep -E 'ERESOLVE|npm error' 2>/dev/null \
    | /usr/bin/grep -v -E '_logs|A complete log|For a full report' \
    | /usr/bin/head -n 8 | /usr/bin/tr '\n\t' '  ')
  if [ -z "$said" ]; then
    said=$(printf '%s' "$1" | /usr/bin/tail -n 3 | /usr/bin/tr '\n\t' '  ')
  fi
  printf '%s' "$said"
}
appium_version=$(printf '%s' '@APPIUM_B64@' | /usr/bin/base64 "$decode")
drivers=$(printf '%s' '@DRIVERS_B64@' | /usr/bin/base64 "$decode")
platform_tools=$(printf '%s' '@PLATFORM_TOOLS_B64@' | /usr/bin/base64 "$decode")
prefix="$HOME/.npm-global"
sdk="$HOME/Library/Android/sdk"
npm_bin=''
for candidate in /opt/homebrew/bin/npm /usr/local/bin/npm /usr/bin/npm; do
  if [ -x "$candidate" ]; then npm_bin="$candidate"; break; fi
done
if [ -n "$appium_version" ]; then
  if [ -z "$npm_bin" ]; then
    printf 'STADO_RUNTIME\tfailed\tappium: no npm on this host to install it with\n'
  else
    node_dir=$(/usr/bin/dirname "$npm_bin")
    PATH="$node_dir:$PATH"
    export PATH
    /bin/mkdir -p "$prefix"
    if out=$("$npm_bin" install --global --prefix "$prefix" "appium@$appium_version" 2>&1); then
      printf 'STADO_RUNTIME\tinstalled\tappium@%s\n' "$appium_version"
    else
      printf 'STADO_RUNTIME\tfailed\tappium@%s: %s\n' "$appium_version" "$(diagnose "$out")"
    fi
  fi
fi
appium_bin=''
for candidate in "$prefix/bin/appium" @APPIUM_CANDIDATES@; do
  if [ -x "$candidate" ]; then appium_bin="$candidate"; break; fi
done
# The declared server and the installed driver tree have to agree, and on this
# fleet they did not. `$APPIUM_HOME` outlives any one server: this host still
# carried `appium-mac2-driver@1.20.5`, whose peer is `appium@^2.4.1`, from an
# Appium 2 era, and npm resolves the whole extension tree at once -- so an
# undeclared driver nobody asked about refused every install into it with
# ERESOLVE. Updating the installed set is the conservative repair: it keeps
# every driver the host has, including ones this declaration says nothing
# about, and only moves them onto versions the declared server can host.
# Removing the blocker instead would be Stado deciding that a capability it
# was not asked about is expendable.
# The deadlock needs one, and only one, relaxed resolution. The blocker is a
# STALE pin: `appium-mac2-driver@1.20.5` demands `appium@^2.4.1`, and npm
# resolves the whole extension tree at once, so while it is in the tree even
# `driver update` cannot run -- the command that would remove the conflict is
# refused by the conflict. `NPM_CONFIG_LEGACY_PEER_DEPS` is set for the
# update alone, which lets the update land the CURRENT versions of every
# installed driver, all of which declare `appium@^3.0.0-rc.2`. The tree is
# then consistent on its own terms and the install that follows runs under
# ordinary resolution. Nothing is uninstalled: an undeclared driver is
# updated and kept, never removed, because deciding a capability nobody
# asked about is expendable is the operator's call and not this command's.
update_installed_drivers() {
  if [ "$drivers_updated" = "yes" ]; then return 0; fi
  drivers_updated=yes
  out=$(NPM_CONFIG_LEGACY_PEER_DEPS=true "$appium_bin" driver update installed --unsafe 2>&1) \
    || printf 'STADO_RUNTIME\tfailed\tdriver update: %s\n' "$(diagnose "$out")"
  # The claim is checked against the world before it is made. The first
  # version of this printed "updated" on a zero exit, and on
  # charless-mac-mini that zero exit sat beside a `mac2` still at 1.20.5 with
  # the server still calling it incompatible -- a report of a state nobody
  # had verified, which is the whole defect class this module exists to
  # avoid. So: re-read the server's own verdict and say what it says.
  after=$("$appium_bin" driver list --installed 2>&1 || printf '')
  case "$after" in
    *"potential problem"*)
      printf 'STADO_RUNTIME\tunresolved\tdriver update ran and the server still reports: %s\n' \
        "$(printf '%s' "$after" | /usr/bin/grep -E 'may be incompatible' | /usr/bin/head -n 3 | /usr/bin/tr '\n\t' '  ')"
      ;;
    *)
      printf 'STADO_RUNTIME\tupdated\tinstalled driver set; the server now reports no incompatible driver\n'
      ;;
  esac
}
drivers_updated=no
for driver in $drivers; do
  if [ -z "$appium_bin" ]; then
    printf 'STADO_RUNTIME\tfailed\tdriver %s: no appium on this host to install it into\n' "$driver"
    continue
  fi
  node_dir=$(/usr/bin/dirname "$npm_bin")
  PATH="$node_dir:$PATH"
  export PATH
  if "$appium_bin" driver list --installed 2>&1 | /usr/bin/grep -q -- "$driver"; then
    # Present, but presence is not agreement: a driver installed against an
    # older server is reported here and judged by the verify pass that
    # follows, which reads each driver's own version.
    printf 'STADO_RUNTIME\tpresent\tdriver %s\n' "$driver"
    continue
  fi
  if out=$("$appium_bin" driver install "$driver" 2>&1); then
    printf 'STADO_RUNTIME\tinstalled\tdriver %s\n' "$driver"
    continue
  fi
  case "$out" in
    *ERESOLVE*)
      update_installed_drivers
      if retry=$("$appium_bin" driver install "$driver" 2>&1); then
        printf 'STADO_RUNTIME\tinstalled\tdriver %s\n' "$driver"
      else
        printf 'STADO_RUNTIME\tfailed\tdriver %s: %s\n' "$driver" "$(diagnose "$retry")"
      fi
      ;;
    *)
      printf 'STADO_RUNTIME\tfailed\tdriver %s: %s\n' "$driver" "$(diagnose "$out")"
      ;;
  esac
done
# The server's own verdict on its driver tree, acted on rather than printed.
#
# Appium validates every driver in its manifest at startup and says so:
# `Driver "mac2" has 1 potential problem`. On charless-mac-mini that is a
# stale `mac2@1.20.5` beside a declared 3.7.0 server -- it never blocked THIS
# repair, because `uiautomator2` happened to install before it was reached,
# so nothing here would have noticed and the deadlock would have been waiting
# for whichever install came next. Reading the server's own complaint is not
# inference about npm's resolver; it is the declared authority on this tree
# stating that a driver disagrees with it, and the same conservative update
# answers it. Undeclared drivers are still only updated, never removed.
if [ -n "$appium_bin" ]; then
  verdict=$("$appium_bin" driver list --installed 2>&1 || printf '')
  case "$verdict" in
    *"potential problem"*)
      update_installed_drivers
      ;;
  esac
fi
if [ "$platform_tools" = "yes" ]; then
  if [ -x "$sdk/platform-tools/adb" ]; then
    printf 'STADO_RUNTIME\tpresent\tplatform-tools\n'
  else
    /bin/mkdir -p "$sdk"
    archive="$sdk/platform-tools-latest-darwin.zip"
    if [ "$(uname)" = "Linux" ]; then
      archive="$sdk/platform-tools-latest-linux.zip"
      url='https://dl.google.com/android/repository/platform-tools-latest-linux.zip'
    else
      url='https://dl.google.com/android/repository/platform-tools-latest-darwin.zip'
    fi
    /bin/rm -f "$archive"
    if /usr/bin/curl -fsS -o "$archive" "$url"; then
      if out=$(cd "$sdk" && /usr/bin/unzip -o -q "$archive" 2>&1); then
        printf 'STADO_RUNTIME\tinstalled\tplatform-tools\n'
      else
        printf 'STADO_RUNTIME\tfailed\tplatform-tools unpack: %s\n' "$(printf '%s' "$out" | /usr/bin/tail -n 2 | /usr/bin/tr '\n\t' '  ')"
      fi
      /bin/rm -f "$archive"
    else
      printf 'STADO_RUNTIME\tfailed\tplatform-tools: could not fetch %s\n' "$url"
    fi
  fi
fi
"##;
