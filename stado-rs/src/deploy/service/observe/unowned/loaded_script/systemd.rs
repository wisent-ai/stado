//! Read-only on Linux: every service unit file in the two directories this
//! fleet installs into (`/etc/systemd/system` and the account's
//! `~/.config/systemd/user`), with what its manager says about it and what
//! its live process runs, as the same `STADO_LOADED` rows the launchd read
//! answers. It starts nothing, stops nothing and needs no sudo.
//!
//! Without it a Linux host answered that it holds no unit at all, so a unit
//! there that a product's one process replaced could never be found, and a
//! stray copy could never be reported.

/// One `STADO_LOADED` row per fleet unit file, then exit; expects `details`
/// to be set. Runs only on Linux for a full read; every other case falls
/// through to the launchd read, which names an unsupported system itself.
pub(crate) const SYSTEMD_UNITS_SCRIPT: &str = r##"set -u
if [ "$(/usr/bin/uname -s)" = Linux ] && [ "$details" = full ]; then
  uid=$(/usr/bin/id -u)
  for dir in /etc/systemd/system "$HOME/.config/systemd/user"; do
    [ -d "$dir" ] || continue
    if [ "$dir" = /etc/systemd/system ]; then scope=system; else scope=user; fi
    for file in "$dir"/*.service; do
      # A symlink here is a masked unit or an alias of a distribution unit,
      # not a unit file this fleet wrote.
      [ -f "$file" ] && [ ! -L "$file" ] || continue
      label=${file##*/}
      if [ "$scope" = system ]; then
        show=$(/usr/bin/systemctl show -p MainPID -p ExecMainStatus -p NRestarts -p LoadState -- "$label" 2>/dev/null)
      else
        show=$(/usr/bin/env XDG_RUNTIME_DIR="/run/user/$uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$uid/bus" \
          /usr/bin/systemctl --user show -p MainPID -p ExecMainStatus -p NRestarts -p LoadState -- "$label" 2>/dev/null)
      fi
      pid=$(printf '%s\n' "$show" | /usr/bin/sed -n 's/^MainPID=//p')
      [ "$pid" = 0 ] && pid=''
      exited=$(printf '%s\n' "$show" | /usr/bin/sed -n 's/^ExecMainStatus=//p')
      runs=$(printf '%s\n' "$show" | /usr/bin/sed -n 's/^NRestarts=//p')
      loaded=''
      if [ "$(printf '%s\n' "$show" | /usr/bin/sed -n 's/^LoadState=//p')" = loaded ]; then loaded=$scope; fi
      program=$(/usr/bin/sed -n 's/^ExecStart=[-@+!:]*//p' "$file" | /usr/bin/head -n 1 | /usr/bin/tr '\t\r\n' '   ')
      running=''
      started=''
      written=''
      case "$pid" in
        ''|*[!0-9]*) ;;
        *)
          running=$(/usr/bin/tr '\000' ' ' < "/proc/$pid/cmdline" 2>/dev/null)
          started=$(/usr/bin/stat -c %Y "/proc/$pid" 2>/dev/null)
          image=$(/usr/bin/readlink "/proc/$pid/exe" 2>/dev/null)
          if [ -f "$image" ]; then written=$(/usr/bin/stat -c %Y "$image" 2>/dev/null); fi
          ;;
      esac
      # The launch column is `unread` here: systemd reports a successful
      # ExecMainStatus for a main process that has not exited yet, so "ended
      # by itself, successfully" cannot be read from it, and NRestarts already
      # counts only the restarts systemd made on its own.
      [ -n "$exited" ] || exited=-
      [ -n "$program" ] || program=-
      [ -n "$running" ] || running=-
      [ -n "$started" ] || started=-
      [ -n "$written" ] || written=-
      [ -n "$loaded" ] || loaded=-
      [ -n "$runs" ] || runs=-
      printf 'STADO_LOADED\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$pid" "$exited" "$label" "$file" "$program" "$file" "$running" "$started" "$written" fleet-directory "$loaded" "$runs" "$exited" - - - unread
    done
  done
  exit 0
fi
"##;
