//! The repair half: install exactly what the host declared, and report what
//! the installer said about each step.

use super::super::paths::with_candidates;
use super::install_script::REMOTE_REPAIR_BODY;
use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::{ComputeTarget, MobileRuntime};

/// Wall clock a repair is allowed, and why it is not the shared default.
///
/// [`host_channel::run_script`]'s bound is 120 seconds, which is right for
/// the reads every other host command makes and wrong for this one: a single
/// `appium driver install uiautomator2` fetches the driver, its dependency
/// tree and its bundled server APKs, and the first attempt at this repair
/// died at exactly that bound with the driver half-installed. A timeout
/// shorter than the operation does not protect anything — it converts a slow
/// success into an indeterminate state — so the bound is sized to the work
/// and stays a bound, because an install that has not finished in a quarter
/// of an hour is a fault to report and not a download to keep waiting on.
const REPAIR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(900);

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
        )));
    }
    for driver in &declared.drivers {
        if driver.trim().is_empty()
            || !driver
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err(DeployError(format!(
                "{driver:?} is not an Appium driver name"
            )));
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
    let output =
        host_channel::run_script_with_timeout(target, &script, REPAIR_TIMEOUT, runner).await?;
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

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    fn declared() -> MobileRuntime {
        MobileRuntime {
            appium: "3.2.1".to_string(),
            drivers: vec!["xcuitest".to_string(), "uiautomator2".to_string()],
            platform_tools: true,
            extra: Map::new(),
        }
    }

    #[test]
    fn a_version_that_is_not_a_coordinate_is_refused_before_it_reaches_a_shell() {
        let runner = crate::deploy::production_runner();
        let mut requirement = declared();
        requirement.appium = "3.2.1; rm -rf /".to_string();
        let target = ComputeTarget {
            name: "h".to_string(),
            kind: "local".to_string(),
            ..serde_json::from_value(serde_json::json!({"name":"h","kind":"local"}))
                .expect("a minimal target")
        };
        let error = futures::executor::block_on(repair(&target, &requirement, &runner))
            .expect_err("a refusal");
        assert!(error.0.contains("not an Appium version coordinate"));
    }

    #[test]
    fn a_driver_name_that_is_not_a_name_is_refused_too() {
        let runner = crate::deploy::production_runner();
        let mut requirement = declared();
        requirement.drivers = vec!["xcuitest && curl evil".to_string()];
        let target: ComputeTarget =
            serde_json::from_value(serde_json::json!({"name":"h","kind":"local"}))
                .expect("a minimal target");
        let error = futures::executor::block_on(repair(&target, &requirement, &runner))
            .expect_err("a refusal");
        assert!(error.0.contains("not an Appium driver name"));
    }

    #[test]
    fn the_declaration_round_trips_through_the_registry_shape() {
        let parsed: MobileRuntime = serde_json::from_value(serde_json::json!({
            "appium": "3.2.1",
            "drivers": ["xcuitest", "uiautomator2"],
            "platform_tools": true,
            "future_key":
                7
        }))
        .expect("a declaration");
        assert_eq!(parsed.appium, "3.2.1");
        assert_eq!(parsed.drivers.len(), 2);
        assert!(parsed.platform_tools);
        // A newer publisher's key survives a rewrite from this checkout.
        assert!(parsed.extra.contains_key("future_key"));
    }
}
