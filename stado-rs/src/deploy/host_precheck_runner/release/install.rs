//! Installing one declared runner on one host, with everything its profile
//! declares it needs before the installer runs.

use std::time::Duration;

use serde_json::{json, Value};

use crate::deploy::host_precheck_runner::accounts::brama::{brama_identity_host, private_brama_route};
use crate::deploy::host_precheck_runner::accounts::credentials::kronika_agent_credential;
use crate::deploy::host_precheck_runner::declaration::{runner_profile, runner_target, RunnerProfile};
use crate::deploy::host_precheck_runner::signing::developer_id::bootstrap_developer_id;
use crate::deploy::host_precheck_runner::accounts::github::{github_runner, github_runner_is_online, github_runner_token, RunnerRecord};
use crate::deploy::host_precheck_runner::release::installer::{
    installer_program, Decision, InstallerRequest, PROBIERZ_AGENT_ID, PROBIERZ_AGENT_RESOURCE,
};
use crate::deploy::host_precheck_runner::accounts::model_review::MODEL_REVIEW_SECRET;
use crate::deploy::host_precheck_runner::platform::Platform;
use crate::deploy::host_precheck_runner::release::publisher::bootstrap_publisher_repository;
use crate::deploy::host_precheck_runner::verdict::report::{command_failure, report};
use crate::deploy::host_precheck_runner::verdict::scope::{scope_for_profile, RunnerScope};
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
    let runner_root = platform.runner_root(profile);
    // Labels, group, and actual registration scope are fixed when config.sh
    // runs. Reconcile whenever the host record differs from this declaration.
    let registration = format!(
        "printf '%s\\n%s\\n%s\\n' {} {} {}",
        shlex_quote(&profile.labels_text()),
        shlex_quote(scope.group(profile)),
        shlex_quote(&scope.label())
    );
    let record = format!("{runner_root}/.stado/registered-runner");
    let configured = host_channel::run_script(
        &target,
        &format!("test -f {}/.runner", shlex_quote(&runner_root)),
        &production_runner(),
    )
    .await?
    .ok();
    let record_matches = host_channel::run_script(
        &target,
        &format!(
            "test -f {record} && {registration} | /usr/bin/diff -q - {record} >/dev/null",
            record = shlex_quote(&record)
        ),
        &production_runner(),
    )
    .await?
    .ok();
    let already_registered = configured && record_matches;
    // A host that carries a runner registered against something else is the
    // case this lifecycle used to skip: it wrote files, restarted nothing and
    // reported the profile installed while GitHub kept the old registration.
    let reconfigure = configured && !record_matches;
    let token = if already_registered {
        String::new()
    } else {
        github_runner_token(scope, "registration").await?
    };
    let runner_name = format!("{}-{}", profile.slug, target.name);
    let restart_registered = already_registered
        && profile.needs_publisher_bootstrap()
        && !github_runner_is_online(scope, &runner_name).await?;
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
        Decision {
            restart_registered,
            reconfigure,
        },
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
    // What the host did is not what GitHub holds. The registration is read
    // back from the scope it was made against, and an install that produced
    // no runner there is a failure however cleanly the program exited.
    let status = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match github_runner(scope, &runner_name).await {
                RunnerRecord::Present { status } if status == "online" => return Ok(status),
                RunnerRecord::Present { .. } => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                RunnerRecord::Absent { listed } => {
                    return Err(DeployError(format!(
                        "{}: {} installation exited successfully, but GitHub lists no runner \
                         named {runner_name} under {}; listed runners: {listed:?}",
                        target.name,
                        profile.name,
                        scope.label()
                    )));
                }
                RunnerRecord::Unreadable { detail } => {
                    return Err(DeployError(format!(
                        "{}: {} installation cannot be verified at {}: {detail}",
                        target.name,
                        profile.name,
                        scope.label()
                    )));
                }
            }
        }
    })
    .await
    .map_err(|_| {
        DeployError(format!(
            "{}: {runner_name} did not report online at {} within 60 seconds",
            target.name,
            scope.label()
        ))
    })??;
    value["registration"] = json!({
        "scope": scope.label(),
        "runner": runner_name,
        "present": true,
        "status": status,
        "reconfigured": reconfigure,
    });
    value["listener"] = json!({
        "connected": true,
        "state": format!("GitHub reports {runner_name} online at {}", scope.label()),
    });
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
    let mut value = install_profile(target_name, profile, &scope).await?;
    if let Some(repository_bootstrap) = repository_bootstrap {
        value["repository_bootstrap"] = repository_bootstrap;
    }
    // The model-review bearer is a Brama capability for the repository's CI,
    // not a property of the runner. Minting it inside `install` meant a Brama
    // that refused a route — HTTP 401 on `PUT /v1/admin/routes`, measured on
    // 2026-09-09 — stopped a repository from getting a runner at all, for a
    // secret its checks never read. `runner model-review` reconciles it, and
    // the report says so rather than leaving the capability unnamed.
    if profile.needs_model_review() {
        value["model_review"] = match repository {
            Some(repository) => json!({
                "secret": MODEL_REVIEW_SECRET,
                "state": "declared",
                "reconcile_with": format!(
                    "stado runner model-review {target_name} --repository {repository}"
                ),
            }),
            None => json!({
                "secret": MODEL_REVIEW_SECRET,
                "state": "repository-scoped",
                "reconcile_with": Value::Null,
            }),
        };
    }
    Ok(value)
}
