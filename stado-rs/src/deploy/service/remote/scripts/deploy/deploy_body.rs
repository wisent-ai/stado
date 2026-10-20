//! `service deploy` writes the rendered unit, then bootstraps it; a standby
//! stage writes the same unit and leaves it unloaded. Both renderings travel
//! in the same program and the host picks, so either costs one round trip and
//! never depends on a local guess about the remote OS. The writing is one
//! text shared by both bodies, so a staged unit is byte for byte the unit a
//! deploy or a move would start.

use std::sync::LazyLock;

/// The declared program has to be on the host and executable.
const PROGRAM_PRESENT: &str = "program=@PROGRAM@
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
";

/// The Darwin unit and its log, written owner-only at `$unit_path`. A
/// directory that cannot be made is reported with mkdir's own words.
const DARWIN_WRITE: &str =
    "  detail=$(/bin/mkdir -p \"$HOME/@USER_LAUNCHD_AGENTS@\" \"$HOME/.stado/logs\" 2>&1) || {
    say 'write_failed' \"$detail\"
    exit 0
  }
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
";

/// The systemd unit, written owner-only at `$unit_path`, its directory made
/// first and reported with mkdir's own words when it cannot be.
const LINUX_WRITE: &str = "  detail=$(/bin/mkdir -p \"$HOME/@USER_SYSTEMD_UNITS@\" 2>&1) || {
    say 'write_failed' \"$detail\"
    exit 0
  }
  template=\"$unit_path.template.$$\"
  /bin/cat > \"$template\" <<'@HEREDOC@'
@LINUX_UNIT@
@HEREDOC@
  escaped_home=$(/usr/bin/printf '%s' \"$HOME\" | /usr/bin/sed 's/[\\/&]/\\\\&/g')
  account=$(/usr/bin/id -un)
  /usr/bin/sed -e \"s/__STADO_HOME__/$escaped_home/g\" -e \"s/__STADO_USER__/$account/g\" \"$template\" > \"$unit_path\" || exit 1
  /bin/rm -f \"$template\"
  /bin/chmod u=rw,go= \"$unit_path\" || exit 1
";

const DARWIN_BOOTSTRAP: &str = "  $launch bootout \"$domain/$unit\" >/dev/null 2>&1 || true
  # `retire` leaves the label disabled, and launchd refuses to bootstrap a
  # disabled label with `5: Input/output error` — so a retire followed by a
  # deploy of the same name could never load, leaving a unit down between
  # the two. Enable first; enabling a label
  # that is not disabled is a no-op.
  $launch enable \"$domain/$unit\" >/dev/null 2>&1 || true
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
";

const LINUX_ENABLE: &str = "  # A user unit lives inside the user's systemd instance, and without linger
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
";

/// A standby unit is never written over a running one: a unit the init system
/// runs on the standby host is a second active copy, which a move refuses.
const DARWIN_UNLOADED: &str = "  if $launch print \"$domain/$unit\" >/dev/null 2>&1; then
    say 'loaded' \"$domain/$unit runs on this host; a standby unit is installed and not loaded\"
    exit 0
  fi
";

const LINUX_UNLOADED: &str = "  if stado_systemctl is-active --quiet \"$unit\"; then
    say 'loaded' \"$unit is active on this host; a standby unit is installed and not started\"
    exit 0
  fi
";

/// The init system reads the staged definition, still without starting it.
const LINUX_RELOAD: &str = "  detail=$(stado_systemctl daemon-reload 2>&1) || {
    say 'reload_failed' \"$detail\"
    exit 0
  }
";

/// `service deploy`: write the rendered unit, then bootstrap it.
pub(crate) static DEPLOY_BODY: LazyLock<String> = LazyLock::new(|| {
    unit_directories(format!(
        "{PROGRAM_PRESENT}if [ \"$os\" = \"Darwin\" ]; then\n{DARWIN_WRITE}{DARWIN_BOOTSTRAP}else\n{LINUX_WRITE}{LINUX_ENABLE}fi\n"
    ))
});

/// A standby stage: write the rendered unit where a move starts it and leave
/// it unloaded. It reports `staged` with the unit path, or `loaded` when the
/// unit already runs here, which is refused before anything is written.
pub(crate) static STAGE_BODY: LazyLock<String> = LazyLock::new(|| {
    unit_directories(format!(
        "{PROGRAM_PRESENT}if [ \"$os\" = \"Darwin\" ]; then\n{DARWIN_UNLOADED}{DARWIN_WRITE}  say 'staged' \"$unit_path\"\nelse\n{LINUX_UNLOADED}{LINUX_WRITE}{LINUX_RELOAD}  say 'staged' \"$unit_path\"\nfi\n"
    ))
});

/// The account's unit directories, as `deploy::service` declares them, put
/// into a deploy, stage or ensure body.
pub(super) fn unit_directories(body: String) -> String {
    body.replace(
        "@USER_LAUNCHD_AGENTS@",
        crate::deploy::service::USER_LAUNCHD_AGENTS,
    )
    .replace(
        "@USER_SYSTEMD_UNITS@",
        crate::deploy::service::USER_SYSTEMD_UNITS,
    )
}
