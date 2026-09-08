pub(super) const ENSURE_BODY_TAIL: &str = "    activation_detail=$(stado_systemctl daemon-reload 2>&1)
    activation_rc=$?
    if [ \"$activation_rc\" -ne 0 ]; then
      activation_failure=\"systemctl daemon-reload exited $activation_rc: ${activation_detail:-no detail}\"
      return 1
    fi
    activation_detail=$(stado_systemctl restart \"$unit\" 2>&1)
    activation_rc=$?
    if [ \"$activation_rc\" -ne 0 ]; then
      activation_failure=\"systemctl restart exited $activation_rc: ${activation_detail:-no detail}\"
      return 1
    fi
  fi
  attempts=0
  while [ \"$attempts\" -lt 150 ]; do
    if [ \"$os\" = Darwin ]; then
      stado_launchd_state
      pid=\"$pc_pid\"
    else
      pid=$(stado_systemctl show --property=MainPID --value \"$unit\" 2>/dev/null)
    fi
    if [ -n \"$pid\" ] && [ \"$pid\" != 0 ] && /bin/kill -0 \"$pid\" 2>/dev/null; then
      return 0
    fi
    attempts=$((attempts + 1))
    /bin/sleep 0.1
  done
  activation_failure=\"activation exited 0 but $unit did not acquire a live pid\"
  return 1
}
loaded_drift=no
if [ \"$os\" = Darwin ] && [ \"$had_unit\" = yes ]; then
  if [ -z \"$loaded_program\" ] || [ \"$loaded_arguments_valid\" != yes ]; then
    /bin/rm -f \"$staged\" \"$rendered\"
    say 'loaded_definition_unknown' \"$domain/$unit launchctl readback did not expose a valid Program and complete arguments definition\"
    exit 0
  fi
  if [ \"$loaded_program\" != \"$program\" ] || [ \"$loaded_argv\" != \"$argv\" ]; then
    loaded_drift=yes
  fi
fi
unit_drift=no
if [ -n \"$rendered\" ] && { [ ! -f \"$unit_path\" ] || ! /bin/cmp -s \"$rendered\" \"$unit_path\"; }; then
  unit_drift=yes
fi
reload_needed=no
reload_action=converged
if [ \"$os\" = Darwin ] && [ \"$had_unit\" = yes ] \
  && { [ \"$loaded_drift\" = yes ] || [ \"$unit_drift\" = yes ]; }; then
  reload_needed=yes
  if [ \"$loaded_drift\" = yes ]; then reload_action=reloaded; fi
elif [ \"$declared_argv\" = \"$argv\" ] && [ \"$unit_drift\" = yes ]; then
  reload_needed=yes
fi
if [ \"$reload_needed\" = yes ]; then
  [ -n \"$rendered\" ] || bail 'cannot reload a definition without rendered configuration'
  previous=''
  rollback_unavailable='no prior unit file existed'
  if [ -f \"$unit_path\" ]; then
    if [ \"$unit_drift\" = no ]; then
      rollback_unavailable='existing unit already matched the desired definition; no distinct prior definition exists'
    else
      previous=\"$staged.previous\"
      /bin/cp \"$unit_path\" \"$previous\" || bail 'cannot preserve the prior unit'
      /bin/chmod u=rw,go= \"$previous\" || bail 'cannot protect the prior unit'
      rollback_unavailable=''
    fi
  fi
  if [ \"$unit_drift\" = yes ] && ! stado_install_unit \"$rendered\"; then
    if [ -n \"$previous\" ]; then
      stado_install_unit \"$previous\" || bail \"unit write failed; rollback failed; prior unit is $previous\"
      /bin/rm -f \"$previous\"
      bail 'unit write failed; prior unit restored'
    fi
    bail \"unit write failed; rollback not attempted: $rollback_unavailable\"
  fi
  if ! stado_activate_definition; then
    replacement_failure=\"$activation_failure\"
    if [ -n \"$previous\" ]; then
      if stado_install_unit \"$previous\" && stado_activate_definition; then
        /bin/rm -f \"$previous\"
        bail \"replacement activation failed ($replacement_failure); prior unit restored and running\"
      fi
      bail \"replacement activation failed ($replacement_failure); rollback failed; prior unit is $previous\"
    fi
    bail \"replacement activation failed ($replacement_failure); rollback not attempted: $rollback_unavailable\"
  fi
  verification_failure=''
  if [ \"$os\" = Darwin ]; then
    stado_loaded_identity
    if [ -z \"$loaded_program\" ] || [ \"$loaded_arguments_valid\" != yes ]; then
      verification_failure='launchctl readback after reload did not expose a valid Program and complete arguments definition'
    elif [ \"$loaded_program\" != \"$program\" ] || [ \"$loaded_argv\" != \"$argv\" ]; then
      verification_failure=\"launchctl retained program [$loaded_program] argv [$loaded_argv]; expected program [$program] argv [$argv]\"
    else
      stado_process_serves \"$pid\"
      if [ \"$serves\" != yes ]; then
        verification_failure=\"$domain/$unit pid $pid executes [$running]; expected [$program]\"
      fi
    fi
  fi
  if [ -n \"$verification_failure\" ]; then
    if [ -n \"$previous\" ]; then
      if stado_install_unit \"$previous\" && stado_activate_definition; then
        /bin/rm -f \"$previous\"
        bail \"replacement verification failed ($verification_failure); prior unit restored and running\"
      fi
      bail \"replacement verification failed ($verification_failure); rollback failed; prior unit is $previous\"
    fi
    bail \"replacement verification failed ($verification_failure); rollback not attempted: $rollback_unavailable\"
  fi
  /bin/rm -f \"$previous\" \"$staged\" \"$rendered\"
  printf 'STADO_ENSURE\\t%s\\t%s\\t%s\\n' \"$domain\" \"$pid\" \"$unit_path\"
  say \"$reload_action\" \"$unit_path reloaded and verified\"
  exit 0
fi
if [ \"$declared_argv\" = \"$argv\" ] && [ \"$serves\" = yes ]; then
  /bin/rm -f \"$staged\" \"$rendered\"
  printf 'STADO_ENSURE\\t%s\\t%s\\t%s\\n' \"$domain\" \"$pid\" \"$unit_path\"
  say 'already_correct' \"$domain/$unit pid $pid\"
  exit 0
fi
if [ \"$declared_argv\" = \"$argv\" ]; then
  /bin/rm -f \"$staged\" \"$rendered\"
  rendered=''
fi
if [ \"$declared_argv\" != \"$argv\" ]; then

  if [ \"$os\" = \"Darwin\" ]; then
    /bin/rm -f \"$staged\" \"$rendered\"
    /bin/mkdir -p \"$HOME/.stado/logs\" >/dev/null 2>&1 || bail 'cannot create the log directory'
    /bin/chmod u=rwx,go= \"$HOME/.stado/logs\" || bail 'cannot protect the log directory'
    log=\"$HOME/.stado/logs/$unit.log\"
    : >> \"$log\" || bail \"cannot create $log\"
    /bin/chmod u=rw,go= \"$log\" || bail \"cannot protect $log\"
    staged=\"$HOME/.stado/$unit.plist.$$\"
    if [ \"$domain\" = system ]; then
      /bin/cat > \"$staged\" <<'@HEREDOC@'
@DARWIN_DAEMON_UNIT@
@HEREDOC@
    else
      /bin/mkdir -p \"$HOME/Library/LaunchAgents\" >/dev/null 2>&1 || bail 'cannot create LaunchAgents'
      /bin/cat > \"$staged\" <<'@HEREDOC@'
@DARWIN_UNIT@
@HEREDOC@
    fi
    escaped_home=$(/usr/bin/printf '%s' \"$HOME\" | /usr/bin/sed 's/[\\/&]/\\\\&/g')
    account=$(/usr/bin/id -un)
    /usr/bin/sed -e \"s/__STADO_HOME__/$escaped_home/g\" -e \"s/__STADO_USER__/$account/g\" \"$staged\" > \"$staged.rendered\" || bail 'cannot render the unit'
    if [ \"$domain\" = system ]; then
      /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel \"$staged.rendered\" \"$unit_path\" || bail \"sudo -n install $unit_path was refused\"
    else
      /bin/cp \"$staged.rendered\" \"$unit_path\" || bail \"cannot write $unit_path\"
      /bin/chmod u=rw,go= \"$unit_path\" || bail \"cannot protect $unit_path\"
    fi
    /bin/rm -f \"$staged\" \"$staged.rendered\"
  else
    /bin/rm -f \"$staged\" \"$rendered\"
    /bin/mkdir -p \"$HOME/.config/systemd/user\" >/dev/null 2>&1 || bail 'cannot create the systemd user directory'
    staged=\"$HOME/.stado/$unit.ensure.$$\"
    /bin/cat > \"$staged\" <<'@HEREDOC@'
@LINUX_UNIT@
@HEREDOC@
    escaped_home=$(/usr/bin/printf '%s' \"$HOME\" | /usr/bin/sed 's/[\\/&]/\\\\&/g')
    account=$(/usr/bin/id -un)
    /usr/bin/sed -e \"s/__STADO_HOME__/$escaped_home/g\" -e \"s/__STADO_USER__/$account/g\" \"$staged\" > \"$staged.rendered\" || bail 'cannot render the unit'
    stado_install_unit \"$staged.rendered\" || bail \"cannot write $unit_path\"
    /bin/rm -f \"$staged\" \"$staged.rendered\"
  fi
fi
if [ \"$os\" = \"Darwin\" ]; then
  if [ \"$had_unit\" = yes ]; then
    action=restarted
    detail=$($launch kickstart -k \"$domain/$unit\" 2>&1)
    rc=$?
  else
    action=created
    $launch enable \"$domain/$unit\" >/dev/null 2>&1 || true
    detail=$($launch bootstrap \"$domain\" \"$unit_path\" 2>&1)
    rc=$?
  fi
else
  # Same linger guarantee as DEPLOY_BODY, only for per-user units.
  if [ \"$scope\" = \"user\" ]; then
    /usr/bin/loginctl enable-linger \"$service_user\" >/dev/null 2>&1 \
      || \"$sudo_bin\" -n /usr/bin/loginctl enable-linger \"$service_user\" >/dev/null 2>&1 \
      || true
  fi
  stado_systemctl daemon-reload >/dev/null 2>&1 || true
  stado_systemctl unmask \"$unit\" >/dev/null 2>&1 || true
  if [ \"$had_unit\" = yes ]; then
    action=restarted
    detail=$(stado_systemctl restart \"$unit\" 2>&1)
    rc=$?
  else
    action=created
    detail=$(stado_systemctl enable --now \"$unit\" 2>&1)
    rc=$?
  fi
fi
if [ \"$rc\" -ne 0 ]; then
  say \"${action}_failed\" \"$rc $detail\"
  exit 0
fi
/bin/sleep 1
pid=''
if [ \"$os\" = \"Darwin\" ]; then
  stado_launchd_state
  if [ \"$pc_loaded\" = no ]; then
    # The verb reported success and launchd has no job under the label: the
    # same shape a `restarted: direct process <pid>` used to hide.
    say 'not_loaded' \"${detail:-launchctl reported success and left no job}\"
    exit 0
  fi
  pid=\"$pc_pid\"
else
  pid=$(stado_systemctl show --property=MainPID --value \"$unit\" 2>/dev/null)
  if [ \"$pid\" = 0 ]; then pid=''; fi
fi
printf 'STADO_ENSURE\t%s\t%s\t%s\n' \"$domain\" \"$pid\" \"$unit_path\"
say \"$action\" \"$unit_path\"
";
