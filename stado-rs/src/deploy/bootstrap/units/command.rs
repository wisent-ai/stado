//! Stage two, unit installation: wrap a rendered unit in its remote
//! write-unmask-enable command, and pair the agent unit with the watchdog
//! unit into the one install list a provision walks.

use crate::deploy::{shlex_quote, CommandSpec};
use crate::targets::ComputeTarget;

use super::super::install::ssh_argv;
use super::text::{agent_unit_text, watchdog_unit_text};

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

/// `.../stado` → `.../{name}`, otherwise bare name.
pub fn sibling_bin(stado_bin: &str, name: &str) -> String {
    if let Some(prefix) = stado_bin.strip_suffix("/stado") {
        return format!("{prefix}/{name}");
    }
    name.to_string()
}

/// The two (unit name, unit text, command) installs for one target, given
/// the resolved remote stado path and WC_PYTHON.
pub fn unit_installs(
    target: &ComputeTarget,
    ssh_target: &str,
    stado_bin: &str,
    wc_python: &str,
) -> Vec<(String, String, CommandSpec)> {
    let user = remote_user(ssh_target);
    let agent_text = agent_unit_text(&target.name, stado_bin, wc_python, &user);
    let watchdog_text = watchdog_unit_text(
        &target.name,
        &sibling_bin(stado_bin, "stado-watchdog"),
        &user,
    );
    [
        ("wisent-compute-agent.service", agent_text),
        ("wisent-compute-watchdog.service", watchdog_text),
    ]
    .into_iter()
    .map(|(unit_name, unit_text)| {
        let command = write_unit_command(unit_name, &unit_text);
        (
            unit_name.to_string(),
            unit_text,
            CommandSpec::new(ssh_argv(ssh_target, &command)),
        )
    })
    .collect()
}

/// Python: `ssh_target.split("@", 1)[0] if "@" in ssh_target else "root"`.
fn remote_user(ssh_target: &str) -> String {
    match ssh_target.split_once('@') {
        Some((user, _)) => user.to_string(),
        None => "root".to_string(),
    }
}
