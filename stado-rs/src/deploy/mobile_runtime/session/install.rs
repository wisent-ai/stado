//! The repair half: install exactly what the host declared, and report what
//! the installer said about each step.

use super::super::paths::with_candidates;
use super::install_script::REMOTE_REPAIR_BODY;
use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::{ComputeTarget, MobileRuntime};

/// Install the declared runtime on the host, and report every step.
pub async fn repair(
    target: &ComputeTarget,
    declared: &MobileRuntime,
    runner: &Runner,
) -> Result<Vec<String>, DeployError> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    // The declaration reaches a shell, so it is checked before it does. Not
    // quoting-as-defence: a version or driver name is a coordinate, and one
    // carrying a shell character is a malformed declaration to refuse, for
    // the reason `host_exec` refuses an argument a shell would interpret.
    if declared.appium.trim().is_empty()
        || !declared
            .appium
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-+".contains(character))
    {
        return Err(DeployError(format!(
            "{:?} is not an Appium version coordinate",
            declared.appium
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    for driver in &declared.drivers {
        if driver.trim().is_empty()
            || !driver
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err(
                DeployError(format!("{driver:?} is not an Appium driver name"))
                    .stating(crate::primitives::failure::FailureCode::Config),
            );
        }
    }
    let script = with_candidates(REMOTE_REPAIR_BODY)
        .replace("@APPIUM_B64@", &STANDARD.encode(declared.appium.as_bytes()))
        .replace(
            "@DRIVERS_B64@",
            &STANDARD.encode(declared.drivers.join(" ").as_bytes()),
        )
        .replace(
            "@PLATFORM_TOOLS_B64@",
            &STANDARD.encode(if declared.platform_tools { "yes" } else { "no" }),
        );
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
        return Err(DeployError::unreachable(format!(
            "{}: the installer reported nothing: {}",
            target.name,
            host_channel::last_error_line(&output, "no output")
        )));
    }
    Ok(lines)
}
