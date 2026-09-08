/// `service converge`: which artefact the live process is executing, as
/// against the one the unit's declaration resolves to today.
///
/// Read-only. Two production incidents are exactly this gap, and neither is
/// visible in any other answer this group gives: Brama's process kept running
/// an artefact tree that `current` no longer pointed at, and the Weles worker
/// kept serving a `dist` that was replaced 26 seconds after it started. In
/// both cases the unit was loaded, the declaration was true, the version on
/// disk was the declared one, and the running code was not it.
///
/// So the host reports facts and the verdict is computed off-host by
/// [`RunningProgram::matches_process`]: the pid, the program the unit
/// declares, what that declaration's `current` link resolves to now, the
/// executable the process table says the pid is running, when the process
/// started, and when each of those two files was last written. A judgement
/// made in shell would be a second opinion about artefact identity.
pub(crate) const PROCESS_BODY: &str = "declared=''
if [ \"$os\" = \"Darwin\" ]; then
  if [ -f \"$unit_path\" ]; then
    declared=$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments:0' \"$unit_path\" 2>/dev/null)
    if [ -z \"$declared\" ]; then
      declared=$(/usr/libexec/PlistBuddy -c 'Print :Program' \"$unit_path\" 2>/dev/null)
    fi
  fi
  stado_launchd_state
  pid=\"$pc_pid\"
else
  if [ -f \"$unit_path\" ]; then
    declared=$(/usr/bin/sed -n 's/^ExecStart=//p' \"$unit_path\" | /usr/bin/head -n 1)
    declared=\"${declared%% *}\"
  fi
  pid=$(stado_systemctl show --property=MainPID --value \"$unit\" 2>/dev/null)
  if [ \"$pid\" = 0 ]; then pid=''; fi
fi
# A unit that runs .../current/... names a link, and the link is what every
# release and every rollback moves. The declaration therefore stays identical
# while the artefact under it changes, so the link has to be resolved here to
# have anything to compare the running process against.
resolved=\"$declared\"
case \"$declared\" in
  */current/*)
    link=\"${declared%%/current/*}/current\"
    leaf=\"${declared#*/current/}\"
    if [ -L \"$link\" ]; then
      dest=$(/usr/bin/readlink \"$link\")
      case \"$dest\" in
        /*) resolved=\"$dest/$leaf\" ;;
        *) resolved=\"${declared%%/current/*}/$dest/$leaf\" ;;
      esac
    fi
    ;;
esac
running=''
started=''
declared_written=''
running_written=''
if [ -n \"$pid\" ]; then
  running=$(/bin/ps -p \"$pid\" -o comm= 2>/dev/null)
  lstart=$(/bin/ps -p \"$pid\" -o lstart= 2>/dev/null)
  if [ \"$os\" = \"Darwin\" ]; then
    started=$(/bin/date -j -f '%a %b %d %T %Y' \"$lstart\" +%s 2>/dev/null)
    if [ -f \"$resolved\" ]; then declared_written=$(/usr/bin/stat -f %m \"$resolved\" 2>/dev/null); fi
    if [ -f \"$running\" ]; then running_written=$(/usr/bin/stat -f %m \"$running\" 2>/dev/null); fi
  else
    started=$(/usr/bin/date -d \"$lstart\" +%s 2>/dev/null)
    if [ -f \"$resolved\" ]; then declared_written=$(/usr/bin/stat -c %Y \"$resolved\" 2>/dev/null); fi
    if [ -f \"$running\" ]; then running_written=$(/usr/bin/stat -c %Y \"$running\" 2>/dev/null); fi
  fi
fi
printf 'STADO_PROCESS\\t%s\\t%s\\t%s\\t%s\\t%s\\t%s\\t%s\\n' \"$pid\" \"$declared\" \"$resolved\" \"$running\" \"$started\" \"$declared_written\" \"$running_written\"
say 'inspected' \"$unit\"
";
