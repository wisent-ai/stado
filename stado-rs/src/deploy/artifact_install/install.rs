//! The two operations a caller performs: put one version on the host, and
//! name the path a unit definition must run.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use super::script::{INSTALL_ARCHIVE_BODY, INSTALL_BODY, PRUNE_BODY};
use super::validate::{primary_location, validate_service_name, version_segment};
use super::{InstalledArtifact, SERVICES_ROOT};
use crate::artifacts_models::ArtifactManifest;
use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Place one artifact version on the host and point `current` at it.
///
/// Returns the path the unit should run: always through `current`, never the
/// version directory, so a later install moves every unit forward without
/// re-rendering any of them.
pub async fn install_artifact(
    target: &ComputeTarget,
    name: &str,
    manifest: &ArtifactManifest,
    runner: &Runner,
) -> Result<InstalledArtifact, DeployError> {
    validate_service_name(name)?;
    let version = version_segment(&manifest.ref_)?;
    let location = primary_location(manifest)?;
    if location.sha256.trim().is_empty() {
        return Err(DeployError(format!(
            "artifact {} declares no sha256 for its primary location; \
             an unverifiable download must not become a running unit",
            manifest.ref_
        )));
    }

    // A bundle is not a program. When the manifest says the location is an
    // archive, the verified download is unpacked into the version directory
    // instead of becoming the executable itself -- brama ships its launcher,
    // its entitlements router and its config beside the binary, and installing
    // only the tarball would leave `current` pointing at a tarball.
    let archive = manifest.labels.get("archive").map(String::as_str);
    let subdir = manifest
        .labels
        .get("extract_subdir")
        .map(String::as_str)
        .unwrap_or_default();
    if subdir.contains("..") || subdir.starts_with('/') {
        return Err(DeployError(format!(
            "artifact {} declares an unusable extract_subdir {subdir:?}",
            manifest.ref_
        )));
    }
    let body = match archive {
        Some("tar.gz" | "tgz") => INSTALL_ARCHIVE_BODY,
        Some(other) => {
            return Err(DeployError(format!(
                "artifact {} declares an unsupported archive format {other:?}",
                manifest.ref_
            )))
        }
        None => INSTALL_BODY,
    };
    let script = format!("{body}{PRUNE_BODY}")
        .replace("@SERVICES_ROOT@", SERVICES_ROOT)
        .replace("@NAME@", name)
        .replace("@VERSION@", &version)
        .replace("@SUBDIR@", subdir)
        .replace("@URI@", &shlex_quote(&location.uri))
        .replace("@SHA256@", &location.sha256);
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: could not install artifact {}: {}",
            target.name,
            manifest.ref_,
            host_channel::last_error_line(&output, "install failed")
        )));
    }

    Ok(InstalledArtifact {
        program_path: format!("$HOME/{SERVICES_ROOT}/{name}/current/{name}"),
        version,
        sha256: location.sha256.clone(),
    })
}

/// The same path the install reports, resolved for a unit definition.
///
/// `deploy` validates an absolute path, and `$HOME` is not one, so the caller
/// needs the expanded form. The home directory comes from the host rather than
/// from this machine: a target's account is its own business.
pub async fn resolve_program_path(
    target: &ComputeTarget,
    name: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    validate_service_name(name)?;
    let script = format!(
        "set -eu\necho \"STADO_HOME=$HOME\"\n_={}\n",
        STANDARD.encode(name.as_bytes())
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    let home = output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_HOME="))
        .map(str::trim)
        .filter(|value| value.starts_with('/'))
        .ok_or_else(|| {
            DeployError(format!(
                "{}: could not resolve the home directory for the unit path",
                target.name
            ))
        })?;
    Ok(format!("{home}/{SERVICES_ROOT}/{name}/current/{name}"))
}
