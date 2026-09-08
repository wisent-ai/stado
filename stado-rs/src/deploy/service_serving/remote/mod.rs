//! The remote read: the program that answers on the host, the parse of the
//! one line it prints, and the single call an already-resolved host is asked
//! through.

use super::*;
use crate::deploy::{host_channel, service, DeployError, Runner};
use crate::targets::ComputeTarget;

mod script;

#[cfg(test)]
mod ports_travel_base64;

pub use script::remote_serving_script;

/// Parse the script's one line of JSON.
pub fn parse_serving(stdout: &str) -> Result<ServingReport, DeployError> {
    let payload = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{'))
        .ok_or_else(|| DeployError("serving script produced no JSON report".to_string()))?;
    serde_json::from_str(payload).map_err(|error| {
        DeployError(format!(
            "serving script did not return the expected JSON: {error}"
        ))
    })
}

/// Ask one already-resolved host whether this unit is the process on its ports.
pub async fn read_serving(
    target: &ComputeTarget,
    unit: &str,
    unit_path: &str,
    ports: &[u16],
    runner: &Runner,
) -> Result<ServingReport, DeployError> {
    let script = service::serving_script(unit, unit_path, &remote_serving_script(ports))?;
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: {}",
            target.name,
            host_channel::last_error_line(&output, "ssh failed")
        )));
    }
    parse_serving(&output.stdout)
}
