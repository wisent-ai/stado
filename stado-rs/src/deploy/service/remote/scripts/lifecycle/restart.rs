/// `service restart`: restart the unit, in place wherever launchd will allow it.
/// Deliberately narrower than a recovery pass — no disk cleanup, no
/// coordinator teardown, no other agents touched.
///
/// Order matters more than it looks. This used to `bootout` first and then
/// `bootstrap` the unit file back, which recreates the job rather than
/// restarting it. When launchd still holds children of the old job the
/// bootstrap fails, and the command returned `restart_failed` with the unit
/// left *unloaded* — a partial failure strictly worse than never having run
/// the restart, because the listeners it owned are gone and there is nothing
/// to roll back to. A control-plane primitive whose failure mode is an outage
/// will eventually cause one; this one did, on the always-on host.
///
/// So a loaded job is kicked in place first: `kickstart -k` never unloads, so
/// there is no window in which the job does not exist and nothing orphaned to
/// sweep. The unload-and-recreate path remains for a unit that is not loaded
/// at all, or whose in-place kick fails.
///
/// What is deliberately NOT here any more is everything that used to happen
/// after the bootstrap failed: a second `launchctl asuser` attempt, a
/// `launchctl submit` of a `<label>-recovery` job, and finally a `perl`-exec
/// of the unit's argv in the background, reported as
/// `restarted: direct process <pid>`. On 2026-08-19 that last line is what
/// `stado service restart com.wisent.compute.service.stado-agent-mini --host
/// control-host` returned, beside `postcondition unmet: no job at
/// user/501/com.wisent.compute.service.stado-agent-mini` — a bare process
/// under the ssh session, no unit behind it, and a report an operator read as
/// success. A process that dies with the login that spawned it is not a
/// restarted service, so a bootstrap that leaves no job in the domain the
/// restart used is [`STATUS_NOT_LOADED`]: the domain, launchd's own words and
/// the reason, and nothing started outside launchd.
pub(crate) const RESTART_BODY: &str = "if [ \"$os\" = \"Darwin\" ]; then
  if [ \"${stado_reload_unit:-0}\" != 1 ] && $launch print \"$domain/$unit\" >/dev/null 2>&1; then
    # An in-place kick re-execs the argv launchd already holds. It cannot
    # apply a unit file whose program or arguments have changed, and it
    # reports success either way -- which is how two restarts and an ensure
    # of com.wisent.compute.service.stado-local-control-plane on 2026-09-03
    # all said `restarted` while the job kept executing the shared global
    # binary the plist no longer named. A silent no-op is the worst available
    # answer, so the two vectors are compared first and a job whose argv has
    # drifted from its file goes to the unload-and-bootstrap path below, which
    # is the only one that can carry the change.
    loaded_argv=$($launch print \"$domain/$unit\" 2>/dev/null | /usr/bin/awk '
      /^[ \\t]*arguments[ \\t]*=[ \\t]*\\{/ { collecting=1; argv=\"\"; next }
      collecting && /^[ \\t]*\\}/ { collecting=0; sub(/^ /, \"\", argv); print argv; exit }
      collecting { line=$0; sub(/^[ \\t]+/, \"\", line); argv=argv \" \" line }
    ')
    file_argv=\"\"
    if [ -f \"$unit_path\" ]; then
      file_argv=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments' \"$unit_path\" 2>/dev/null | /usr/bin/sed -e '1d' -e '$d' -e 's/^ *//' | /usr/bin/grep -v '^$' | /usr/bin/tr '\\n' ' ')
      file_argv=$(printf '%s' \"$file_argv\" | /usr/bin/sed -e 's/ *$//')
      [ -n \"$file_argv\" ] || file_argv=$(/usr/libexec/PlistBuddy -c 'Print :Program' \"$unit_path\" 2>/dev/null)
    fi
    loaded_argv=$(printf '%s' \"$loaded_argv\" | /usr/bin/sed -e 's/^ *//' -e 's/ *$//')
    if [ -z \"$file_argv\" ] || [ \"$loaded_argv\" = \"$file_argv\" ]; then\n      detail=$($launch kickstart -k \"$domain/$unit\" 2>&1)\n      rc=$?\n      if [ \"$rc\" -eq 0 ]; then\n        say 'restarted' \"$domain in place\"\n        exit 0\n      fi\n    fi\n  fi
  if [ ! -f \"$unit_path\" ]; then
    $launch enable \"$domain/$unit\" >/dev/null 2>&1 || true
    detail=$($launch kickstart -k \"$domain/$unit\" 2>&1)
    rc=$?
    if [ \"$rc\" -eq 0 ]; then say 'restarted' \"$domain\"; else say 'restart_failed' \"$rc $detail\"; fi
    exit 0
  fi
  $launch bootout \"$domain/$unit\" >/dev/null 2>&1 || true
@DISOWNED_SWEEP@
  $launch enable \"$domain/$unit\" >/dev/null 2>&1 || true
  detail=$($launch bootstrap \"$domain\" \"$unit_path\" 2>&1)
  rc=$?
  if ! $launch print \"$domain/$unit\" >/dev/null 2>&1; then
    # What the sweep ended goes first. A restart that could not load the unit
    # AND ended the process that was serving without one leaves the host with
    # nothing running this unit, and an operator who is not told that reads the
    # refusal as \"nothing happened\". The unit and the domain are not repeated
    # here: the report names both already.
    say 'not_loaded' \"${left:+ended disowned process(es) $left; }${detail:-launchctl bootstrap said nothing and left no job}\"
    exit 0
  fi
  if [ -n \"$still\" ]; then
    say 'restart_failed' \"disowned process survived: $still; unit reloaded in $domain\"
    exit 0
  fi
  say 'restarted' \"$domain\"
  exit 0
else
  # A user unit must outlive the login session that restarted it. A system
  # unit belongs to the machine manager and needs no per-user linger state.
  if [ \"$scope\" = \"user\" ]; then
    /usr/bin/loginctl enable-linger \"$service_user\" >/dev/null 2>&1 \
      || \"$sudo_bin\" -n /usr/bin/loginctl enable-linger \"$service_user\" >/dev/null 2>&1 \
      || true
  fi
  stado_systemctl daemon-reload >/dev/null 2>&1 || true
  detail=$(stado_systemctl restart \"$unit\" 2>&1)
  rc=$?
  if [ \"$rc\" -eq 0 ]; then say 'restarted' \"$systemd_detail\"; else say 'restart_failed' \"$rc $detail\"; fi
fi
";

/// `service show`: what the unit FILE declares — its program and argument
/// vector — and nothing about whether any of it is running.
///
/// The status word is `declares`, not `runs`, and the difference is a
/// multi-day outage. This body reaches no process table and asks launchd
/// nothing; it read `ProgramArguments` out of the plist and then said `runs`,
/// so on 2026-08-30 it reported `com.wisent.always-on.weles` as `runs` while
/// both pids the preceding restart had reported were already gone and the
/// unit's stderr ended in `EADDRINUSE`. A word that means "this file exists
/// and declares this" must not be spelled like a word that means "this is
/// serving". Whether the unit is the process on its own port is
/// [`crate::deploy::service_serving`]'s question.
pub(crate) const SHOW_BODY: &str = "if [ ! -f \"$unit_path\" ]; then
  say 'missing' \"$unit_path\"
  exit 0
fi
if [ \"$os\" = \"Darwin\" ]; then
  args=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments' \"$unit_path\" 2>/dev/null | /usr/bin/sed -n '/^[[:space:]]*[^A-Z}]/{s/^[[:space:]]*//;s/[[:space:]]*$//;p;}' | /usr/bin/tr '\\n' ' ')
  if [ -z \"$args\" ]; then args=$(/usr/libexec/PlistBuddy -c 'Print :Program' \"$unit_path\" 2>/dev/null); fi
else
  args=$(/usr/bin/sed -n 's/^ExecStart=//p' \"$unit_path\" | /usr/bin/tr '\\n' ' ')
fi
# A unit that runs .../services/NAME/current/... names a link, not a version,
# and the link is what every rollback and every competing operator moves. The
# declared path therefore stays identical while the code under it changes,
# which makes 'what does this unit run' unanswerable from the unit alone --
# and answering it by guessing has ended badly enough to be worth one readlink.
program=\"${args%% *}\"
resolved=\"\"
case \"$program\" in
  */current/*)
    link=\"${program%%/current/*}/current\"
    if [ -L \"$link\" ]; then resolved=$(/usr/bin/readlink \"$link\"); fi
    ;;
esac
if [ -n \"$resolved\" ]; then
  say 'declares' \"$args(current -> $resolved)\"
else
  say 'declares' \"$args\"
fi
";

/// End a program that outlived every label it was ever started under.
///
/// Booting out a label is not the same as the program being gone. A unit started
/// once outside its own label -- by a recovery fallback, or by hand -- survives
/// every bootout, keeps the listening socket, and makes each later start die on
/// `address already in use` while the stale instance serves on.
///
/// `stop` has always done this. `restart` did not, which is why a restart after
/// `service update` reported success and left the previous version serving: the
/// relink took effect on no restart at all, and the operator had to know to stop
/// first. Both bodies now splice in this one sweep, so they cannot disagree about
/// what stopping means.
///
/// Sets `left` (what was found) and `still` (what survived a TERM); reporting is
/// the caller's, because stop and restart have different things to say about it.
///
/// Scoped to the unit's whole declared argv, through `stado_unit_pids`. It used
/// to sweep every process whose executable was the unit's program, and on a host
/// where one binary runs the object API, the resolver, the agent and the beacon
/// that is a sweep of the control plane: `stado service restart
/// com.wisent.always-on.stado-object-api --host control-host` on
/// 2026-08-19 TERMed eight processes, among them the host's resolver holding
/// 17600/17601/17612/17621, and reported one unit restarted.
pub(crate) const DISOWNED_SWEEP: &str = "  sweep_argv=$(stado_unit_argv \"$unit_path\")
  left=\"\"
  still=\"\"
  if [ -n \"$sweep_argv\" ]; then
    left=$(stado_unit_pids \"$sweep_argv\")
    if [ -n \"$left\" ]; then
      for pid in $left; do /bin/kill -TERM \"$pid\" >/dev/null 2>&1 || true; done
      /bin/sleep 2
      still=$(stado_unit_pids \"$sweep_argv\")
      # A service that serves each adapter from its own process does not go
      # away on one round of TERM: the process holding the port exits, the
      # siblings holding theirs do not, and launchd is then refused the ports
      # it is being asked to bind. Reporting that as \"survived\" left the unit
      # booted out -- a restart that ends with nothing running. Escalate, and
      # keep 'survived' for a process that refuses SIGKILL.
      if [ -n \"$still\" ]; then
        for pid in $still; do /bin/kill -KILL \"$pid\" >/dev/null 2>&1 || true; done
        /bin/sleep 2
        still=$(stado_unit_pids \"$sweep_argv\")
      fi
    fi
  fi
";
