/// `service logs`: tail the unit's own logs. On launchd the log paths come
/// from the unit file itself, so an adopted unit keeps its chosen
/// destinations. A unit without a StandardOutPath falls back to the
/// account's owner-only Stado log path.
///
/// stderr is a second file under launchd, not a stream the stdout tail
/// already carries, so it gets its own delimited section: `STADO_ERR` names
/// the path and streams its tail, or carries the reason there is nothing to
/// show. It is never silently omitted — a unit that died answering nothing
/// on stdout kept its reason in stderr, and "no section" used to be
/// indistinguishable from "nothing written". The two tails share the
/// --lines budget, half each. On Linux the journal already merges the
/// streams, so that branch stays one section.
pub(crate) const LOGS_BODY: &str = "if [ \"$os\" = \"Darwin\" ]; then
  log=''
  err_log=''
  if [ -f \"$unit_path\" ]; then
    log=$(/usr/bin/plutil -extract StandardOutPath raw -o - \"$unit_path\" 2>/dev/null)
    err_log=$(/usr/bin/plutil -extract StandardErrorPath raw -o - \"$unit_path\" 2>/dev/null)
  fi
  if [ -z \"$log\" ]; then log=\"$HOME/.stado/logs/$unit.log\"; fi
  if [ -f \"$log\" ]; then
    printf 'STADO_LOG\\t%s\\n' \"$log\"
    /usr/bin/tail -c @MAX_BYTES@ \"$log\" | /usr/bin/tail -n @OUT_LINES@
  else
    say 'missing_log' \"$log\"
  fi
  if [ -z \"$err_log\" ]; then
    printf 'STADO_ERR\\t%s\\n' 'absent in plist'
  elif [ -s \"$err_log\" ]; then
    printf 'STADO_ERR\\t%s\\n' \"$err_log\"
    /usr/bin/tail -c @MAX_BYTES@ \"$err_log\" | /usr/bin/tail -n @ERR_LINES@
  else
    printf 'STADO_ERR\\t%s\\n' \"$err_log (empty)\"
  fi
else
  if [ \"$scope\" = \"system\" ]; then
    printf 'STADO_LOG\\tjournalctl -u %s\\n' \"$unit\"
    stado_root /usr/bin/journalctl -u \"$unit\" -n @LINES@ --no-pager 2>&1
  else
    printf 'STADO_LOG\\tjournalctl --user -u %s\\n' \"$unit\"
    runtime=\"/run/user/$service_uid\"
    if [ \"$service_uid\" = \"$uid\" ]; then
      /usr/bin/env \
        XDG_RUNTIME_DIR=\"$runtime\" \
        DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" \
        /usr/bin/journalctl --user -u \"$unit\" -n @LINES@ --no-pager 2>&1
    else
      \"$sudo_bin\" -n -u \"$service_user\" /usr/bin/env \
        XDG_RUNTIME_DIR=\"$runtime\" \
        DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" \
        /usr/bin/journalctl --user -u \"$unit\" -n @LINES@ --no-pager 2>&1
    fi
  fi
fi
";

/// `service env`: fetch the complete unit definition, including systemd drop-ins.
/// Parsing on this side keeps the remote program fixed and narrow, and
/// keeps redaction in one place instead of trusting a shell pipeline to
/// have caught every credential-shaped key.
pub(crate) const UNIT_FILE_BODY: &str = "if [ \"$os\" = Linux ]; then
  if ! content=$(stado_systemctl cat --no-pager \"$unit\" 2>&1); then
    say 'unit_definition_unavailable' \"$content\"
    exit 1
  fi
  printf 'STADO_UNITFILE\\t%s\\n%s\\n' \"$unit_path\" \"$content\"
elif [ -f \"$unit_path\" ]; then
  printf 'STADO_UNITFILE\\t%s\\n' \"$unit_path\"
  /bin/cat \"$unit_path\"
else
  say 'missing_unit_file' \"$unit_path\"
fi
";
