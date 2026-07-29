//! Registry-authorized reboot of a managed macOS host.
//!
//! Sibling of `host_recovery`: the registry selects only the host, the
//! remote program is fixed (a graceful shutdown), BatchMode ssh is the only
//! transport, and no shell fragments come from registry data.

use serde_json::{json, Map, Value};

use super::{py_str_repr, CommandSpec, DeployError, Runner};
use crate::targets::{ComputeTarget, Registry};

fn resolve_target<'a>(
    registry: &'a Registry,
    target_name: &str,
) -> Result<&'a ComputeTarget, DeployError> {
    let Some(target) = registry.lookup(target_name) else {
        return Err(DeployError(format!(
            "target {} is not in the canonical registry",
            py_str_repr(target_name)
        )));
    };
    if !target.is_provider(crate::capabilities::ProviderId::Local) {
        return Err(DeployError(format!(
            "target {} is not a local host",
            py_str_repr(target_name)
        )));
    }
    if target.ssh.as_deref().unwrap_or("").is_empty() {
        return Err(DeployError(format!(
            "target {} has no registry-managed ssh destination",
            py_str_repr(target_name)
        )));
    }
    Ok(target)
}

/// ssh argv for a graceful reboot; option set matches
/// `host_recovery::ssh_argv` so both commands ride the identical channel.
pub fn ssh_reboot_argv(ssh_target: &str) -> Vec<String> {
    vec![
        "ssh".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ConnectTimeout=15".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        ssh_target.to_string(),
        "sudo -n /sbin/shutdown -r now".to_string(),
    ]
}

/// `stado host reboot TARGET` — request a graceful reboot on one canonical
/// registry host (the canonical remote registry, the fleet-survival
/// authority; an unreachable store is an error, never an empty registry).
pub async fn reboot_host(target_name: &str, runner: &Runner) -> Result<Value, DeployError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    let target = resolve_target(&registry, target_name)?;
    let output = runner(CommandSpec {
        argv: ssh_reboot_argv(target.ssh.as_deref().unwrap_or("")),
        stdin: None,
        timeout: None,
    })
    .await
    .map_err(DeployError)?;

    let mut report = Map::new();
    report.insert("target".to_string(), json!(target.name));
    report.insert(
        "ssh".to_string(),
        target.ssh.as_ref().map_or(Value::Null, |ssh| json!(ssh)),
    );
    report.insert("exit_code".to_string(), json!(output.code));
    report.insert(
        "status".to_string(),
        json!(if output.ok() {
            "reboot_requested"
        } else {
            "failed"
        }),
    );
    if !output.ok() {
        // The common failure is sudo requiring a password; surface the last
        // stderr line verbatim so the operator knows whether to grant
        // passwordless shutdown or reboot the box physically.
        let detail = output.detail().trim();
        let last = match detail.lines().next_back() {
            Some(line) => line.to_string(),
            None => "ssh failed".to_string(),
        };
        report.insert("error".to_string(), json!(last));
    }
    Ok(Value::Object(report))
}
