// ---------------------------------------------------------------------------
// The fixed remote programs
// ---------------------------------------------------------------------------

/// Shared head of every remote program: identify the OS, resolve the
/// launchd domain the way the recovery script does, derive the unit path
/// when the caller did not declare one, and define the marker emitter.
///
/// `say` flattens its detail exactly like `host_recovery`'s script (`tr`
/// over tab/CR/LF, then `cut`) so one marker can never span two lines and
/// desynchronise the parser. Written as an escaped string, not a raw
/// string, for the same reason the recovery script is.
pub(crate) const REMOTE_PRELUDE: &str = "set -u
unit=@UNIT@
linux_unit=@LINUX_UNIT@
unit_path=\"@PATH@\"
os=$(/usr/bin/uname -s)
uid=$(/usr/bin/id -u)
gui=\"gui/$uid\"
user_domain=\"user/$uid\"
domain=\"\"
domain_status=\"\"
domain_reason=\"\"
launch=/bin/launchctl
say() {
  detail=$(printf '%s' \"$2\" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-400)
  printf 'STADO_SERVICE\\t%s\\t%s\\t%s\\n' \"$unit\" \"$1\" \"$detail\"
}
@DOMAIN_RESOLVER@@UNIT_STATE@if [ \"$os\" = \"Darwin\" ]; then
  # The file first, the domain second. An unqualified label may name this
  # login's agent or a system daemon, and which domain the unit belongs to
  # follows from the file -- so resolving a domain before knowing which file
  # this is, and patching it afterwards, is how one command came to act in one
  # domain, probe another, and report a third. The search covers
  # /Library/LaunchDaemons as well as this login's LaunchAgents because
  # adoption used to look only in the second and reported a running always-on
  # daemon as absent.
  if [ -z \"$unit_path\" ]; then
    if [ -f \"$HOME/Library/LaunchAgents/$unit.plist\" ]; then
      unit_path=\"$HOME/Library/LaunchAgents/$unit.plist\"
    elif [ -f \"/Library/LaunchDaemons/$unit.plist\" ]; then
      unit_path=\"/Library/LaunchDaemons/$unit.plist\"
    else
      unit_path=\"$HOME/Library/LaunchAgents/$unit.plist\"
    fi
  fi
  if ! stado_domain_of \"$unit_path\"; then
@NO_DOMAIN@
  fi
@OBSERVED_DOMAIN@
elif [ \"$os\" = \"Linux\" ]; then
  # The same search the Darwin branch above makes, for the same reason it was
  # widened: adoption looked only at this login's user units and reported a
  # running system unit as absent. On 2026-09-03 that unit was
  # `wisent-compute-agent.service` on the fleet's only linux-amd64 builder --
  # loaded, running a stado image that refuses today's registry document, and
  # so unmanaged that nothing could cycle it while every linux release build
  # queued behind it.
  if [ -n \"$linux_unit\" ]; then unit=\"$linux_unit\"; fi
  if [ -z \"$unit_path\" ]; then
    if [ -f \"$HOME/.config/systemd/user/$unit\" ]; then
      unit_path=\"$HOME/.config/systemd/user/$unit\"
    elif [ -f \"/etc/systemd/system/$unit\" ]; then
      unit_path=\"/etc/systemd/system/$unit\"
    else
      unit_path=\"$HOME/.config/systemd/user/$unit\"
    fi
  fi
  case \"$unit_path\" in
    /etc/systemd/system/*) scope=system ;;
    *) scope=user ;;
  esac
  domain=\"$scope\"
  if [ \"$scope\" = \"system\" ]; then
    systemd_detail='systemd system scope'
  else
    systemd_detail='systemd --user'
  fi
  case \"$unit_path\" in
    */.config/systemd/user/*) owner_path=\"${unit_path%%/.config/systemd/user/*}\" ;;
    *) owner_path=\"$unit_path\" ;;
  esac
  while [ ! -e \"$owner_path\" ] && [ \"$owner_path\" != \"/\" ]; do
    owner_path=$(/usr/bin/dirname \"$owner_path\")
  done
  service_user=$(/usr/bin/stat -c %U \"$owner_path\")
  service_uid=$(/usr/bin/id -u \"$service_user\")
  if [ -x /usr/bin/sudo ]; then sudo_bin=/usr/bin/sudo; else sudo_bin=/bin/sudo; fi
  stado_root() {
    if [ \"$uid\" = \"0\" ]; then
      \"$@\"
      return
    fi
    \"$sudo_bin\" -n \"$@\"
  }
  stado_systemctl() {
    # A system unit is root's job and has no per-user bus: addressing it with
    # `--user` is what made every verb in this module answer \"not present\"
    # for a unit the host was plainly running.
    if [ \"$scope\" = \"system\" ]; then
      stado_root /usr/bin/systemctl \"$@\"
      return
    fi
    runtime=\"/run/user/$service_uid\"
    if [ \"$service_uid\" = \"$uid\" ]; then
      /usr/bin/env \
        XDG_RUNTIME_DIR=\"$runtime\" \
        DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" \
        /usr/bin/systemctl --user \"$@\"
      return
    fi
    \"$sudo_bin\" -n -u \"$service_user\" /usr/bin/env \
      XDG_RUNTIME_DIR=\"$runtime\" \
      DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" \
      /usr/bin/systemctl --user \"$@\"
  }
else
  say 'unsupported_os' \"$os\"
  exit 65
fi
printf 'STADO_HOST\\t%s\\t%s\\t%s\\t%s\\n' \"$os\" \"$domain\" \"$unit\" \"$unit_path\"
if [ \"$os\" = \"Darwin\" ]; then
  printf 'STADO_DOMAIN\\t%s\\t%s\\t%s\\n' \"$domain\" \"$domain_status\" \"$(printf '%s' \"$domain_reason\" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-400)\"
fi
";

/// The one answer to "which launchd domain does this unit belong to", read by
/// every program in this module and by `host_recovery`'s recovery pass.
///
/// Three domains, and the difference between the last two is the defect this
/// function exists for:
///
/// - `/Library/LaunchDaemons/...` is root's job: the `system` domain, reached
///   with sudo.
/// - A LaunchAgent of a user who has a graphical session lives in
///   `gui/<uid>`, and an ssh login can address that domain while the session
///   exists.
/// - A LaunchAgent of a user who has none has only the background per-user
///   domain `user/<uid>` — the domain an ssh login is itself placed in, and
///   the one an agent that needs the login session cannot be loaded into.
///
/// The graphical session is read the way macOS exposes it, and the check was
/// chosen against the live host rather than guessed: `/dev/console` is owned
/// by the user holding the graphical session and by root at the login window,
/// and launchd has a `gui/<uid>` domain only while that session exists. Both
/// halves are required, so the reported domain is one the next `launchctl`
/// verb can actually address.
///
/// What that read answers on control-host on 2026-08-19, through
/// `stado host exec` (read-only, allowlisted): `who` prints nothing,
/// `loginwindow` runs as root, no `Dock`, `Finder` or `SystemUIServer`
/// process exists for any account, and the login's own `launchctl list`
/// holds 62 background `com.apple.*` agents and no `com.wisent.*` label.
/// Nobody is logged in graphically there, so `gui/501` does not exist, and
/// the honest answer for that host's agent is the `user/501` fallback —
/// reported as the reason the agent cannot be loaded instead of papered over
/// with a bare process.
///
/// Sets `$domain` (what every verb addresses and every probe reads),
/// `$domain_status` ([`DOMAIN_STATUS_SYSTEM`], [`DOMAIN_STATUS_GRAPHICAL`],
/// [`DOMAIN_STATUS_BACKGROUND`] or [`DOMAIN_STATUS_UNAVAILABLE`]),
/// `$domain_reason` (the operator's sentence for that choice) and `$launch`
/// (the launchctl this domain needs). Returns non-zero only when launchd has
/// no per-login domain at all, which is the one case a caller may answer
/// differently.
pub const DOMAIN_RESOLVER: &str = "stado_domain_of() {
  domain=\"\"
  domain_status=\"\"
  domain_reason=\"\"
  launch=/bin/launchctl
  case \"$1\" in
    /Library/LaunchDaemons/*)
      domain='system'
      domain_status='system'
      domain_reason='a unit in /Library/LaunchDaemons is a system LaunchDaemon, so its job belongs to the system domain and loading it needs root'
      launch=\"/usr/bin/sudo -n /bin/launchctl\"
      return 0
      ;;
  esac
  account=$(/usr/bin/id -un)
  console=$(/usr/bin/stat -f%Su /dev/console 2>/dev/null | /usr/bin/tr -d ' \t\r\n')
  if [ -z \"$console\" ]; then console='nobody'; fi
  if [ \"$console\" = \"$account\" ] && /bin/launchctl print \"$gui\" >/dev/null 2>&1; then
    domain=\"$gui\"
    domain_status='graphical'
    domain_reason=\"$account owns /dev/console and launchd has $gui, so a LaunchAgent of this login loads there\"
    return 0
  fi
  if /bin/launchctl print \"$user_domain\" >/dev/null 2>&1; then
    domain=\"$user_domain\"
    domain_status='background'
    domain_reason=\"/dev/console belongs to $console, not $account: no graphical session, so $gui does not exist and a LaunchAgent has only the background domain $user_domain\"
    return 0
  fi
  domain_status='unavailable'
  domain_reason=\"launchd has neither $gui nor $user_domain for $account\"
  return 1
}
";
