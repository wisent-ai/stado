//! Restarting, repairing and removing a runner that is already installed.

use std::time::Duration;

use serde_json::{json, Value};

use crate::deploy::host_precheck_runner::accounts::github::github_runner_token;
use crate::deploy::host_precheck_runner::declaration::{
    runner_profile, runner_target, RunnerProfile,
};
use crate::deploy::host_precheck_runner::linux::scripts::{LINUX_REMOVE, LINUX_RESTART};
use crate::deploy::host_precheck_runner::macos::runtime::{
    MACOS_RUNTIME_FUNCTIONS, MACOS_RUNTIME_REPAIR,
};
use crate::deploy::host_precheck_runner::macos::scripts::{MACOS_REMOVE, MACOS_RESTART};
use crate::deploy::host_precheck_runner::platform::{profile_template, replace, Platform};
use crate::deploy::host_precheck_runner::verdict::report::{command_failure, report};
use crate::deploy::host_precheck_runner::verdict::scope::{scope_for_profile, RunnerScope};
use crate::deploy::{host_channel, production_runner, service, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Restore the upstream apphosts of an adopted macOS GitHub runner in place.
/// Its existing listener retry loop picks up the repaired files; no unit is cycled.
pub async fn repair_runtime(
    target: &ComputeTarget,
    managed: &service::ManagedService,
    runner: &Runner,
) -> Result<Value, DeployError> {
    if Platform::for_target(target)? != Platform::DarwinArm64 {
        return Err(DeployError(
            "runner runtime repair requires a darwin-arm64 host".to_string(),
        ));
    }
    let unit = service::fetch_unit_file(target, managed, runner).await?;
    let program = service::parse_unit_program(&unit)?
        .ok_or_else(|| DeployError("runner unit declares no executable".to_string()))?;
    let path = std::path::Path::new(&program);
    if !path.is_absolute() || path.file_name().is_none_or(|name| name != "runsvc.sh") {
        return Err(DeployError(
            "runner unit must directly declare GitHub's runsvc.sh".to_string(),
        ));
    }
    let mut root = path
        .parent()
        .ok_or_else(|| DeployError("runner has no install directory".to_string()))?;
    if root.file_name().is_some_and(|name| name == "bin") {
        root = root
            .parent()
            .ok_or_else(|| DeployError("runner has no install directory".to_string()))?;
    }
    let script = replace(
        MACOS_RUNTIME_REPAIR,
        &[
            ("__RUNNER_ROOT__", shlex_quote(&root.to_string_lossy())),
            (
                "__MACOS_RUNTIME_FUNCTIONS__",
                MACOS_RUNTIME_FUNCTIONS.to_string(),
            ),
        ],
    );
    let output =
        host_channel::run_script_with_timeout(target, &script, Duration::from_secs(300), runner)
            .await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: runner runtime repair failed: {} {}",
            target.name, output.stdout, output.stderr
        )));
    }
    Ok(json!({
        "target": target.name,
        "unit": managed.unit_id(),
        "runner_root": root,
        "action": "repair-runtime",
        "restarted": false,
        "stdout": output.stdout,
        "stderr": output.stderr,
    }))
}

/// Restart one declared runner in place and wait for a fresh listener event.
pub async fn restart_declared(target_name: &str, profile_name: &str) -> Result<Value, DeployError> {
    let profile = runner_profile(profile_name)?;
    let target = runner_target(target_name).await?;
    let platform = Platform::for_target(&target)?;
    profile.installer_kind(platform.name())?;
    let script = profile_template(
        match platform {
            Platform::LinuxAmd64 => LINUX_RESTART,
            Platform::DarwinArm64 => MACOS_RESTART,
        },
        profile,
    );
    let output = host_channel::run_script_with_timeout(
        &target,
        &script,
        Duration::from_secs(4 * 60),
        &production_runner(),
    )
    .await?;
    let value = report(&target, &output, "restart", profile);
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: {} runner restart failed: {}",
            target.name,
            profile.name,
            command_failure(&output, "remote restart failed")
        )));
    }
    Ok(value)
}

async fn remove_profile(
    target_name: &str,
    profile: &RunnerProfile,
    scope: &RunnerScope,
) -> Result<Value, DeployError> {
    let target = runner_target(target_name).await?;
    let platform = Platform::for_target(&target)?;
    profile.installer_kind(platform.name())?;
    let token = github_runner_token(scope, "remove").await?;
    let script = replace(
        &profile_template(
            match platform {
                Platform::LinuxAmd64 => LINUX_REMOVE,
                Platform::DarwinArm64 => MACOS_REMOVE,
            },
            profile,
        ),
        &[("__TOKEN__", shlex_quote(&token))],
    );
    let output = host_channel::run_script_with_timeout(
        &target,
        &script,
        Duration::from_secs(5 * 60),
        &production_runner(),
    )
    .await?;
    let value = report(&target, &output, "remove", profile);
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: {} runner removal failed: {}",
            target.name,
            profile.name,
            command_failure(&output, "remote removal failed")
        )));
    }
    Ok(value)
}

pub async fn remove_declared(
    target_name: &str,
    profile_name: &str,
    repository: Option<&str>,
) -> Result<Value, DeployError> {
    let profile = runner_profile(profile_name)?;
    let scope = scope_for_profile(profile, repository)?;
    remove_profile(target_name, profile, &scope).await
}
