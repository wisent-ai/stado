//! Reading one published coordinate back: which objects a verification
//! touches, and what it proves before a release may be promoted.

use crate::cli::CmdError;
use crate::release_control::{
    self, QualificationStatus, ReleaseArtifactRef, ReleaseControl, ReleaseManifest,
};

/// One verified release reference: manifest identity, qualification,
/// signature, and an archive that is actually published.
pub(crate) async fn verified_artifact(
    product: &str,
    version: &str,
    platform: &str,
    control: &ReleaseControl,
) -> Result<ReleaseArtifactRef, CmdError> {
    let base =
        release_control::release_base(product, version, platform).map_err(CmdError::click)?;
    let manifest_uri = format!("{base}/{}", release_control::RELEASE_MANIFEST_NAME);
    let signature_uri = format!("{base}/{}", release_control::RELEASE_SIGNATURE_NAME);
    let archive_uri = format!("{base}/{}", release_control::RELEASE_ARCHIVE_NAME);
    let manifest_bytes = crate::cli::storage::fetch_object(&manifest_uri).await?;
    let manifest: ReleaseManifest = serde_json::from_slice(&manifest_bytes)?;
    release_control::validate_manifest(&manifest).map_err(CmdError::click)?;
    if manifest.product != product || manifest.version != version || manifest.platform != platform {
        return Err(CmdError::click(
            "release manifest identity does not match its object coordinate",
        ));
    }
    let claimed_source =
        crate::cli::storage::release_claim_source(product, version, platform).await?;
    if claimed_source != manifest.source_revision {
        return Err(CmdError::click(format!(
            "release manifest source revision {} disagrees with authoritative claim {}",
            manifest.source_revision, claimed_source
        )));
    }
    if manifest.qualification.status != QualificationStatus::Passed {
        return Err(CmdError::click(format!(
            "release {product} {version} {platform} has not passed qualification"
        )));
    }
    let public = control.trusted_keys.get(&manifest.key_id).ok_or_else(|| {
        CmdError::click(format!(
            "release key {} is not trusted by registry",
            manifest.key_id
        ))
    })?;
    let signature = crate::cli::storage::fetch_object(&signature_uri).await?;
    release_control::verify_manifest(
        public,
        &manifest,
        std::str::from_utf8(&signature)
            .map_err(|_| CmdError::click("release signature is not UTF-8"))?,
    )
    .map_err(CmdError::click)?;
    // Presence, not bytes. `release_object_present` propagates an unanswered
    // store as an error rather than as `false`, so a blip is never read as a
    // half-published release.
    if !crate::cli::storage::release_object_present(&archive_uri).await? {
        return Err(CmdError::click(format!(
            "{archive_uri} is not published, so its signed manifest describes an archive no host \
             could fetch"
        )));
    }
    Ok(ReleaseArtifactRef {
        manifest_uri,
        signature_uri,
        archive_uri,
        manifest_sha256: release_control::sha256_bytes(&manifest_bytes),
        artifact_sha256: manifest.artifact_sha256,
        source_revision: manifest.source_revision,
        key_id: manifest.key_id,
    })
}

pub(crate) async fn verified_artifact_for_submit(
    product: &str,
    version: &str,
    platform: &str,
) -> Result<ReleaseArtifactRef, CmdError> {
    let (document, _) = crate::cli::registry::fetch_versioned_document().await?;
    let control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    verified_artifact(product, version, platform, &control).await
}
