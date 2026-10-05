use serde_json::Value;

use super::super::CatalogIdentity;
use super::conflict::revision_conflict;
use crate::deploy::products::Product;
use crate::deploy::DeployError;

pub(super) async fn pipeline_catalog_identity(
    product: &Product,
    version: &str,
    platform: &str,
) -> Result<CatalogIdentity, DeployError> {
    let base = format!(
        "stado://releases/{}/{version}/{platform}",
        product.source.product
    );
    let manifest_uri = format!("{base}/{}", crate::release_control::RELEASE_MANIFEST_NAME);
    let bytes = crate::cli::storage::fetch_object(&manifest_uri)
        .await
        .map_err(|error| {
            DeployError(format!(
                "the signed release manifest {manifest_uri} is unavailable: {error}"
            ))
        })?;
    let manifest: crate::release_control::ReleaseManifest = serde_json::from_slice(&bytes)
        .map_err(|error| {
            DeployError(format!(
                "canonical pipeline release manifest is invalid: {error}"
            ))
        })?;
    crate::release_control::validate_manifest(&manifest).map_err(DeployError)?;
    for (field, found, expected) in [
        (
            "product",
            manifest.product.as_str(),
            product.source.product.as_str(),
        ),
        ("version", manifest.version.as_str(), version),
        ("platform", manifest.platform.as_str(), platform),
    ] {
        if found != expected {
            return Err(DeployError(format!(
                "canonical pipeline release manifest {field} is {found:?}, expected {expected:?}"
            )));
        }
    }
    let claimed_source =
        crate::cli::storage::release_claim_source(&product.source.product, version, platform)
            .await
            .map_err(|error| DeployError(error.to_string()))?;
    if claimed_source != manifest.source_revision {
        return Err(DeployError(format!(
            "canonical pipeline release source {} disagrees with authoritative claim {}",
            manifest.source_revision, claimed_source
        )));
    }
    // One immutable coordinate, one build. The signed document above is the
    // delivery authority and stays it; what was missing is any check that the
    // OTHER publisher of the same coordinate agrees with it.
    //
    // Both can write the same coordinate: a `release submit` publishes the
    // signed `release.json` and `release.tar.gz` from one commit, and the
    // tag's own train publishes the platform objects from a later one.
    // Create-only puts mean neither can overwrite the other, so the
    // coordinate holds two builds, and preferring the signed one without
    // reading the sidecar beside it makes release delivery report the version
    // as installed while installing the older build, with host-state
    // confirming `in-sync` — every reading true about itself and none of them
    // about the version that was requested.
    //
    // A version number that means two different builds is not deliverable, and
    // the doctrine for that is already written in `catalog_identity` below:
    // immutable objects mean the coordinate can never be repaired, so publish a
    // new version. This refuses with both revisions named rather than choosing
    // one of them quietly.
    let sidecar_uri = format!("{base}/release-manifest-{platform}.json");
    if let Ok(sidecar) = crate::cli::storage::fetch_object(&sidecar_uri).await {
        let sidecar: Value = serde_json::from_slice(&sidecar).map_err(|error| {
            DeployError(format!(
                "{sidecar_uri} sits beside a signed release manifest and is invalid: {error}"
            ))
        })?;
        if let Some(conflict) = revision_conflict(
            &product.source.product,
            version,
            platform,
            &manifest.source_revision,
            &sidecar_uri,
            &sidecar,
        ) {
            return Err(DeployError(conflict));
        }
    }
    let (document, _) = crate::cli::registry::fetch_versioned_document()
        .await
        .map_err(|error| DeployError(error.to_string()))?;
    let control = crate::release_control::control(&document)
        .map_err(DeployError)?
        .ok_or_else(|| DeployError("registry declares no release trust keys".to_string()))?;
    let public_key = control.trusted_keys.get(&manifest.key_id).ok_or_else(|| {
        DeployError(format!(
            "canonical pipeline release uses untrusted key {}",
            manifest.key_id
        ))
    })?;
    let signature_uri = format!("{base}/{}", crate::release_control::RELEASE_SIGNATURE_NAME);
    let signature = crate::cli::storage::fetch_object(&signature_uri)
        .await
        .map_err(|error| {
            DeployError(format!(
                "canonical pipeline release signature is unavailable at {signature_uri}: {error}"
            ))
        })?;
    let signature = String::from_utf8(signature).map_err(|_| {
        DeployError("canonical pipeline release signature is not UTF-8".to_string())
    })?;
    crate::release_control::verify_manifest(public_key, &manifest, &signature)
        .map_err(DeployError)?;
    let archive_name = crate::release_control::RELEASE_ARCHIVE_NAME.to_string();
    let mut missing = Vec::new();
    for name in [
        archive_name.as_str(),
        crate::release_control::RELEASE_QUALIFICATION_NAME,
    ] {
        let uri = format!("{base}/{name}");
        if !crate::cli::storage::release_object_present(&uri)
            .await
            .map_err(|error| DeployError(error.to_string()))?
        {
            missing.push(name.to_string());
        }
    }
    if !missing.is_empty() {
        return Err(DeployError(format!(
            "{} {version} is incomplete on {platform}: missing {}",
            product.source.product,
            missing.join(", ")
        )));
    }
    Ok(CatalogIdentity {
        source_commit: manifest.source_revision,
        sha256: manifest.artifact_sha256,
        archive_name,
        member: if manifest.binary.is_empty() {
            product.source.member.clone()
        } else {
            manifest.binary
        },
    })
}
