//! One round trip to a host: read every component of its declared runtime in
//! a single ssh connection, and judge what came back.
//!
//! The install that repairs what this reads is `install`, the shell programs
//! both halves send are `verify_script` and `install_script`, and the reading
//! of the Appium server's own listing is `diagnostics`.

mod diagnostics;
mod install;
mod install_script;
mod verify_script;

pub use diagnostics::{incompatible_drivers, installed_driver_version};
pub use install::repair;

use serde_json::Value;

use self::verify_script::REMOTE_VERIFY_BODY;
use super::paths::{with_candidates, ANDROID_SDK_ROOT, NPM_PREFIX};
use super::report::{
    ComponentState, RuntimeReport, COMPONENT_DRIFTED, COMPONENT_MISSING, COMPONENT_PRESENT,
    COMPONENT_UNDECLARED_INCOMPATIBLE, COMPONENT_UNKNOWN,
};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::{ComputeTarget, MobileRuntime};

/// Everything the host said about its runtime, judged against the
/// declaration.
pub async fn verify(
    target: &ComputeTarget,
    declared: &MobileRuntime,
    runner: &Runner,
) -> Result<RuntimeReport, DeployError> {
    let script = with_candidates(REMOTE_VERIFY_BODY);
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
    let field = |name: &str| {
        parsed
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    let mut components = Vec::new();

    let appium_path = field("appium_path");
    let appium_version = field("appium_version");
    // The declaration is an exact version, so a present-but-different Appium
    // is `drifted` and not `present`: a placement pinned to a driver protocol
    // that its server does not speak fails at the first command, which is the
    // failure this is meant to precede rather than reproduce.
    let appium_state = if appium_path.is_empty() {
        COMPONENT_MISSING
    } else if appium_version.is_empty() {
        COMPONENT_UNKNOWN
    } else if appium_version.trim() == declared.appium.trim() {
        COMPONENT_PRESENT
    } else {
        COMPONENT_DRIFTED
    };
    components.push(ComponentState {
        name: "appium".to_string(),
        declared: declared.appium.clone(),
        path: if appium_path.is_empty() {
            format!("{NPM_PREFIX}/bin/appium and 3 more")
        } else {
            appium_path
        },
        observed: appium_version,
        state: appium_state.to_string(),
    });

    let installed_drivers = field("drivers");
    for driver in &declared.drivers {
        // The listing's columns differ between Appium 2 and 3, so this reads
        // the one shape both spell the same way: `<name>@<version>`. Reporting
        // the version and not just the name matters here — this host carried
        // an Appium 2-era driver set under a `$APPIUM_HOME` that outlived its
        // server, and "installed" alone would have called that agreement.
        let installed_version = installed_driver_version(&installed_drivers, driver);
        let present = installed_version.is_some()
            || installed_drivers
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
                .any(|word| word == driver.as_str());
        components.push(ComponentState {
            name: format!("driver:{driver}"),
            declared: "required".to_string(),
            path: "appium driver list --installed".to_string(),
            observed: installed_version.unwrap_or_else(|| {
                if present {
                    "installed".to_string()
                } else {
                    String::new()
                }
            }),
            state: if present {
                COMPONENT_PRESENT.to_string()
            } else {
                COMPONENT_MISSING.to_string()
            },
        });
    }

    if declared.platform_tools {
        let adb_path = field("adb_path");
        let adb_version = field("adb_version");
        let adb_state = if adb_path.is_empty() {
            COMPONENT_MISSING
        } else if adb_version.is_empty() {
            COMPONENT_UNKNOWN
        } else {
            COMPONENT_PRESENT
        };
        components.push(ComponentState {
            name: "adb".to_string(),
            declared: "required".to_string(),
            path: if adb_path.is_empty() {
                format!("{ANDROID_SDK_ROOT}/platform-tools/adb and 2 more")
            } else {
                adb_path
            },
            observed: adb_version,
            state: adb_state.to_string(),
        });
    }

    // Visible, not judged. The server named these itself; a declaration that
    // says nothing about them cannot fail on them, and a report that omitted
    // them is how `charless-mac-mini` kept a driver the server refuses to
    // host, waiting to deadlock npm for the next install into that tree.
    for (driver, server_said) in incompatible_drivers(&field("warnings")) {
        if declared.drivers.contains(&driver) {
            continue;
        }
        let version = installed_driver_version(&installed_drivers, &driver)
            .unwrap_or_else(|| "installed".to_string());
        components.push(ComponentState {
            name: format!("driver:{driver}"),
            declared: "undeclared".to_string(),
            path: "appium driver list --installed".to_string(),
            // The server's own sentence, so an operator reads why rather than
            // a state name this module invented.
            observed: format!("{version} — {server_said}"),
            state: COMPONENT_UNDECLARED_INCOMPATIBLE.to_string(),
        });
    }

    Ok(RuntimeReport { components })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_remote_scripts_resolve_the_paths_the_allowlist_probes() {
        // One table, two readers: the script the host runs must carry the same
        // candidates `host_exec` uses, or the two could disagree about which
        // binary a machine has.
        let script = with_candidates(REMOTE_VERIFY_BODY);
        assert!(!script.contains("@APPIUM_CANDIDATES@"));
        assert!(!script.contains("@ADB_CANDIDATES@"));
        assert!(!script.contains("@NODE_CANDIDATES@"));
        for candidate in
            crate::deploy::host_exec::program_candidates(crate::deploy::host_exec::APPIUM_PROGRAM)
                .expect("appium is in the table")
        {
            let expected = candidate
                .strip_prefix("~/")
                .map_or_else(|| (*candidate).to_string(), |rest| rest.to_string());
            assert!(script.contains(&expected), "{expected} missing from script");
        }
        // A home-relative candidate is expanded by the host, not here.
        assert!(script.contains("\"$HOME\"/"));
    }
}
