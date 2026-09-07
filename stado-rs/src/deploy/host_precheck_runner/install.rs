//! Installing one declared runner on one host, with everything its profile
//! declares it needs before the installer runs.

use std::time::Duration;

use serde_json::{json, Value};

use super::brama::{brama_identity_host, private_brama_route};
use super::credentials::kronika_agent_credential;
use super::declaration::{runner_profile, runner_target, RunnerProfile};
use super::developer_id::bootstrap_developer_id;
use super::github::{github_runner_is_online, github_runner_token};
use super::installer::{
    installer_program, InstallerRequest, PROBIERZ_AGENT_ID, PROBIERZ_AGENT_RESOURCE,
};
use super::model_review::reconcile_model_review_secret;
use super::platform::Platform;
use super::publisher::bootstrap_publisher_repository;
use super::report::{command_failure, report};
use super::scope::{scope_for_profile, RunnerScope};
use crate::deploy::{host_channel, production_runner, shlex_quote, DeployError};
use crate::targets::ComputeTarget;

async fn install_kronika_agent_secret(
    target: &ComputeTarget,
    platform: Platform,
    profile: &RunnerProfile,
    secret: &str,
) -> Result<String, DeployError> {
    let runner = production_runner();
    let secret_file = platform.kronika_agent_secret_file(profile);
    let runner_user = profile.slug.as_str();
    // macOS install(1) rejects /dev/stdin as a source. Create the destination
    // with its final owner and mode first, then let that owner replace only its
    // bytes through dd. The secret never appears in argv or command output.
    let prepared = host_channel::run_program(
        target,
        &[
            "/usr/bin/sudo",
            "-n",
            "/usr/bin/install",
            "-o",
            runner_user,
            "-g",
            runner_user,
            "-m",
            "600",
            "/dev/null",
            &secret_file,
        ],
        &runner,
    )
    .await?;
    if !prepared.ok() {
        return Err(DeployError(format!(
            "{}: cannot prepare the Probierz signing credential file: {}",
            target.name,
            command_failure(&prepared, "remote secret file preparation failed")
        )));
    }
    let destination = format!("of={secret_file}");
    let written = host_channel::run_program_with_stdin(
        target,
        &[
            "/usr/bin/sudo",
            "-n",
            "-u",
            runner_user,
            "/bin/dd",
            &destination,
            "bs=4096",
        ],
        secret,
        &runner,
    )
    .await?;
    if !written.ok() {
        return Err(DeployError(format!(
            "{}: Probierz signing identity installation failed: {}",
            target.name,
            command_failure(&written, "remote secret write failed")
        )));
    }
    Ok(secret_file)
}

async fn install_profile(
    target_name: &str,
    profile: &RunnerProfile,
    scope: &RunnerScope,
) -> Result<Value, DeployError> {
    let target = runner_target(target_name).await?;
    let platform = Platform::for_target(&target)?;
    profile.installer_kind(platform.name())?;
    let (brama_url, brama_port) = private_brama_route(target_name).await?;
    let kronika_credential = if profile.needs_kronika() {
        Some(kronika_agent_credential(&brama_identity_host(&target).await?).await?)
    } else {
        None
    };
    let runner_root = match platform {
        Platform::LinuxAmd64 => format!("/opt/wisent/{}-runner", profile.slug),
        Platform::DarwinArm64 => format!("/Users/Shared/{}-runner", profile.slug),
    };
    // Labels, group, and actual registration scope are fixed when config.sh
    // runs. Reconcile whenever the host record differs from this declaration.
    let registration = format!(
        "printf '%s\\n%s\\n%s\\n' {} {} {}",
        shlex_quote(&profile.labels_text()),
        shlex_quote(scope.group(profile)),
        shlex_quote(&scope.label())
    );
    let probe = format!(
        "test -f {root}/.runner && test -f {root}/.stado/registered-runner && \
         {registration} | /usr/bin/diff -q - {root}/.stado/registered-runner >/dev/null",
        root = shlex_quote(&runner_root)
    );
    let already_registered = host_channel::run_script(&target, &probe, &production_runner())
        .await?
        .ok();
    let token = if already_registered {
        String::new()
    } else {
        github_runner_token(scope, "registration").await?
    };
    let runner_name = format!("{}-{}", profile.slug, target.name);
    let restart_registered = already_registered
        && profile.needs_publisher_bootstrap()
        && !github_runner_is_online(&runner_name).await?;
    let script = installer_program(
        &InstallerRequest {
            profile_name: &profile.name,
            target_name: &target.name,
            platform_name: platform.name(),
            repository: match &scope {
                RunnerScope::Organization => None,
                RunnerScope::Repository(repository) => Some(repository.as_str()),
            },
        },
        &token,
        &brama_url,
        brama_port,
        restart_registered,
    )?;
    let output = host_channel::run_script_with_timeout(
        &target,
        &script,
        Duration::from_secs(15 * 60),
        &production_runner(),
    )
    .await?;
    let mut value = report(&target, &output, "install", profile);
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: {} runner installation failed: {}",
            target.name,
            profile.name,
            command_failure(&output, "remote installer failed")
        )));
    }
    if let Some(kronika_credential) = kronika_credential {
        let secret_file =
            install_kronika_agent_secret(&target, platform, profile, &kronika_credential.secret)
                .await?;
        value["kronika_identity"] = json!({
            "agent_id": PROBIERZ_AGENT_ID,
            "resource": PROBIERZ_AGENT_RESOURCE,
            "secret_item": kronika_credential.item,
            "secret_field": kronika_credential.field,
            "secret_file": secret_file,
            "status": "installed",
        });
    }
    Ok(value)
}

/// Install one profile, including the profile's repository-scoped setup.
pub async fn install_declared(
    target_name: &str,
    profile_name: &str,
    repository: Option<&str>,
) -> Result<Value, DeployError> {
    let profile = runner_profile(profile_name)?;
    let scope = scope_for_profile(profile, repository)?;
    // Resolve before any repository mutation so a typo cannot publish secrets.
    runner_target(target_name).await?;

    let repository_bootstrap = if let Some(repository) = repository {
        if profile.needs_publisher_bootstrap() {
            let bootstrap = bootstrap_publisher_repository(repository).await?;
            let developer_id = match &profile.developer_id_account_item {
                Some(account_item) => {
                    bootstrap_developer_id(target_name, account_item, &[repository.to_string()])
                        .await?
                }
                None => Value::Null,
            };
            Some(json!({
                "release": bootstrap,
                "developer_id": developer_id,
            }))
        } else {
            None
        }
    } else {
        None
    };
    let model_review = if profile.needs_kronika() {
        match repository {
            Some(repository) => Some(reconcile_model_review_secret(target_name, repository).await?),
            None => None,
        }
    } else {
        None
    };

    let mut value = install_profile(target_name, profile, &scope).await?;
    if let Some(repository_bootstrap) = repository_bootstrap {
        value["repository_bootstrap"] = repository_bootstrap;
    }
    if let Some(model_review) = model_review {
        value["model_review"] = model_review;
    }
    Ok(value)
}
