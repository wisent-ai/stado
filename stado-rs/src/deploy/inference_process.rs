//! Guarded inspection and release of unmanaged GPU compute processes.

use serde_json::{json, Value};

use super::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

const LIST_SCRIPT: &str = include_str!("inference_process_list.txt");
const RELEASE_SCRIPT: &str = include_str!("inference_process_release.txt");

fn report(target: &ComputeTarget, output: &super::CommandOutput, ok: &str) -> Value {
    let mut body = host_channel::base_report(target);
    host_channel::finish_report(&mut body, output, ok, "GPU process operation failed");
    body.insert("stdout".to_string(), Value::String(output.stdout.clone()));
    Value::Object(body)
}

fn process_rows(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            let [marker, identity, pid, start_ticks, used_mib, owner, command, cgroup] =
                fields.as_slice()
            else {
                return None;
            };
            if *marker != "PROCESS" {
                return None;
            }
            Some(json!({
                "identity": identity,
                "pid": pid.parse::<u32>().ok()?,
                "start_ticks": start_ticks.parse::<u64>().ok()?,
                "used_mib": used_mib.parse::<u64>().ok()?,
                "owner": owner,
                "command": command,
                "cgroup": cgroup,
            }))
        })
        .collect()
}

fn parse_identity(identity: &str) -> Result<(u32, u64), DeployError> {
    let (pid, start_ticks) = identity.split_once(':').ok_or_else(|| {
        DeployError("invalid process identity; use PID:START_TICKS printed by blockers".to_string())
    })?;
    let pid = pid.parse::<u32>().map_err(|_| {
        DeployError("invalid process identity; PID must be an unsigned integer".to_string())
    })?;
    let start_ticks = start_ticks.parse::<u64>().map_err(|_| {
        DeployError("invalid process identity; START_TICKS must be an unsigned integer".to_string())
    })?;
    if pid == u32::from(false) || start_ticks == u64::from(false) {
        return Err(DeployError(
            "invalid process identity; PID and START_TICKS must be non-zero".to_string(),
        ));
    }
    Ok((pid, start_ticks))
}

pub async fn blockers(target: &ComputeTarget, runner: &Runner) -> Result<Value, DeployError> {
    let output = host_channel::run_script(target, LIST_SCRIPT, runner).await?;
    let processes = process_rows(&output.stdout);
    let mut body = report(target, &output, "inventoried");
    body.as_object_mut()
        .expect("report is an object")
        .insert("processes".to_string(), Value::Array(processes));
    Ok(body)
}

pub async fn release(
    target: &ComputeTarget,
    identity: &str,
    force: bool,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let (pid, start_ticks) = parse_identity(identity)?;
    let script = RELEASE_SCRIPT
        .replace("__PID__", &pid.to_string())
        .replace("__START_TICKS__", &start_ticks.to_string())
        .replace("__FORCE__", if force { "true" } else { "false" });
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(report(target, &output, "released"))
}
