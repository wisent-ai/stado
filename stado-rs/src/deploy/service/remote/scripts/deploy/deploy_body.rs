/// `service deploy`: write the rendered unit, then bootstrap it. Both
/// renderings travel in the same program and the host picks, so a deploy
/// costs one round trip and never depends on a local guess about the
/// remote OS.
pub(crate) const DEPLOY_BODY: &str = "program=@PROGRAM@
if [ ! -f \"$program\" ]; then
  say 'program_missing' \"$program\"
  exit 0
fi
if [ ! -x \"$program\" ]; then
  /bin/chmod u+x \"$program\" || {
    say 'program_not_executable' \"$program\"
    exit 0
  }
fi
if [ \"$os\" = \"Darwin\" ]; then
  /bin/mkdir -p \"$HOME/Library/LaunchAgents\" \"$HOME/.stado/logs\" >/dev/null 2>&1 || exit 1
  /bin/chmod u=rwx,go= \"$HOME/.stado/logs\" || exit 1
  log=\"$HOME/.stado/logs/$unit.log\"
  : >> \"$log\" || exit 1
  /bin/chmod u=rw,go= \"$log\" || exit 1
  template=\"$unit_path.template.$$\"
  /bin/cat > \"$template\" <<'@HEREDOC@'
@DARWIN_UNIT@
@HEREDOC@
  escaped_home=$(/usr/bin/printf '%s' \"$HOME\" | /usr/bin/sed 's/[\\/&]/\\\\&/g')
  /usr/bin/sed \"s/__STADO_HOME__/$escaped_home/g\" \"$template\" > \"$unit_path\" || exit 1
  /bin/rm -f \"$template\"
  /bin/chmod u=rw,go= \"$unit_path\" || exit 1
  $launch bootout \"$domain/$unit\" >/dev/null 2>&1 || true
  detail=$($launch bootstrap \"$domain\" \"$unit_path\" 2>&1)
  rc=$?
  # No `asuser` retry into a domain this login cannot join, no `launchctl
  # submit` of a second label, and no `nohup` of the program: a deploy is
  # recorded in the canonical registry by its caller, and a record naming a
  # unit launchd never accepted is a declaration no later command can act on.
  if ! $launch print \"$domain/$unit\" >/dev/null 2>&1; then
    say 'not_loaded' \"${detail:-launchctl bootstrap said nothing and left no job}\"
    exit 0
  fi
  $launch enable \"$domain/$unit\" >/dev/null 2>&1 || true
  $launch kickstart -k \"$domain/$unit\" >/dev/null 2>&1 || true
  say 'deployed' \"$unit_path\"
else
  /bin/mkdir -p \"$HOME/.config/systemd/user\" >/dev/null 2>&1 || true
  template=\"$unit_path.template.$$\"
  /bin/cat > \"$template\" <<'@HEREDOC@'
@LINUX_UNIT@
@HEREDOC@
  escaped_home=$(/usr/bin/printf '%s' \"$HOME\" | /usr/bin/sed 's/[\\/&]/\\\\&/g')
  account=$(/usr/bin/id -un)
  /usr/bin/sed -e \"s/__STADO_HOME__/$escaped_home/g\" -e \"s/__STADO_USER__/$account/g\" \"$template\" > \"$unit_path\" || exit 1
  /bin/rm -f \"$template\"
  /bin/chmod u=rw,go= \"$unit_path\" || exit 1
  # A user unit lives inside the user's systemd instance, and without linger
  # that instance ends with the login session that created it — on rtx every
  # user-scoped service (beacon, agent, router) died seconds after the deploy
  # channel closed, and the host read as down. A system unit belongs to the
  # machine manager and needs no per-user linger state.
  if [ \"$scope\" = \"user\" ]; then
    /usr/bin/loginctl enable-linger \"$service_user\" >/dev/null 2>&1 \
      || \"$sudo_bin\" -n /usr/bin/loginctl enable-linger \"$service_user\" >/dev/null 2>&1 \
      || true
  fi
  stado_systemctl daemon-reload >/dev/null 2>&1 || true
  stado_systemctl unmask \"$unit\" >/dev/null 2>&1 || true
  detail=$(stado_systemctl enable --now \"$unit\" 2>&1)
  rc=$?
  if [ \"$rc\" -eq 0 ]; then say 'deployed' \"$unit_path\"; else say 'enable_failed' \"$rc $detail\"; fi
fi
";
