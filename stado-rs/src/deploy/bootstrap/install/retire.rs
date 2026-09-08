//! Stage one, retirement: shut down the queue agents that this bootstrap
//! supersedes, in both the system and every live user systemd domain.

use crate::deploy::CommandSpec;

use super::specs::ssh_argv;

/// End superseded Linux queue agents in the system domain and every live user
/// domain before enabling the canonical system unit. Remote bootstrap may run
/// as root while the old units belong to a login user, so using only the
/// caller's user manager leaves duplicate agents publishing one consumer id.
pub fn retire_superseded_agent_units_spec(ssh_target: &str) -> CommandSpec {
    let script = "set -eu
for unit in wisent-agent.service stado-agent.service; do
  if sudo systemctl is-active --quiet \"$unit\"; then
    sudo systemctl disable --now \"$unit\"
  else
    sudo systemctl disable \"$unit\" >/dev/null 2>&1 || true
  fi
done
for runtime in /run/user/[0-9]*; do
  [ -S \"$runtime/bus\" ] || continue
  uid=${runtime##*/}
  for unit in wisent-agent.service stado-agent.service wisent-compute-agent.service \
    com.wisent.compute.service.stado-agent.service \
    com.wisent.compute.service.stado-agent.service.service \
    com.wisent.compute.service.stado-agent.service.service.service; do
    if sudo -u \"#$uid\" env XDG_RUNTIME_DIR=\"$runtime\" \
      DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" \
      systemctl --user is-active --quiet \"$unit\"; then
      sudo -u \"#$uid\" env XDG_RUNTIME_DIR=\"$runtime\" \
        DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" \
        systemctl --user disable --now \"$unit\"
    else
      sudo -u \"#$uid\" env XDG_RUNTIME_DIR=\"$runtime\" \
        DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" \
        systemctl --user disable \"$unit\" >/dev/null 2>&1 || true
    fi
  done
done";
    CommandSpec::new(ssh_argv(ssh_target, script))
}
