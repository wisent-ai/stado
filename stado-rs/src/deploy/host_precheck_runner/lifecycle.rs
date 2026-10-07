//! Restarting, repairing and removing a runner that is already installed.

use serde_json::{json, Value};

use crate::deploy::host_precheck_runner::accounts::github::{
    github_runner, github_runner_token, RunnerRecord,
};
use crate::deploy::host_precheck_runner::declaration::{
    runner_profile, runner_target, RunnerProfile,
};
use crate::deploy::host_precheck_runner::linux::scripts::LINUX_REMOVE;
use crate::deploy::host_precheck_runner::macos::runtime::{
    MACOS_RUNTIME_FUNCTIONS, MACOS_RUNTIME_REPAIR,
};
use crate::deploy::host_precheck_runner::macos::scripts::MACOS_REMOVE;
use crate::deploy::host_precheck_runner::platform::{profile_template, replace, Platform};
use crate::deploy::host_precheck_runner::role::declare_runner_role;
use crate::deploy::host_precheck_runner::verdict::report::{command_failure, report};
use crate::deploy::host_precheck_runner::verdict::scope::{scope_for_profile, RunnerScope};
use crate::deploy::{host_channel, production_runner, service, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Restore the upstream apphosts of the macOS GitHub runners a unit runs, in
/// place. Each listener's own retry loop picks up the repaired files; no unit
/// is cycled.
///
/// The host's Stado unit names each runner it runs by its `--precheck-runner
/// <ROOT>` role; an adopted runner unit (`com.wisent.actions-runner.<name>`)
/// names its root by the launcher it starts.
pub async fn repair_runtime(
    target: &ComputeTarget,
    managed: &service::ManagedService,
    runner: &Runner,
) -> Result<Value, DeployError> {
    if Platform::for_target(target)? != Platform::DarwinArm64 {
        return Err(DeployError(
            "runner runtime repair requires a darwin-arm64 host".to_string(),
        )
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    let roots = runner_roots(target, managed, runner).await?;
    let mut repairs = Vec::with_capacity(roots.len());
    for root in roots {
        // `run_runner_reconciliation` names the host's Stado as `STADO_BIN`
        // and refuses a host whose Stado cannot sign.
        let script = replace(
            MACOS_RUNTIME_REPAIR,
            &[
                ("__RUNNER_ROOT__", shlex_quote(&root)),
                (
                    "__MACOS_RUNTIME_FUNCTIONS__",
                    MACOS_RUNTIME_FUNCTIONS.to_string(),
                ),
            ],
        );
        let output =
            crate::deploy::native_signing::run_runner_reconciliation(target, &script, runner)
                .await?;
        if !output.ok() {
            return Err(DeployError::unreachable(format!(
                "{}: runner runtime repair of {root} failed: {} {}",
                target.name, output.stdout, output.stderr
            )));
        }
        repairs.push(json!({
            "runner_root": root,
            "stdout": output.stdout,
            "stderr": output.stderr,
        }));
    }
    Ok(json!({
        "target": target.name,
        "unit": managed.unit_id(),
        "action": "repair-runtime",
        "restarted": false,
        "repairs": repairs,
        "stdout": repairs_text(&repairs, "stdout"),
        "stderr": repairs_text(&repairs, "stderr"),
    }))
}

fn repairs_text(repairs: &[Value], key: &str) -> String {
    repairs
        .iter()
        .filter_map(|repair| repair[key].as_str())
        .collect::<Vec<_>>()
        .join("")
}

/// The runner roots `managed` is responsible for.
async fn runner_roots(
    target: &ComputeTarget,
    managed: &service::ManagedService,
    runner: &Runner,
) -> Result<Vec<String>, DeployError> {
    let option = format!(
        "{}=",
        crate::deploy::host_precheck_runner::role::RUNNER_ROLE
    );
    let roles: Vec<String> = managed
        .args
        .iter()
        .filter_map(|argument| argument.strip_prefix(&option))
        .map(str::to_string)
        .collect();
    if !roles.is_empty() {
        return Ok(roles);
    }
    if !managed.unit_id().starts_with("com.wisent.actions-runner.") {
        return Err(DeployError(format!(
            "{} runs no {} role and is not an adopted runner unit; `stado runner install` \
             declares the role",
            managed.unit_id(),
            crate::deploy::host_precheck_runner::role::RUNNER_ROLE
        ))
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    // An adopted runner whose unit starts `start-runner.sh` beside its
    // install must be accepted here, because its apphosts can be the ones
    // failing (`Failed to create CoreCLR, HRESULT: 0x8007000C`). The repair
    // script itself checks that the directory is a runner install.
    let unit = service::fetch_unit_file(target, managed, runner).await?;
    let program = service::parse_unit_program(&unit)?
        .ok_or_else(|| {
            DeployError::unreachable("runner unit declares no executable".to_string())
        })?;
    let path = std::path::Path::new(&program);
    if !path.is_absolute()
        || path
            .file_name()
            .is_none_or(|name| name != "start-runner.sh" && name != "runsvc.sh")
    {
        return Err(DeployError(
            "an adopted runner unit must directly declare GitHub's runsvc.sh or the \
             start-runner.sh beside its install"
                .to_string(),
        )
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    let mut root = path
        .parent()
        .ok_or_else(|| {
            DeployError("runner has no install directory".to_string())
                .stating(crate::primitives::failure::FailureCode::Refused)
        })?;
    if root.file_name().is_some_and(|name| name == "bin") {
        root = root
            .parent()
            .ok_or_else(|| {
                DeployError("runner has no install directory".to_string())
                    .stating(crate::primitives::failure::FailureCode::Refused)
            })?;
    }
    Ok(vec![root.to_string_lossy().into_owned()])
}

/// Restart one declared runner: its listener is a role of the host's Stado
/// unit, so the role is taken off that unit and switched back on, which
/// restarts the unit twice and the listener with it. GitHub's own view of
/// the runner is the proof it came back, read the same way `install` reads
/// it.
pub async fn restart_declared(
    target_name: &str,
    profile_name: &str,
    repository: Option<&str>,
) -> Result<Value, DeployError> {
    let profile = runner_profile(profile_name)?;
    let scope = scope_for_profile(profile, repository)?;
    let target = runner_target(target_name).await?;
    let platform = Platform::for_target(&target)?;
    profile.installer_kind(platform.name())?;
    let runner_root = platform.runner_root(profile);
    let reason = format!(
        "{} runner at {runner_root} restarted: its listener role is cycled on this host's \
         Stado unit",
        profile.name
    );
    let off = declare_runner_role(&target.name, &runner_root, true, &reason).await?;
    if off == "absent" {
        return Err(DeployError(format!(
            "{}: {} runner is not a role of this host's Stado unit; `stado runner install` \
             declares it",
            target.name, profile.name
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound));
    }
    let on = declare_runner_role(&target.name, &runner_root, false, &reason).await?;
    let runner_name = format!("{}-{}", profile.slug, target.name);
    let status = match github_runner(&scope, &runner_name).await {
        RunnerRecord::Present { status } => status,
        RunnerRecord::Absent { listed } => {
            return Err(DeployError::unreachable(format!(
                "{}: {} restarted, but GitHub lists no runner named {runner_name} under {}; \
                 listed runners: {listed:?}",
                target.name,
                profile.name,
                scope.label()
            )));
        }
        RunnerRecord::Unreadable { detail } => {
            return Err(DeployError::unreachable(format!(
                "{}: {} restart cannot be verified at {}: {detail}",
                target.name,
                profile.name,
                scope.label()
            )));
        }
    };
    Ok(json!({
        "host": target.name,
        "profile": profile.name,
        "action": "restart",
        "role": { "taken_off": off, "ensure": on },
        "registration": { "scope": scope.label(), "runner": runner_name, "status": status },
    }))
}

async fn remove_profile(
    target_name: &str,
    profile: &RunnerProfile,
    scope: &RunnerScope,
) -> Result<Value, DeployError> {
    let target = runner_target(target_name).await?;
    let platform = Platform::for_target(&target)?;
    profile.installer_kind(platform.name())?;
    let runner_root = platform.runner_root(profile);
    // The listener stops with its role, before the host forgets the
    // registration it was listening under.
    let role = declare_runner_role(
        &target.name,
        &runner_root,
        true,
        &format!(
            "{} runner at {runner_root} removed: its listener role leaves this host's Stado unit",
            profile.name
        ),
    )
    .await?;
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
    let output = host_channel::run_script(&target, &script, &production_runner()).await?;
    let mut value = report(&target, &output, "remove", profile);
    if !output.ok() {
        return Err(DeployError::unreachable(format!(
            "{}: {} runner removal failed: {}",
            target.name,
            profile.name,
            command_failure(&output, "remote removal failed")
        )));
    }
    value["role"] = json!({ "taken_off": role });
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
