//! Stage one, retirement: shut down the queue agents an earlier bootstrap
//! installed, in both the system and every live user systemd domain.
//!
//! A superseded agent is found from what its unit runs, never from its name:
//! any service unit whose `ExecStart` runs a program of the same name as the
//! Stado this bootstrap installs, with `agent` as its command, runs the
//! removed standalone queue agent. Every such unit is disabled and stopped,
//! so a unit an older release named differently needs no code change to be
//! retired; the host's worker is the `--worker` role of its one serve unit.

use crate::deploy::{shlex_quote, CommandSpec};

use super::specs::ssh_argv;

const RETIRE_SCRIPT: &str = "set -eu
program_name=$(basename @PROGRAM@)
queue_agent() {
  printf '%s\\n' \"$1\" | awk -v name=\"$program_name\" '
    { count = split($0, words, \" \") }
    { for (i = 1; i <= count; i++) if (words[i] ~ /^argv\\[\\]=/) {
        program = substr(words[i], length(\"argv[]=\") + 1)
        sub(\".*/\", \"\", program)
        if (program == name && words[i + 1] == \"agent\") found = 1
    } }
    END { exit found ? 0 : 1 }'
}
retire_in() {
  \"$@\" list-unit-files --type=service --no-legend --plain | awk '{ print $1 }' |
  while read -r unit; do
    exec_start=$(\"$@\" show -p ExecStart --value \"$unit\")
    if queue_agent \"$exec_start\"; then
      printf 'retiring %s: it runs the %s queue agent\\n' \"$unit\" \"$program_name\"
      \"$@\" disable --now \"$unit\"
    fi
  done
}
retire_in sudo systemctl
for runtime in /run/user/[0-9]*; do
  [ -S \"$runtime/bus\" ] || continue
  uid=${runtime##*/}
  retire_in sudo -u \"#$uid\" env XDG_RUNTIME_DIR=\"$runtime\" \\
    DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" systemctl --user
done";

/// End the removed standalone queue agents in the system domain and every
/// live user domain. Remote bootstrap may run as root while the old units
/// belong to a login user, so using only the caller's user manager leaves
/// them running. `stado_bin` is the program the host unit runs; every unit
/// running a program of that name as a queue agent is retired and named in
/// the output.
pub fn retire_superseded_agent_units_spec(ssh_target: &str, stado_bin: &str) -> CommandSpec {
    let script = RETIRE_SCRIPT.replace("@PROGRAM@", &shlex_quote(stado_bin));
    CommandSpec::new(ssh_argv(ssh_target, &script))
}
