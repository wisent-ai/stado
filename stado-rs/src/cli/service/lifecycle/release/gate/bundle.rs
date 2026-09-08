//! What one release consists of: the qualified artifact, its bytes staged
//! locally, and the version the host is running now.

use super::*;

pub(super) async fn service_release_bundle(
    options: &ServiceReleaseOptions<'_>,
    target: &targets::ComputeTarget,
    declared: &ManagedService,
) -> Result<ServiceReleaseBundle, CmdError> {
    let document = registry::fetch_document().await?;
    let control = crate::release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    let policy = control.products.get(options.product).ok_or_else(|| {
        CmdError::click(format!(
            "registry.release_control declares no product {:?}",
            options.product
        ))
    })?;
    let target_policy = policy.targets.get(options.host).ok_or_else(|| {
        CmdError::click(format!(
            "product {:?} has no release target {:?}",
            options.product, options.host
        ))
    })?;
    let exact_legacy_unit = target_policy
        .legacy_launchd_label
        .as_deref()
        .is_some_and(|label| label == declared.unit_id());
    if policy.service != options.name && policy.service != declared.name && !exact_legacy_unit {
        return Err(CmdError::click(format!(
            "product {:?} releases service {:?}, not unit {:?}",
            options.product,
            policy.service,
            declared.unit_id()
        )));
    }
    if target_policy.platform != target.release_platform {
        return Err(CmdError::click(format!(
            "release target platform {:?} disagrees with host platform {:?}",
            target_policy.platform, target.release_platform
        )));
    }
    let desired = policy.desired.as_ref().ok_or_else(|| {
        CmdError::click(format!(
            "product {:?} has no desired release",
            options.product
        ))
    })?;
    if desired.version != options.version {
        return Err(CmdError::click(format!(
            "product {:?} desires {}, not {}",
            options.product, desired.version, options.version
        )));
    }
    let artifact = desired
        .artifacts
        .get(&target_policy.platform)
        .cloned()
        .ok_or_else(|| {
            CmdError::click(format!(
                "desired release has no artifact for {:?}",
                target_policy.platform
            ))
        })?;
    let (_, archive, _) = crate::release_agent::fetch_candidate(
        &control,
        options.product,
        desired,
        &artifact,
        policy,
        target_policy,
    )
    .await
    .map_err(CmdError::click)?;

    let observed_uri = crate::release_agent::release_status_uri(options.product, options.host);
    let observed = match crate::cli::storage::fetch_object(&observed_uri).await {
        Ok(bytes) => serde_json::from_slice::<ObservedServiceRelease>(&bytes).unwrap_or_default(),
        Err(_) => ObservedServiceRelease::default(),
    };
    let previous_version = observed.active_version.or_else(|| {
        policy
            .previous
            .as_ref()
            .map(|release| release.version.clone())
    });
    let previous_sha256 = observed.active_sha256.or_else(|| {
        policy.previous.as_ref().and_then(|release| {
            release
                .artifacts
                .get(&target_policy.platform)
                .map(|artifact| artifact.artifact_sha256.clone())
        })
    });
    Ok(ServiceReleaseBundle {
        artifact,
        archive,
        rollout_generation: desired.rollout_generation,
        previous_version,
        previous_sha256,
    })
}

pub(super) fn stage_service_release_archive(
    product: &str,
    version: &str,
    platform: &str,
    archive: &[u8],
) -> Result<std::path::PathBuf, CmdError> {
    let root = crate::config_file::expand_tilde("~")
        .join(".stado/work/service-release")
        .join(product)
        .join(version);
    std::fs::create_dir_all(&root)?;
    let path = root.join(format!("{platform}.tar.gz"));
    std::fs::write(&path, archive)?;
    Ok(path)
}

pub(super) async fn current_service_version(
    target: &targets::ComputeTarget,
    directory: &str,
    runner: &crate::deploy::Runner,
) -> Result<String, CmdError> {
    let script = format!(
        "set -euo pipefail\nname={}\nroot=\"$HOME/.stado/services/$name\"\n\
         target=$(/usr/bin/readlink \"$root/current\")\n\
         /usr/bin/basename \"$target\"",
        crate::deploy::shlex_quote(directory),
    );
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(host_channel::last_error_line(
            &output,
            "current service version is unreadable",
        )));
    }
    let version = output.stdout.trim();
    if version.is_empty() {
        return Err(CmdError::click("current service version is empty"));
    }
    Ok(version.to_string())
}
