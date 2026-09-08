use crate::deploy::service::*;

/// The three reads every body and every probe makes about one unit, in one
/// place: what the unit declares it runs, which processes are running exactly
/// that, and what launchd itself says about the label.
///
/// `stado_unit_pids` is the one that had to change. It used to be
/// `pgrep -f "^$program"` against the unit's program path, and on a host
/// where every Stado service runs one binary that pattern is every service:
/// on 2026-08-19 a unit-scoped `stado service restart
/// com.wisent.always-on.stado-object-api --host control-host` ended
/// eight processes — the object API, the host's resolver holding
/// 17600/17601/17612/17621, and a bare agent — because every one of them runs
/// `/Users/charles/.stado/bin/stado`, and it reported `restarted` with a met
/// postcondition afterwards. `KeepAlive` brought them back; one non-KeepAlive
/// sibling would have stayed down. The distinguishing fact is the argv the
/// unit declares (`dashboard --bind 127.0.0.1 --port 8765` against `resolver
/// serve --target <host>`), so the whole argv is matched, and where launchd
/// will answer for the label at all its own pid is preferred to any pattern.
pub(crate) const UNIT_STATE: &str = "stado_unit_argv() {
  if [ ! -f \"$1\" ]; then return 0; fi
  argv_read=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments' \"$1\" 2>/dev/null | /usr/bin/awk 'NR == 1 && $0 == \"Array {\" { next } $0 == \"}\" { next } { sub(/^[[:space:]]+/, \"\"); sub(/[[:space:]]+$/, \"\"); printf \"%s%s\", separator, $0; separator = \" \" }')
  if [ -z \"$argv_read\" ]; then
    argv_read=$(/usr/libexec/PlistBuddy -c 'Print :Program' \"$1\" 2>/dev/null)
    # PlistBuddy answers a missing key on stdout, so a program is accepted
    # only in the one shape a program has: an absolute path.
    case \"$argv_read\" in /*) ;; *) argv_read='' ;; esac
  fi
  printf '%s' \"$argv_read\"
}
stado_unit_pids() {
  pids_want=\"$1\"
  pids_program=\"${pids_want%% *}\"
  if [ -z \"$pids_program\" ]; then return 0; fi
  pids_args=''
  case \"$pids_want\" in *' '*) pids_args=\" ${pids_want#* }\" ;; esac
  # A unit that runs .../services/NAME/current/... names a link, and a process
  # that outlived a relink shows the version directory that link resolved to.
  # Both spellings share the service directory, so that is what the candidate
  # scan is widened to; the argv below is what decides.
  pids_root=\"$pids_program\"
  case \"$pids_program\" in */current/*) pids_root=\"${pids_program%%/current/*}/\" ;; esac
  pids_found=''
  for pids_pid in $(/usr/bin/pgrep -f \"^$pids_root\" 2>/dev/null); do
    pids_have=$(/bin/ps -p \"$pids_pid\" -o command= 2>/dev/null | /usr/bin/tr -s ' ' | /usr/bin/sed 's/^ //;s/ $//')
    if [ -z \"$pids_have\" ]; then continue; fi
    if [ \"$pids_have\" = \"$pids_want\" ]; then
      pids_found=\"$pids_found$pids_pid \"
      continue
    fi
    pids_have_args=''
    case \"$pids_have\" in *' '*) pids_have_args=\" ${pids_have#* }\" ;; esac
    if [ \"$pids_have_args\" != \"$pids_args\" ]; then continue; fi
    case \"${pids_have%% *}\" in \"$pids_root\"*) pids_found=\"$pids_found$pids_pid \" ;; esac
  done
  printf '%s' \"${pids_found% }\"
}
stado_launchd_state() {
  pc_pid=''
  if pc_info=$($launch print \"$domain/$unit\" 2>&1); then
    pc_loaded=yes
    pc_pid=$(printf '%s\\n' \"$pc_info\" | /usr/bin/awk '$1 == \"pid\" && $2 == \"=\" { print $3; exit }')
  else
    pc_loaded=no
  fi
}
";

/// What the prelude does on a Darwin host whose per-login launchd domain does
/// not exist at all, for every command that addresses an installed unit.
///
/// A restart, a stop or a retire aimed at a domain that is not there has
/// nothing to act on, and inventing one would mean installing a unit in the
/// middle of an operation that promised only to touch an existing one.
pub(crate) const NO_DOMAIN_REFUSE: &str = "    say 'no_launchd_domain' \"$domain_reason\"
    exit 66";

/// What [`ensure_service`] does instead: install into the system domain.
///
/// `launchctl bootstrap gui/$uid` over ssh answers `Could not switch to audit
/// session ... Operation not permitted`, and `stado service deploy` returned
/// that failure having installed nothing — which is how two `stado agent`
/// processes came to run for four days with no unit behind them. The system
/// domain is the one that does exist on an ssh login, so the unit that gets
/// installed is the daemon spelling of the same job, in
/// `/Library/LaunchDaemons`, and [`DOMAIN_RESOLVER`] then resolves every
/// later command to `system` from that path alone.
pub(crate) const NO_DOMAIN_SYSTEM: &str = "    domain=\"system\"
    domain_status='system'
    domain_reason='launchd has no per-login domain on this login, so the job is installed as a system LaunchDaemon instead'
    launch=\"/usr/bin/sudo -n /bin/launchctl\"
    unit_path=\"/Library/LaunchDaemons/$unit.plist\"";

// ---------------------------------------------------------------------------
// The end states the lifecycle operations intend
// ---------------------------------------------------------------------------

/// The end state a restart or a start intends.
///
/// Both halves are load-bearing. A unit can be loaded with nothing running
/// under it (launchd accepted the job and the program died on start), and a
/// program can be running with no unit loaded — that second one is what the
/// last-resort fallbacks in these scripts used to produce, and reporting it
/// as a successful restart is how an operator comes to believe a service is
/// under management when the next logout will end it.
pub(crate) const RUNNING_DESCRIBE: &str = "the unit is loaded and has a pid";

/// Read in the domain the action used, because that is the only domain whose
/// answer means anything: `no job at user/501/<label>` is a failure when the
/// action bootstrapped into `user/501` and says nothing at all about a job
/// the action never addressed.
pub(crate) const RUNNING_PROBE: &str = "  if [ \"$os\" = \"Darwin\" ]; then
    stado_launchd_state
    if [ \"$pc_loaded\" = no ]; then
      stado_post 'unmet' \"no job at $domain/$unit\"
    elif [ -n \"$pc_pid\" ]; then
      stado_post 'met' \"$domain/$unit pid $pc_pid\"
    else
      stado_post 'unmet' \"$domain/$unit is loaded with no pid\"
    fi
  elif stado_systemctl is-active --quiet \"$unit\"; then
    pc_pid=$(stado_systemctl show --property=MainPID --value \"$unit\" 2>/dev/null)
    if [ -n \"$pc_pid\" ] && [ \"$pc_pid\" != 0 ]; then
      stado_post 'met' \"$unit pid $pc_pid\"
    else
      stado_post 'unmet' \"$unit is active with no main pid\"
    fi
  else
    stado_post 'unmet' \"$unit is not active\"
  fi
";

/// The end state a stop intends.
///
/// A booted-out label is not the same fact as a stopped service: the sweep
/// these bodies run exists because a program started once outside its own
/// label survives every `bootout`, keeps the listening socket, and makes
/// every later start die on `address already in use`. So the probe asks
/// whether anything is running under the unit, not whether the label is
/// gone; a loaded job with no pid is a stopped service and says so.
pub(crate) const STOPPED_DESCRIBE: &str = "the unit is not running";

pub(crate) const STOPPED_PROBE: &str = "  if [ \"$os\" = \"Darwin\" ]; then
    # launchctl bootout returns before an exiting job disappears from
    # `launchctl print`. Wait for that declared end state instead of reporting
    # a failed stop that becomes true moments after the command returns.
    stopped_attempt=0
    while [ \"$stopped_attempt\" -lt 30 ]; do
      stado_launchd_state
      if [ \"$pc_loaded\" = no ] || [ -z \"$pc_pid\" ]; then break; fi
      stopped_attempt=$((stopped_attempt + 1))
      /bin/sleep 1
    done
    if [ \"$pc_loaded\" = no ]; then
      stado_post 'met' \"no job at $domain/$unit\"
    elif [ -n \"$pc_pid\" ]; then
      stado_post 'unmet' \"$domain/$unit still running as pid $pc_pid\"
    else
      stado_post 'met' \"$domain/$unit is loaded but not running\"
    fi
  else
    stopped_attempt=0
    while [ \"$stopped_attempt\" -lt 30 ]; do
      if ! stado_systemctl is-active --quiet \"$unit\"; then break; fi
      stopped_attempt=$((stopped_attempt + 1))
      /bin/sleep 1
    done
    if stado_systemctl is-active --quiet \"$unit\"; then
      stado_post 'unmet' \"$unit is still active\"
    else
      stado_post 'met' \"$unit is not active\"
    fi
  fi
";

/// One declared end state. The probe reads the host through the same prelude
/// vocabulary the body does — `$domain` above all — so the check cannot end
/// up asking about a domain the operation never acted in.
pub(crate) fn end_state(
    describe: &'static str,
    probe: &'static str,
) -> host_channel::PostCondition {
    host_channel::PostCondition {
        describe,
        probe: probe.to_string(),
    }
}

/// The end state an unprivileged restart of a system LaunchDaemon intends.
///
/// A system daemon's job lives in launchd's `system` domain, which an
/// unprivileged login cannot read: `launchctl print system/<label>` needs
/// root, and the `sudo -n` this channel would need is not granted. So
/// [`RUNNING_DESCRIBE`]'s two facts — a loaded job with a pid — are not
/// observable here at all, and asserting them would report every successful
/// restart of a daemon as a failure.
///
/// What IS observable without privilege is the process: it runs as the
/// approved user, so this login can see its pid and its owner. The end state
/// is therefore stated about the process, and it is the honest one for this
/// operation — the whole point of ending a `KeepAlive` daemon's process is
/// that launchd puts a NEW one in its place.
pub(crate) const RESPAWNED_DESCRIBE: &str = "the system daemon is running under a new pid";

/// Reads `daemon_argv` and `daemon_before`, which [`DAEMON_TERM_BODY`] sets.
/// The probe is armed as an `EXIT` trap in the body's own shell
/// (`host_channel::PostCondition::arm`), so it observes the pids that body
/// actually signalled rather than a second, racing observation of its own.
///
/// It asks about the pids running the unit's whole declared argv, not the
/// pids running its program: on a host where every service runs one binary
/// the second question answers with every service, and this probe reported a
/// met end state over the siblings a restart had ended.
pub(crate) const RESPAWNED_PROBE: &str = "  pc_now=$(stado_unit_pids \"${daemon_argv:-}\")
  pc_new=''
  for pc_pid in $pc_now; do
    case \" ${daemon_before:-} \" in
      *\" $pc_pid \"*) ;;
      *) pc_new=\"$pc_new$pc_pid \" ;;
    esac
  done
  if [ -n \"$pc_new\" ]; then
    stado_post 'met' \"$unit runs as pid(s) ${pc_new% }\"
  elif [ -n \"$pc_now\" ]; then
    stado_post 'unmet' \"$unit still runs as the pid(s) this restart ended: $pc_now\"
  else
    stado_post 'unmet' \"nothing runs the program of $unit; launchd did not respawn it\"
  fi
";
