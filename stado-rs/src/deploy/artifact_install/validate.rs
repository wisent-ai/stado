//! What must be refused before anything reaches a host: the two values that
//! become path segments inside a remote script, and the manifest location a
//! consumer is meant to read.

use crate::artifacts_models::{ArtifactManifest, ArtifactRef};
use crate::deploy::DeployError;

/// A service name that is safe as a path segment. Deliberately stricter than
/// the unit-name rule: this value is interpolated into a remote shell script,
/// and a name that needs quoting to be safe is a name that should be refused.
pub(super) fn validate_service_name(name: &str) -> Result<(), DeployError> {
    let safe = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.');
    if safe {
        Ok(())
    } else {
        Err(DeployError(format!(
            "service name {name:?} must be lowercase letters, digits, '.', '-' or '_'"
        )))
    }
}

/// The manifest's primary location, which is the copy a consumer is meant to
/// read. A manifest without one is a publication bug rather than a transfer
/// failure, so it is reported as such.
pub(super) fn primary_location(
    manifest: &ArtifactManifest,
) -> Result<&crate::artifacts_models::ArtifactLocation, DeployError> {
    manifest
        .locations
        .iter()
        .find(|location| location.role == "primary")
        .ok_or_else(|| {
            DeployError(format!(
                "artifact {} declares no primary location",
                manifest.ref_
            ))
        })
}

/// The version segment a materialised artifact is stored under.
///
/// Taken from the resolved reference rather than from an alias, so the path on
/// disk names the immutable version even when the operator deployed
/// `service@stable`. That is the whole point: `current` may move, the version
/// directory beside it may not.
pub(super) fn version_segment(reference: &ArtifactRef) -> Result<String, DeployError> {
    let version = reference.version.trim();
    let safe = !version.is_empty()
        && version.len() <= 128
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !version.starts_with('.');
    if safe {
        Ok(version.to_string())
    } else {
        Err(DeployError(format!(
            "artifact version {version:?} is not usable as a path segment"
        )))
    }
}
