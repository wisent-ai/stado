//! Stage two, unit installation: wrap the rendered agent unit in its remote
//! write-unmask-enable command.

use crate::deploy::{shlex_quote, CommandSpec};
use crate::targets::ComputeTarget;

use super::super::install::ssh_argv;
use super::text::agent_unit_text;

/// Python `_write_unit` remote command: unmask before writing, then
/// payload-escaped `echo ... | sudo tee`, daemon-reload, enable and restart.
/// Writing before `unmask` follows a `/dev/null` mask and loses the unit.
pub fn write_unit_command(unit_name: &str, unit_text: &str) -> String {
    let payload = unit_text.replace('\\', "\\\\").replace('\'', "'\\''");
    let unit_path = shlex_quote(&format!("/etc/systemd/system/{unit_name}"));
    let unit_arg = shlex_quote(unit_name);
    format!(
        "sudo systemctl unmask {unit_arg} && echo '{payload}' | sudo tee {unit_path} >/dev/null && sudo systemctl daemon-reload && sudo systemctl enable {unit_arg} && sudo systemctl restart {unit_arg}"
    )
}

/// The one unit Linux bootstrap writes for the queue agent. Retirement keeps
/// it and retires every other unit that runs the same program's queue agent.
pub const AGENT_UNIT: &str = "wisent-compute-agent.service";

/// The agent unit [`AGENT_UNIT`] for one target: its text and
/// the command that installs it, given the resolved remote stado path and the
/// environment the agent runs with (its dedicated Skarbiec grant, at
/// bootstrap). Diagnostics run as the `--watchdog` role of a host's one
/// `stado serve` process where a host declares them.
pub fn agent_install(
    target: &ComputeTarget,
    ssh_target: &str,
    stado_bin: &str,
    environment: &[(&'static str, String)],
) -> (String, CommandSpec) {
    let user = remote_user(ssh_target);
    let unit_text = agent_unit_text(&target.name, stado_bin, &user, environment);
    let command = write_unit_command(AGENT_UNIT, &unit_text);
    (unit_text, CommandSpec::new(ssh_argv(ssh_target, &command)))
}

/// Python: `ssh_target.split("@", 1)[0] if "@" in ssh_target else "root"`.
fn remote_user(ssh_target: &str) -> String {
    match ssh_target.split_once('@') {
        Some((user, _)) => user.to_string(),
        None => "root".to_string(),
    }
}

/// The remote account's home, for a path a systemd unit must carry
/// absolute: `/root` for root, `/home/<user>` otherwise.
pub fn remote_home(ssh_target: &str) -> String {
    match remote_user(ssh_target).as_str() {
        "root" => "/root".to_string(),
        user => format!("/home/{user}"),
    }
}
