//! The shell program the host runs to describe its own mobile runtime.

/// Read every component off the host in one round trip.
///
/// One script rather than a probe per component: each round trip is an ssh
/// connection, and a five-connection read of one host's runtime is four
/// connections spent on formatting.
pub(super) const REMOTE_VERIFY_BODY: &str = r##"set -eu
LC_ALL=C
export LC_ALL
resolve() {
  for candidate in "$@"; do
    if [ -x "$candidate" ]; then printf '%s' "$candidate"; return 0; fi
  done
  printf ''
}
json_escape() {
  printf '%s' "$1" | /usr/bin/sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' | /usr/bin/tr -d '\n\r\t'
}
# The same candidate order deploy::host_exec probes, so the two readers cannot
# name different binaries on this machine.
appium=$(resolve @APPIUM_CANDIDATES@)
adb=$(resolve @ADB_CANDIDATES@)
# `appium` is a JavaScript shim whose first line is `#!/usr/bin/env node`, so
# on a channel whose PATH does not carry Node it answers `env: node: No such
# file or directory` -- present, runnable, and reported as broken. That is
# exactly what charless-mac-mini answered on the first repair: the binary was
# installed at ~/.npm-global/bin/appium and `--version` came back empty, so
# the runtime read `unknown`. The interpreter a shim needs is a sibling of the
# Node the fleet installs, which is the argument `host_exec::candidate_script`
# already makes for the same programs.
node_dir=''
for candidate in @NODE_CANDIDATES@; do
  if [ -x "$candidate" ]; then node_dir=$(/usr/bin/dirname "$candidate"); break; fi
done
if [ -n "$node_dir" ]; then
  PATH="$node_dir:$PATH"
  export PATH
fi
appium_version=''
drivers=''
warnings=''
if [ -n "$appium" ]; then
  appium_version=$("$appium" --version 2>/dev/null | /usr/bin/tr -d '\n\r' || printf '')
  # `driver list --installed` writes its table on stderr in some Appium
  # builds, so both streams are read and the names are matched out of it.
  listing=$("$appium" driver list --installed 2>&1 | /usr/bin/tr -d '\r' || printf '')
  drivers=$(printf '%s' "$listing" | /usr/bin/tr '\n' ' ')
  # The server's incompatibility verdicts, kept APART from the listing.
  # Joined into one blob they cannot be told apart, and the reader ends up
  # quoting a driver's whole table back at the operator as if it were the
  # sentence about one driver. One field per question.
  warnings=$(printf '%s' "$listing" | /usr/bin/grep -E 'may be incompatible' | /usr/bin/tr '\n' ' ' || printf '')
fi
adb_version=''
if [ -n "$adb" ]; then
  adb_version=$("$adb" version 2>/dev/null | /usr/bin/head -n 1 | /usr/bin/tr -d '\n\r' || printf '')
fi
printf '{"appium_path":"%s","appium_version":"%s","drivers":"%s","warnings":"%s","adb_path":"%s","adb_version":"%s"}\n' \
  "$(json_escape "$appium")" \
  "$(json_escape "$appium_version")" \
  "$(json_escape "$drivers")" \
  "$(json_escape "$warnings")" \
  "$(json_escape "$adb")" \
  "$(json_escape "$adb_version")"
"##;
