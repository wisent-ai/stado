/// `service stop`: boot the label out of the domain the resolver chose, then end
/// whatever is still running the unit's argv.
///
/// The second bootout is cleanup and not a second opinion about where the unit
/// lives: the resolution this fix replaces bootstrapped agents into whichever
/// per-login domain answered first, so a host can still carry the job under the
/// spelling the resolver did not choose, and a stop that left that one loaded
/// would be a fence with a writer behind it. Every report and every end-state
/// probe names `$domain`.
pub(crate) const STOP_BODY: &str = "if [ \"$os\" = \"Darwin\" ]; then
  recovery_unit=\"${unit}-recovery\"
  other_domain=\"\"
  case \"$domain\" in
    \"$gui\") other_domain=\"$user_domain\" ;;
    \"$user_domain\") other_domain=\"$gui\" ;;
  esac
  $launch bootout \"$domain/$unit\" >/dev/null 2>&1 || true
  $launch bootout \"$domain/$recovery_unit\" >/dev/null 2>&1 || true
  if [ -n \"$other_domain\" ]; then
    /bin/launchctl bootout \"$other_domain/$unit\" >/dev/null 2>&1 || true
    /bin/launchctl bootout \"$other_domain/$recovery_unit\" >/dev/null 2>&1 || true
  fi
@DISOWNED_SWEEP@
  if [ -n \"$left\" ]; then
    if [ -n \"$still\" ]; then
      say 'stop_failed' \"disowned process still running: $still\"
      exit 0
    fi
    say 'stopped' \"booted out of $domain, and ended disowned process(es): $left\"
    exit 0
  fi
else
  stado_systemctl stop \"$unit\" >/dev/null 2>&1 || true
fi
say 'stopped' \"$unit_path\"
";

/// `service adopt`: a read-only probe. Adoption claims an existing unit, so
/// the host has to agree the unit is there before the registry says Stado
/// owns it — that check is the whole difference between adoption and
/// fiction.
pub(crate) const PROBE_BODY: &str = "file_state='absent'
if [ -f \"$unit_path\" ]; then file_state='present'; fi
unit_state='unloaded'
if [ \"$os\" = \"Darwin\" ]; then
  if /bin/launchctl print \"$domain/$unit\" >/dev/null 2>&1; then unit_state='loaded'; fi
else
  if stado_systemctl cat \"$unit\" >/dev/null 2>&1; then unit_state='loaded'; fi
fi
printf 'STADO_ADOPT\\t%s\\t%s\\n' \"$file_state\" \"$unit_state\"
say 'probed' \"$unit_path\"
";

/// `service retire`: withdraw and stop, while leaving the unit file on disk.
///
/// launchd has two per-login spellings, so both are booted out and disabled.
/// systemd is runtime-masked before it is disabled. The mask is the rolling
/// upgrade fence: a coordinator still running an older Stado may have read the
/// declaration before its withdrawal and try one last `enable --now`; systemd
/// must refuse that stale start without relying on the new shared lease.
pub(crate) const RETIRE_BODY: &str = "if [ \"$os\" = \"Darwin\" ]; then
  recovery_unit=\"${unit}-recovery\"
  /bin/launchctl bootout \"$gui/$unit\" >/dev/null 2>&1 || true
  /bin/launchctl bootout \"$user_domain/$unit\" >/dev/null 2>&1 || true
  /bin/launchctl bootout \"$gui/$recovery_unit\" >/dev/null 2>&1 || true
  /bin/launchctl bootout \"$user_domain/$recovery_unit\" >/dev/null 2>&1 || true
  /bin/launchctl disable \"$gui/$unit\" >/dev/null 2>&1 || true
  /bin/launchctl disable \"$user_domain/$unit\" >/dev/null 2>&1 || true
  /bin/launchctl disable \"$gui/$recovery_unit\" >/dev/null 2>&1 || true
  /bin/launchctl disable \"$user_domain/$recovery_unit\" >/dev/null 2>&1 || true
  say 'retired' \"$unit_path\"
else
  stado_systemctl disable --now \"$unit\" >/dev/null 2>&1 || true
  detail=$(stado_systemctl mask --runtime --now \"$unit\" 2>&1)
  rc=$?
  if [ \"$rc\" -ne 0 ]; then
    say 'retire_failed' \"$rc $detail\"
  elif stado_systemctl is-active --quiet \"$unit\"; then
    say 'retire_failed' \"$unit remained active after its runtime mask\"
  else
    say 'retired' \"$unit_path\"
  fi
fi
";
