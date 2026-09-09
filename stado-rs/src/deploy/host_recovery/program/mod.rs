//! The fixed remote program's substitutions and the ssh argv that carries it.
//! The program text itself is `template`.

mod template;

use template::REMOTE_SCRIPT_TEMPLATE;

use super::plan::{plan_agents, StableBindPlan};
use super::WC_CANDIDATES;
use crate::deploy::shlex_quote;
use crate::targets::{normalize_hostname, ssh_hostname, ComputeTarget};

/// Python `_identity_values`: normalized names, hostname aliases, and the
/// host part of the SSH destination; empty values dropped, sorted.
pub fn identity_values(target: &ComputeTarget) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    values.push(normalize_hostname(&target.name));
    values.extend(target.hostnames.iter().map(|v| normalize_hostname(v)));
    values.extend(
        target
            .ssh_connections()
            .map(|(_, destination)| ssh_hostname(destination)),
    );
    values.retain(|v| !v.is_empty());
    values.sort();
    values.dedup();
    values
}

/// Python `_remote_script`: the fixed recovery program with this target's
/// identity words spliced in.
///
/// One row per managed unit, and which of the two shell functions the row
/// calls is decided HERE rather than on the host: the plist path alone says
/// whether loading the unit takes root, and a pass that cannot take root has
/// no business running `bootout` against a system daemon on the way to
/// reporting a success it did not have.
pub fn remote_script(target: &ComputeTarget) -> String {
    remote_script_with_stable_binds(target, &[])
}

/// [`remote_script`] with the stable-bind rows this host's `release_control`
/// declares.
///
/// Separate because those rows come from the registry document while every
/// other substitution comes from the target alone: a caller holding only the
/// target still gets a correct pass, and one holding the document also gets
/// the stage that can put a serving port back.
pub fn remote_script_with_stable_binds(
    target: &ComputeTarget,
    stable_binds: &[StableBindPlan],
) -> String {
    let identity_words = identity_values(target)
        .iter()
        .map(|value| shlex_quote(value))
        .collect::<Vec<_>>()
        .join(" ");
    let wc_words = WC_CANDIDATES
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let agent_rows = plan_agents(target)
        .iter()
        .map(|plan| {
            let verb = if plan.privileged {
                "report_system_agent"
            } else {
                "recover_agent"
            };
            format!("{verb} {} \"{}\"", shlex_quote(&plan.label), plan.plist)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let stable_bind_rows = stable_binds
        .iter()
        .map(|plan| {
            format!(
                "recover_stable_bind {} {} {} {} {}",
                shlex_quote(&plan.product),
                shlex_quote(&plan.bind),
                shlex_quote(&plan.plist),
                shlex_quote(&plan.label),
                shlex_quote(
                    &plan
                        .candidate_ports
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    REMOTE_SCRIPT_TEMPLATE
        .replace("@DOMAIN_RESOLVER@", crate::deploy::service::DOMAIN_RESOLVER)
        .replace("@STABLE_BIND_ROWS@", &stable_bind_rows)
        .replace("@IDENTITY_WORDS@", &identity_words)
        .replace("@WC_WORDS@", &wc_words)
        .replace("@AGENT_ROWS@", &agent_rows)
        .replace("/usr/bin/tr '\t\r\n' ' '", r"/usr/bin/tr '\t\r\n' ' '")
}

/// Python `recover_host` ssh argv (note the -o order: BatchMode,
/// ConnectTimeout, StrictHostKeyChecking).
pub fn ssh_argv(ssh_target: &str) -> Vec<String> {
    vec![
        "ssh".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ConnectTimeout=15".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        ssh_target.to_string(),
        "/bin/bash".to_string(),
        "-s".to_string(),
    ]
}
