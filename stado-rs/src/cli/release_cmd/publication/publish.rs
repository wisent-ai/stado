//! The create-only object writes that make one release coordinate, and the
//! ordered chain that turns a build into a signed, published release.

use crate::cli::CmdError;
use crate::release_control::{self, ReleaseArtifactRef, ReleaseManifest, ReleaseQualification};

use super::claims::claim_release_coordinate;

/// Write one create-only release object, treating an identical existing object
/// as already written.
///
/// The read-back is not optional. It used to swallow every read failure and
/// proceed to the create-only write, so a store that could not answer was
/// indistinguishable from an empty coordinate: the write then returned
/// `409 object exists` and the release reported `object exists` with no hint
/// that nothing had been compared. Measured on stado 0.16.32 on 2026-09-06,
/// where three resumes each died that way on `release.sig`. Presence is asked
/// through [`crate::cli::storage::release_object_present`], which propagates an
/// unanswered store as an error instead of as absence.
async fn put_immutable(uri: &str, bytes: &[u8], content_type: &str) -> Result<(), CmdError> {
    if crate::cli::storage::release_object_present(uri).await? {
        let existing = crate::cli::storage::fetch_object_from_writer(uri)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "immutable release object {uri} exists and could not be read back: {error}"
                ))
            })?;
        return if existing == bytes {
            Ok(())
        } else {
            Err(CmdError::click(format!(
                "immutable release object already differs: {uri}"
            )))
        };
    }
    let temporary = tempfile::NamedTempFile::new()?;
    std::fs::write(temporary.path(), bytes)?;
    crate::cli::storage::store_object(
        uri,
        &temporary.path().display().to_string(),
        content_type,
        true,
    )
    .await
    .map(|_| ())
}

pub(crate) struct PipelinePublishRequest<'a> {
    pub product: &'a str,
    pub version: &'a str,
    pub platform: &'a str,
    pub archive: &'a [u8],
    pub source_revision: &'a str,
    pub source_sha256: &'a str,
    pub pipeline_manifest_sha256: &'a str,
    pub binary: &'a str,
    pub launcher: &'a str,
    pub config_schema: u64,
    pub state_schema: u64,
    pub minimum_stado_version: &'a str,
    pub rollback_compatible_with: &'a [String],
    pub qualification: ReleaseQualification,
    pub qualification_receipt: &'a [u8],
    pub key_id: &'a str,
    pub private_key: &'a [u8],
    pub builder: &'a str,
}

pub(crate) async fn publish_pipeline_release(
    request: PipelinePublishRequest<'_>,
) -> Result<(ReleaseArtifactRef, ReleaseManifest), CmdError> {
    let artifact_bytes = request.archive.len() as u64;
    let artifact_sha256 = release_control::sha256_bytes(request.archive);
    let qualification_receipt_sha256 = release_control::sha256_bytes(request.qualification_receipt);
    if request.qualification.evidence_sha256.as_deref()
        != Some(qualification_receipt_sha256.as_str())
    {
        return Err(CmdError::click(
            "qualification evidence digest does not match its immutable receipt",
        ));
    }
    // `built_at` is when the BUILD finished, and the build's own receipt says
    // so. It used to be `Utc::now()`, which made the manifest and therefore
    // its signature different bytes on every publication attempt: an
    // interrupted publication that had already written `release.sig` could
    // never be resumed, because the retry signed a different manifest and the
    // create-only channel refused it. The coordinate was spent with no
    // commit marker in it, which is the one state the ordered chain
    // `release.tar.gz -> qualification.json -> release.sig -> release.json`
    // exists to make recoverable. Measured on stado 0.16.32 on 2026-09-06.
    //
    // Deriving it from the qualification receipt keeps the publication a pure
    // function of the build, so every retry writes byte-identical objects and
    // `put_immutable` accepts them.
    let built_at = request
        .qualification
        .completed_at
        .clone()
        .ok_or_else(|| CmdError::click("qualification receipt carries no completion time"))?;
    let manifest = ReleaseManifest {
        schema_version: 1,
        product: request.product.to_string(),
        version: request.version.to_string(),
        platform: request.platform.to_string(),
        source_revision: request.source_revision.to_string(),
        source_sha256: request.source_sha256.to_string(),
        pipeline_manifest_sha256: request.pipeline_manifest_sha256.to_string(),
        qualification_receipt_sha256,
        artifact_sha256,
        artifact_bytes,
        binary: request.binary.to_string(),
        launcher: request.launcher.to_string(),
        config_schema: request.config_schema,
        state_schema: request.state_schema,
        minimum_stado_version: request.minimum_stado_version.to_string(),
        rollback_compatible_with: request.rollback_compatible_with.to_vec(),
        qualification: request.qualification,
        key_id: request.key_id.to_string(),
        built_at,
        builder: request.builder.to_string(),
    };
    release_control::validate_manifest(&manifest).map_err(CmdError::click)?;
    let manifest_bytes = release_control::canonical_manifest(&manifest).map_err(CmdError::click)?;
    let signature =
        release_control::sign_manifest(request.private_key, &manifest).map_err(CmdError::click)?;
    let base = release_control::release_base(request.product, request.version, request.platform)
        .map_err(CmdError::click)?;
    let archive_uri = format!("{base}/{}", release_control::RELEASE_ARCHIVE_NAME);
    let qualification_uri = format!("{base}/{}", release_control::RELEASE_QUALIFICATION_NAME);
    let signature_uri = format!("{base}/{}", release_control::RELEASE_SIGNATURE_NAME);
    let manifest_uri = format!("{base}/{}", release_control::RELEASE_MANIFEST_NAME);
    // The coordinate's identity first, then its bytes. Publishing an archive
    // into a coordinate another build already owns is the failure this order
    // removes: the claim is refused while the prefix is still empty of
    // artifacts, instead of being detected at delivery once both producers
    // have written and the version is spent.
    claim_release_coordinate(
        request.product,
        request.version,
        request.platform,
        request.source_revision,
    )
    .await?;
    put_immutable(&archive_uri, request.archive, "application/gzip").await?;
    put_immutable(
        &qualification_uri,
        request.qualification_receipt,
        "application/json",
    )
    .await?;
    put_immutable(
        &signature_uri,
        format!("{signature}\n").as_bytes(),
        "text/plain",
    )
    .await?;
    // The signed manifest is the immutable commit marker and always lands last.
    put_immutable(&manifest_uri, &manifest_bytes, "application/json").await?;
    Ok((
        ReleaseArtifactRef {
            manifest_uri,
            signature_uri,
            archive_uri,
            manifest_sha256: release_control::sha256_bytes(&manifest_bytes),
            artifact_sha256: manifest.artifact_sha256.clone(),
            source_revision: manifest.source_revision.clone(),
            key_id: manifest.key_id.clone(),
        },
        manifest,
    ))
}
