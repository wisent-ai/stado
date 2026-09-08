//! The source identity that authorizes delivery for one coordinate.

use crate::cli::storage::*;

/// Source identity that authorizes delivery for one release coordinate.
///
/// New releases are bound by the platformless version claim. Coordinates
/// published before that contract retain a validated platform-claim fallback.
pub(crate) async fn release_claim_source(
    product: &str,
    version: &str,
    platform: &str,
) -> Result<String, CmdError> {
    let version_base =
        crate::release_control::release_version_base(product, version).map_err(CmdError::click)?;
    let version_uri = format!(
        "{version_base}/{}",
        crate::release_control::RELEASE_VERSION_REVISION_NAME
    );
    if release_object_present(&version_uri).await? {
        let bytes = fetch_object(&version_uri).await?;
        let claim: crate::release_control::VersionRevision = serde_json::from_slice(&bytes)
            .map_err(|error| {
                CmdError::click(format!(
                    "{version_uri} is not a valid version claim: {error}"
                ))
            })?;
        if !claim.describes(product, version) {
            return Err(CmdError::click(format!(
                "{version_uri} does not describe {product}/{version}"
            )));
        }
        return Ok(claim.source_revision);
    }

    let platform_base = crate::release_control::release_base(product, version, platform)
        .map_err(CmdError::click)?;
    let platform_uri = format!(
        "{platform_base}/{}",
        crate::release_control::RELEASE_REVISION_NAME
    );
    let bytes = fetch_object(&platform_uri).await?;
    let claim: crate::release_control::CoordinateRevision = serde_json::from_slice(&bytes)
        .map_err(|error| {
            CmdError::click(format!(
                "{platform_uri} is not a valid platform claim: {error}"
            ))
        })?;
    if !claim.describes(product, version, platform) {
        return Err(CmdError::click(format!(
            "{platform_uri} does not describe {product}/{version}/{platform}"
        )));
    }
    Ok(claim.source_revision)
}
