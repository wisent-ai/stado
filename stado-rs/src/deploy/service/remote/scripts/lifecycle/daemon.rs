/// What the host reports about a system LaunchDaemon, read without
/// privilege and without touching anything.
///
/// Four facts, and each one is a gate on the only repair this channel can
/// perform:
///
/// - the unit's `KeepAlive` spelling, because ending a process nothing will
///   respawn turns a degraded control plane into a dead one;
/// - the account this login runs as;
/// - the pids running exactly the argv the unit declares, that THIS account
///   owns, which are the only ones an unprivileged signal can reach;
/// - the pids running it that some other account owns, so a refusal can say
///   whose process it is instead of just "no".
pub(crate) const DAEMON_PROBE_BODY: &str = "if [ ! -f \"$unit_path\" ]; then
  say 'missing' \"$unit_path\"
  exit 0
fi
daemon_argv=$(stado_unit_argv \"$unit_path\")
daemon_program=\"${daemon_argv%% *}\"
# `raw` answers the scalar spellings (`<true/>`, `<false/>`) in one word.
# A KeepAlive dict has no raw spelling and makes plutil fail, which reads
# identically to a key that is not there -- so the second read asks whether
# the key exists at all, and the third separates an unreadable plist from a
# readable one with no KeepAlive. Three answers, three different repairs.
daemon_keep='absent'
if daemon_raw=$(/usr/bin/plutil -extract KeepAlive raw -o - \"$unit_path\" 2>/dev/null); then
  daemon_keep=$(printf '%s' \"$daemon_raw\" | /usr/bin/tr -d ' \t\r\n')
elif /usr/bin/plutil -extract KeepAlive xml1 -o - \"$unit_path\" >/dev/null 2>&1; then
  daemon_keep='conditional'
elif ! /usr/bin/plutil -lint \"$unit_path\" >/dev/null 2>&1; then
  daemon_keep='unreadable'
fi
if [ -z \"$daemon_keep\" ]; then daemon_keep='unreadable'; fi
daemon_user=$(/usr/bin/id -un)
daemon_owned=''
daemon_foreign=''
# launchd's own answer first, where the domain can be read at all: the pid
# under the label is the one fact no pattern can widen. `sudo -n launchctl
# print system/<label>` is refused on this channel, so a system daemon is
# matched on the argv its unit declares -- never on the program alone, which
# on this fleet names every other service running the same binary.
stado_launchd_state
daemon_pids=\"$pc_pid\"
if [ -z \"$daemon_pids\" ]; then daemon_pids=$(stado_unit_pids \"$daemon_argv\"); fi
for daemon_pid in $daemon_pids; do
  daemon_owner=$(/bin/ps -o user= -p \"$daemon_pid\" 2>/dev/null | /usr/bin/tr -d ' \t\r\n')
  if [ \"$daemon_owner\" = \"$daemon_user\" ]; then
    daemon_owned=\"$daemon_owned$daemon_pid \"
  elif [ -n \"$daemon_owner\" ]; then
    daemon_foreign=\"$daemon_foreign$daemon_pid \"
  fi
done
printf 'STADO_DAEMON\\t%s\\t%s\\t%s\\t%s\\t%s\\n' \"$daemon_keep\" \"$daemon_user\" \"${daemon_owned% }\" \"${daemon_foreign% }\" \"$daemon_argv\"
say 'daemon_probed' \"KeepAlive $daemon_keep\"
";

/// End the daemon's process so launchd recreates it.
///
/// This is `launchctl kickstart -k` without the privilege: that verb stops
/// the job's process and lets launchd start it again, and for a job launchd
/// is unconditionally keeping alive, ending the process from the account
/// that owns it produces the same sequence. It never unloads anything, so
/// there is no window in which the job does not exist -- the property the
/// July outage cost this fleet three commands to learn.
///
/// Only the pids the probe found under THIS account are signalled, and they
/// arrive as a validated digit list from [`validate_pid_list`]; nothing here
/// re-derives a target from a pattern, because a pattern that widened by one
/// character would signal a process nobody chose.
///
/// TERM only, and no escalation. A control-plane daemon that ignores TERM is
/// a finding to report, not a reason to try SIGKILL on the process holding
/// the fleet's authorization state.
pub(crate) const DAEMON_TERM_BODY: &str = "daemon_argv=@ARGV@
daemon_before=@PIDS@
for daemon_pid in $daemon_before; do /bin/kill -TERM \"$daemon_pid\" >/dev/null 2>&1 || true; done
daemon_after=''
daemon_fresh=''
daemon_waited=0
while [ \"$daemon_waited\" -lt 15 ]; do
  /bin/sleep 1
  daemon_waited=$((daemon_waited + 1))
  daemon_after=$(stado_unit_pids \"$daemon_argv\")
  daemon_fresh=''
  for daemon_pid in $daemon_after; do
    case \" $daemon_before \" in
      *\" $daemon_pid \"*) ;;
      *) daemon_fresh=\"$daemon_fresh$daemon_pid \" ;;
    esac
  done
  if [ -n \"$daemon_fresh\" ]; then break; fi
done
daemon_left=''
for daemon_pid in $daemon_before; do
  case \" $daemon_after \" in
    *\" $daemon_pid \"*) daemon_left=\"$daemon_left$daemon_pid \" ;;
  esac
done
if [ -n \"$daemon_fresh\" ]; then
  say 'restarted' \"ended pid(s) $daemon_before owned by $(/usr/bin/id -un); launchd's KeepAlive replaced it with pid(s) ${daemon_fresh% } after ${daemon_waited}s\"
  exit 0
fi
if [ -n \"$daemon_left\" ]; then
  say 'restart_failed' \"pid(s) ${daemon_left% } did not end on SIGTERM and nothing was unloaded. Run: sudo launchctl kickstart -k system/$unit\"
  exit 0
fi
say 'restart_failed' \"ended pid(s) $daemon_before and launchd started nothing in ${daemon_waited}s. Run: sudo launchctl kickstart -k system/$unit\"
";
