//! What one runner, one host, or the whole fleet reports about its runners.

use serde_json::{json, Value};

use crate::deploy::host_precheck_runner::accounts::brama::private_brama_route;
use crate::deploy::host_precheck_runner::declaration::{runner_declaration, runner_profile, runner_target, RunnerProfile};
use crate::deploy::host_precheck_runner::linux::scripts::{LINUX_PUBLISHER_STATUS, LINUX_STATUS};
use crate::deploy::host_precheck_runner::macos::runtime::MACOS_PUBLISHER_STATUS;
use crate::deploy::host_precheck_runner::macos::scripts::MACOS_STATUS;
use crate::deploy::host_precheck_runner::platform::{profile_template, Platform};
use crate::deploy::host_precheck_runner::verdict::report::{command_failure, registered_scope, report, unavailable_status};
use crate::deploy::{host_channel, production_runner, DeployError};

/// The shape of [`fleet_report`], which a reader checks before trusting the
/// fields below it.
const FLEET_REPORT_SCHEMA_VERSION: u64 = 1;

async fn status_profile(target_name: &str, profile: &RunnerProfile) -> Result<Value, DeployError> {
    let target = runner_target(target_name).await?;
    let platform = Platform::for_target(&target)?;
    let installer = profile.installer_kind(platform.name())?;
    let script = if installer.starts_with("publisher-") {
        match platform {
            Platform::LinuxAmd64 => profile_template(LINUX_PUBLISHER_STATUS, profile),
            Platform::DarwinArm64 => profile_template(MACOS_PUBLISHER_STATUS, profile),
        }
    } else {
        profile_template(
            match platform {
                Platform::LinuxAmd64 => LINUX_STATUS,
                Platform::DarwinArm64 => MACOS_STATUS,
            },
            profile,
        )
    };
    let output = host_channel::run_script(&target, &script, &production_runner()).await?;
    let mut value = report(&target, &output, "status", profile);
    if !output.ok() {
        value["installed"] = json!(registered_scope(&output.stdout).is_some());
        value["error"] = json!(format!(
            "{}: {} runner status failed: {}",
            target.name,
            profile.name,
            command_failure(&output, "remote status failed")
        ));
        return Ok(value);
    }
    if profile.needs_kronika() {
        value["brama_route"] = brama_route_verdict(target_name, &output.stdout).await?;
    }
    Ok(value)
}

/// Whether the Brama address the runner actually dials is the one the service
/// directory declares for its host.
///
/// The installer derives that address once and writes it into
/// `routes/brama.url`; nothing re-derived it afterwards and nothing compared
/// the two. When Brama's endpoint on `charless-mac-mini` moved from `8080` to
/// `18080`, the file kept the old port, and the only symptom was `kronika:
/// fetch failed` in the CI of a DIFFERENT repository — the Skarbiec
/// documentation gate, which is also what `build`, `deploy` and `tag` there
/// depend on, so no Skarbiec release could be published at all. A published
/// address that nothing compares to its declaration is the same defect this
/// fleet has already paid for in `~/.stado/forwards/<service>.local`.
/// A verdict rather than an error, so the rest of the status — the account,
/// the boundary, the signing secret, the listener's own last event — still
/// reaches the operator. The command exits non-zero on a drifted route; it
/// does so after printing what it saw.
async fn brama_route_verdict(target_name: &str, stdout: &str) -> Result<Value, DeployError> {
    let published = stdout
        .lines()
        .filter_map(|line| line.trim().strip_prefix("brama route:"))
        .map(str::trim)
        .next_back()
        .unwrap_or_default()
        .to_string();
    let (declared, _) = private_brama_route(target_name).await?;
    if published.is_empty() {
        return Ok(json!({
            "published": Value::Null,
            "declared": declared,
            "matches": false,
            "detail": format!(
                "the precheck runner publishes no Brama route, so the documentation gate on \
                 this host dials nothing. Reinstall it: stado runner install {target_name} \
                 --profile precheck"
            ),
        }));
    }
    if published != declared {
        return Ok(json!({
            "published": published,
            "declared": declared,
            "matches": false,
            "detail": format!(
                "the precheck runner dials {published} and the service directory declares \
                 {declared} for this host. Every Kronika documentation gate on this runner \
                 fails with `fetch failed` until the two agree: correct whichever is wrong, \
                 then republish with stado runner install {target_name} --profile precheck"
            ),
        }));
    }
    Ok(json!({
        "published": published,
        "declared": declared,
        "matches": true,
    }))
}

pub async fn status_declared(target_name: &str, profile_name: &str) -> Result<Value, DeployError> {
    status_profile(target_name, runner_profile(profile_name)?).await
}

/// Read every declared profile from one registered host.
pub async fn status_all(target_name: &str) -> Result<Value, DeployError> {
    let target = runner_target(target_name).await?;
    let mut profiles = Vec::new();
    for profile in &runner_declaration()?.profiles {
        profiles.push(
            status_profile(target_name, profile)
                .await
                .unwrap_or_else(|error| unavailable_status(&target, profile, error)),
        );
    }
    Ok(json!({
        "target": target.name,
        "profiles": profiles,
    }))
}

/// Read runner state across every local host in the canonical registry.
pub async fn fleet_report() -> Result<Value, DeployError> {
    let registry = host_channel::canonical_registry().await?;
    let declaration = runner_declaration()?;
    let mut hosts = Vec::new();
    for target in registry
        .targets
        .iter()
        .filter(|target| target.is_provider(crate::capabilities::ProviderId::Local))
    {
        let mut profiles = Vec::new();
        for profile in &declaration.profiles {
            profiles.push(
                status_profile(&target.name, profile)
                    .await
                    .unwrap_or_else(|error| unavailable_status(target, profile, error)),
            );
        }
        hosts.push(json!({
            "target": target.name,
            "profiles": profiles,
        }));
    }
    Ok(json!({
        "schema_version": FLEET_REPORT_SCHEMA_VERSION,
        "hosts": hosts,
    }))
}
