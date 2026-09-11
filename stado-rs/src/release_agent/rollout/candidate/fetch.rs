//! Fetch one candidate's signed manifest, its signature and its archive, and
//! refuse anything that is not the release the registry asked for.

use std::path::PathBuf;

use crate::release_control::{
    self, DesiredRelease, ProductReleasePolicy, QualificationStatus, ReleaseArtifactRef,
    ReleaseControl, ReleaseManifest, ReleaseTargetPolicy,
};

async fn fetch_release_bytes(uri: &str) -> Result<Vec<u8>, String> {
    // The release channel is served publicly over the object API.
    // Without STADO_API_URL, JobStorage::read_bytes on the canonical root
    // prefix serves a stale copy and the agent quarantines the release.
    crate::cli::storage::fetch_object(uri)
        .await
        .map_err(|e| e.to_string())
}

pub(crate) async fn fetch_candidate(
    control: &ReleaseControl,
    product: &str,
    desired: &DesiredRelease,
    artifact: &ReleaseArtifactRef,
    policy: &ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
) -> Result<(ReleaseManifest, Vec<u8>, PathBuf), String> {
    let manifest_bytes = fetch_release_bytes(&artifact.manifest_uri).await?;
    if release_control::sha256_bytes(&manifest_bytes) != artifact.manifest_sha256 {
        return Err("release manifest digest does not match desired state".to_string());
    }
    let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("invalid release manifest: {error}"))?;
    release_control::validate_manifest(&manifest)?;
    if manifest.product != product
        || manifest.version != desired.version
        || manifest.platform != target.platform
        || manifest.artifact_sha256 != artifact.artifact_sha256
        || manifest.source_revision != artifact.source_revision
        || manifest.key_id != artifact.key_id
        || manifest.binary != policy.binary
        || manifest.launcher != policy.launcher
        || manifest.config_schema != policy.config_schema
        || manifest.state_schema != policy.state_schema
    {
        return Err("release manifest does not match registry desired state".to_string());
    }
    if manifest.qualification.status != QualificationStatus::Passed {
        return Err("release candidate has not passed qualification".to_string());
    }
    if crate::binary::release::version_newer(
        env!("CARGO_PKG_VERSION"),
        &manifest.minimum_stado_version,
    ) {
        return Err(format!(
            "release requires Stado {}, host runs {}",
            manifest.minimum_stado_version,
            env!("CARGO_PKG_VERSION")
        ));
    }
    let signature = fetch_release_bytes(&artifact.signature_uri).await?;
    let signature = std::str::from_utf8(&signature)
        .map_err(|_| "release signature is not UTF-8".to_string())?;
    let public_key = control
        .trusted_keys
        .get(&artifact.key_id)
        .ok_or_else(|| "release signing key is not trusted by registry".to_string())?;
    release_control::verify_manifest(public_key, &manifest, signature)?;
    let archive = fetch_release_bytes(&artifact.archive_uri).await?;
    if archive.len() as u64 != manifest.artifact_bytes
        || release_control::sha256_bytes(&archive) != manifest.artifact_sha256
    {
        return Err("release archive does not match its signed manifest".to_string());
    }
    let directory = release_control::install_directory(policy, target, &manifest);
    Ok((manifest, archive, directory))
}
