//! The reclamation: what is done to one version directory nothing pins any
//! more.
//!
//! [`pass`] is the cleaner entry point that decides which versions reach
//! here, spends the pass budget and writes the report. The two helpers below
//! are the removal itself: the source reservation is read to prove which
//! publication the directory holds, and only the payloads beside it go.

pub(super) mod pass;

use std::path::Path;

use crate::providers::local::space_reclaim::JanitorError;

/// The source revision a version directory claims, when it carries one that
/// describes this product and version. `None` is skipped by the pass as
/// `source_identity_unverified`; which of the three readings failed is
/// logged here, since the pass counts skips and does not carry their text.
fn source_revision(
    version_path: &Path,
    product: &str,
    version: &str,
) -> Option<crate::release_control::VersionRevision> {
    let claim_path = version_path.join(crate::release_control::RELEASE_VERSION_REVISION_NAME);
    let bytes = match std::fs::read(&claim_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(
                path = %claim_path.display(),
                %error,
                "release store: the version's source revision cannot be read; it is kept"
            );
            return None;
        }
    };
    let claim: crate::release_control::VersionRevision = match serde_json::from_slice(&bytes) {
        Ok(claim) => claim,
        Err(error) => {
            tracing::warn!(
                path = %claim_path.display(),
                %error,
                "release store: the version's source revision does not parse; it is kept"
            );
            return None;
        }
    };
    if !claim.describes(product, version) {
        tracing::warn!(
            path = %claim_path.display(),
            product,
            version,
            "release store: the version's source revision describes another product or version; it is kept"
        );
        return None;
    }
    Some(claim)
}

/// Payloads may be reclaimed, but the source reservation must survive.
fn remove_release_payloads(version_path: &Path) -> Result<(), JanitorError> {
    for entry in std::fs::read_dir(version_path)? {
        let entry = entry?;
        if entry.file_name() == crate::release_control::RELEASE_VERSION_REVISION_NAME {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            for member in std::fs::read_dir(&path)? {
                let member = member?;
                if member.file_name() == crate::release_control::RELEASE_REVISION_NAME {
                    continue;
                }
                if member.file_type()?.is_dir() {
                    std::fs::remove_dir_all(member.path())?;
                } else {
                    std::fs::remove_file(member.path())?;
                }
            }
        } else {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}
