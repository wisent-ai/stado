//! The three passes over a host: read the release's requirement, verify the
//! cache against it, and install what is missing.

use serde_json::Value;

use super::remote::{REMOTE_REPAIR_BODY, REMOTE_VERIFY_BODY};
use super::{
    parse_requirements, ComponentState, Requirement, RuntimeReport, BROWSERS_JSON,
    COMPONENT_UNKNOWN,
};
use crate::deploy::{host_channel, service_file_fetch, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Read the release's requirement, byte-exact.
pub async fn requirements(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Vec<Requirement>, DeployError> {
    let fetched = service_file_fetch::fetch_file(target, BROWSERS_JSON, runner).await?;
    if !fetched.ok() {
        return Err(DeployError(format!(
            "{}: could not read {BROWSERS_JSON}, so what browser runtime this release needs is \
             unknown: {} ({})",
            target.name, fetched.report.file_state, fetched.integrity
        )));
    }
    parse_requirements(&String::from_utf8_lossy(&fetched.content))
}

/// Verify every declared component against the host's cache.
pub async fn verify(
    target: &ComputeTarget,
    declared: &[Requirement],
    required: &[String],
    runner: &Runner,
) -> Result<RuntimeReport, DeployError> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    let payload = declared
        .iter()
        .map(|component| format!("{}|{}", component.name, component.marker()))
        .collect::<Vec<String>>()
        .join("\n");
    let script = REMOTE_VERIFY_BODY.replace("@MARKERS_B64@", &STANDARD.encode(payload.as_bytes()));
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: {}",
            target.name,
            host_channel::last_error_line(&output, "ssh failed")
        )));
    }
    let line = output
        .stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{'))
        .ok_or_else(|| DeployError("runtime script produced no JSON report".to_string()))?;
    let parsed: Value = serde_json::from_str(line)
        .map_err(|error| DeployError(format!("runtime script returned bad JSON: {error}")))?;
    let rows = parsed
        .get("components")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let components = declared
        .iter()
        .map(|component| {
            let row = rows.iter().find(|row| {
                row.get("name").and_then(Value::as_str) == Some(component.name.as_str())
            });
            ComponentState {
                name: component.name.clone(),
                revision: component.revision.clone(),
                install_by_default: component.install_by_default,
                expected_path: row
                    .and_then(|row| row.get("path"))
                    .and_then(Value::as_str)
                    .unwrap_or(&component.marker())
                    .to_string(),
                state: row
                    .and_then(|row| row.get("state"))
                    .and_then(Value::as_str)
                    .unwrap_or(COMPONENT_UNKNOWN)
                    .to_string(),
            }
        })
        .collect();
    Ok(RuntimeReport {
        components,
        required: required.to_vec(),
    })
}

/// Install the named components on the host.
pub async fn repair(
    target: &ComputeTarget,
    components: &[String],
    runner: &Runner,
) -> Result<Vec<String>, DeployError> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    for component in components {
        if !component
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err(DeployError(format!(
                "{component:?} is not a Playwright component name"
            )));
        }
    }
    let script =
        REMOTE_REPAIR_BODY.replace("@COMPONENTS_B64@", &STANDARD.encode(components.join(" ")));
    let output = host_channel::run_script(target, &script, runner).await?;
    let lines: Vec<String> = output
        .stdout
        .lines()
        .filter(|line| line.starts_with("STADO_RUNTIME\t"))
        .map(|line| {
            line.trim_start_matches("STADO_RUNTIME\t")
                .replace('\t', ": ")
        })
        .collect();
    if lines.is_empty() {
        return Err(DeployError(format!(
            "{}: the installer reported nothing: {}",
            target.name,
            host_channel::last_error_line(&output, "no output")
        )));
    }
    Ok(lines)
}
