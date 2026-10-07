use super::super::CatalogIdentity;
use crate::deploy::products::Product;
use crate::deploy::DeployError;
use crate::primitives::failure::FailureCode;

/// A read of the release channel that failed: the sentence says which object,
/// the class is the store's own.
fn unavailable(error: crate::cli::CmdError, what: String) -> DeployError {
    DeployError {
        message: format!("{what}: {error}"),
        failure: error.failure,
    }
}

/// The delivered identity of one product coordinate, read from its signed
/// release.
pub(crate) async fn catalog_identity(
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
            unavailable(
                error,
                format!("the signed release manifest {manifest_uri} is unavailable"),
            )
        })?;
    // The channel served bytes that are not the release: refused, never
    // installed.
    let manifest: crate::release_control::ReleaseManifest = serde_json::from_slice(&bytes)
        .map_err(|error| {
            DeployError(format!(
                "canonical pipeline release manifest is invalid: {error}"
            ))
            .stating(FailureCode::Refused)
        })?;
    crate::release_control::validate_manifest(&manifest)
        .map_err(|error| DeployError(error).stating(FailureCode::Refused))?;
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
            ))
            .stating(FailureCode::Refused));
        }
    }
    let claimed_source =
        crate::cli::storage::release_claim_source(&product.source.product, version, platform)
            .await
            .map_err(DeployError::from)?;
    if claimed_source != manifest.source_revision {
        return Err(DeployError(format!(
            "canonical pipeline release source {} disagrees with authoritative claim {}",
            manifest.source_revision, claimed_source
        ))
        .stating(FailureCode::Refused));
    }
    let (document, _) = crate::cli::registry::fetch_versioned_document()
        .await
        .map_err(DeployError::from)?;
    let control = crate::release_control::control(&document)
        .map_err(|error| DeployError(error).stating(FailureCode::Config))?
        .ok_or_else(|| {
            DeployError("registry declares no release trust keys".to_string())
                .stating(FailureCode::Config)
        })?;
    let public_key = control.trusted_keys.get(&manifest.key_id).ok_or_else(|| {
        DeployError(format!(
            "canonical pipeline release uses untrusted key {}",
            manifest.key_id
        ))
        .stating(FailureCode::Refused)
    })?;
    let signature_uri = format!("{base}/{}", crate::release_control::RELEASE_SIGNATURE_NAME);
    let signature = crate::cli::storage::fetch_object(&signature_uri)
        .await
        .map_err(|error| {
            unavailable(
                error,
                format!("canonical pipeline release signature is unavailable at {signature_uri}"),
            )
        })?;
    let signature = String::from_utf8(signature).map_err(|_| {
        DeployError("canonical pipeline release signature is not UTF-8".to_string())
            .stating(FailureCode::Refused)
    })?;
    crate::release_control::verify_manifest(public_key, &manifest, &signature)
        .map_err(|error| DeployError(error).stating(FailureCode::Refused))?;
    let archive_name = crate::release_control::RELEASE_ARCHIVE_NAME.to_string();
    let mut missing = Vec::new();
    for name in [
        archive_name.as_str(),
        crate::release_control::RELEASE_QUALIFICATION_NAME,
    ] {
        let uri = format!("{base}/{name}");
        if !crate::cli::storage::release_object_present(&uri)
            .await
            .map_err(DeployError::from)?
        {
            missing.push(name.to_string());
        }
    }
    if !missing.is_empty() {
        return Err(DeployError(format!(
            "{} {version} is incomplete on {platform}: missing {}",
            product.source.product,
            missing.join(", ")
        ))
        .stating(FailureCode::NotFound));
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
