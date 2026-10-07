//! Fetch one candidate's signed manifest, its signature and its archive, and
//! refuse anything that is not the release the registry asked for.

use std::path::PathBuf;

use crate::release_control::{
    self, DesiredRelease, ProductReleasePolicy, QualificationStatus, ReleaseArtifactRef,
    ReleaseControl, ReleaseManifest, ReleaseTargetPolicy,
};

/// Why a candidate was not fetched. Only bytes that were read and refused say
/// anything about the release; a channel that did not serve them says nothing,
/// so quarantining the digest for it would hold a good release until a person
/// clears it.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CandidateError {
    /// The release channel did not serve one of the candidate's objects; the
    /// store's class is kept.
    #[error("{0}")]
    Unreachable(crate::cli::CmdError),
    /// What the channel served is not the release the registry asked for:
    /// digest, schema, signature, qualification or Stado version.
    #[error("{0}")]
    Refused(String),
}

impl From<CandidateError> for crate::cli::CmdError {
    fn from(error: CandidateError) -> Self {
        match error {
            CandidateError::Unreachable(error) => error,
            CandidateError::Refused(sentence) => Self::refused(sentence),
        }
    }
}

async fn fetch_release_bytes(uri: &str) -> Result<Vec<u8>, CandidateError> {
    // The release channel is served publicly over the object API.
    // Without STADO_API_URL, JobStorage::read_bytes on the canonical root
    // prefix serves a stale copy and the agent quarantines the release.
    crate::cli::storage::fetch_object(uri)
        .await
        .map_err(CandidateError::Unreachable)
}

pub(crate) async fn fetch_candidate(
    control: &ReleaseControl,
    product: &str,
    desired: &DesiredRelease,
    artifact: &ReleaseArtifactRef,
    policy: &ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
) -> Result<(ReleaseManifest, Vec<u8>, PathBuf), CandidateError> {
    let refused = |sentence: &str| CandidateError::Refused(sentence.to_string());
    let manifest_bytes = fetch_release_bytes(&artifact.manifest_uri).await?;
    if release_control::sha256_bytes(&manifest_bytes) != artifact.manifest_sha256 {
        return Err(refused(
            "release manifest digest does not match desired state",
        ));
    }
    let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| CandidateError::Refused(format!("invalid release manifest: {error}")))?;
    release_control::validate_manifest(&manifest).map_err(CandidateError::Refused)?;
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
        return Err(refused(
            "release manifest does not match registry desired state",
        ));
    }
    if manifest.qualification.status != QualificationStatus::Passed {
        return Err(refused("release candidate has not passed qualification"));
    }
    if crate::binary::release::version_newer(
        env!("CARGO_PKG_VERSION"),
        &manifest.minimum_stado_version,
    ) {
        return Err(CandidateError::Refused(format!(
            "release requires Stado {}, host runs {}",
            manifest.minimum_stado_version,
            env!("CARGO_PKG_VERSION")
        )));
    }
    let signature = fetch_release_bytes(&artifact.signature_uri).await?;
    let signature =
        std::str::from_utf8(&signature).map_err(|_| refused("release signature is not UTF-8"))?;
    let public_key = control
        .trusted_keys
        .get(&artifact.key_id)
        .ok_or_else(|| refused("release signing key is not trusted by registry"))?;
    release_control::verify_manifest(public_key, &manifest, signature)
        .map_err(CandidateError::Refused)?;
    let archive = fetch_release_bytes(&artifact.archive_uri).await?;
    if archive.len() as u64 != manifest.artifact_bytes
        || release_control::sha256_bytes(&archive) != manifest.artifact_sha256
    {
        return Err(refused(
            "release archive does not match its signed manifest",
        ));
    }
    let directory = release_control::install_directory(policy, target, &manifest);
    Ok((manifest, archive, directory))
}
