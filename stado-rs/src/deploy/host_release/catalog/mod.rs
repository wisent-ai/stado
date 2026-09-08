mod conflict;
mod objects;
mod pipeline;

use serde_json::Value;

use super::{is_sha256, CatalogIdentity};
use crate::deploy::products::Product;
use crate::deploy::DeployError;
use pipeline::pipeline_catalog_identity;

pub(crate) use conflict::coordinate_revision_conflict;
pub(crate) use objects::missing_release_objects;

pub(crate) async fn catalog_identity(
    product: &Product,
    version: &str,
    platform: &str,
) -> Result<CatalogIdentity, DeployError> {
    // The pipeline publisher also emits the old compatibility manifest for
    // older Stado clients, but its signed release archive is the complete
    // coordinate. Prefer it whenever it exists: validating the compatibility
    // surface first incorrectly demands the legacy sidecar binaries (`wc`,
    // `stado-mcp`, and the rest) from an archive-based pipeline release.
    let pipeline_manifest_uri = format!(
        "stado://releases/{}/{version}/{platform}/{}",
        product.source.product,
        crate::release_control::RELEASE_MANIFEST_NAME
    );
    if crate::cli::storage::release_object_present(&pipeline_manifest_uri)
        .await
        .map_err(|error| DeployError(error.to_string()))?
    {
        return pipeline_catalog_identity(
            product,
            version,
            platform,
            "not selected because a signed pipeline manifest exists",
        )
        .await;
    }
    let manifest_uri = format!(
        "stado://releases/{}/{version}/{platform}/release-manifest-{platform}.json",
        product.source.product
    );
    let bytes = match crate::cli::storage::fetch_object(&manifest_uri).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return pipeline_catalog_identity(
                product,
                version,
                platform,
                &format!("{manifest_uri}: {error}"),
            )
            .await;
        }
    };
    let manifest: Value = serde_json::from_slice(&bytes)
        .map_err(|error| DeployError(format!("canonical release manifest is invalid: {error}")))?;
    let object = manifest.as_object().ok_or_else(|| {
        DeployError("canonical release manifest must be a JSON object".to_string())
    })?;
    let expected_fields = ["platform", "product", "sha256", "source_commit", "version"];
    if object.len() != expected_fields.len()
        || expected_fields
            .iter()
            .any(|field| !object.contains_key(*field))
    {
        return Err(DeployError(
            "canonical release manifest must contain exactly product, version, platform, \
             source_commit, and sha256"
                .to_string(),
        ));
    }
    let exact = |field: &str, wanted: &str| -> Result<(), DeployError> {
        let found = manifest
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default();
        if found == wanted {
            Ok(())
        } else {
            Err(DeployError(format!(
                "canonical release manifest {field} is {found:?}, expected {wanted:?}"
            )))
        }
    };
    exact("product", &product.source.product)?;
    exact("version", version)?;
    exact("platform", platform)?;
    let source_commit = manifest
        .get("source_commit")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let sha256 = manifest
        .get("sha256")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !is_sha256(&sha256) {
        return Err(DeployError(format!(
            "canonical release manifest has no valid SHA-256 for {}",
            product.name
        )));
    }
    if !matches!(source_commit.len(), 40 | 64)
        || !source_commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DeployError(
            "canonical release manifest source_commit is invalid".to_string(),
        ));
    }
    let claimed_source =
        crate::cli::storage::release_claim_source(&product.source.product, version, platform)
            .await
            .map_err(|error| DeployError(error.to_string()))?;
    if claimed_source != source_commit {
        return Err(DeployError(format!(
            "canonical release manifest source {source_commit} disagrees with authoritative claim \
             {claimed_source}"
        )));
    }
    let missing = missing_release_objects(product, version, platform).await?;
    if !missing.is_empty() {
        return Err(DeployError(format!(
            "{} {version} is incomplete on {platform} and cannot be delivered: \
             the release declares objects that are not published — {}. \
             Release objects are immutable, so this version can never be \
             completed; publish a new version instead",
            product.source.product,
            missing.join(", ")
        )));
    }
    Ok(CatalogIdentity {
        source_commit,
        sha256,
        archive_name: format!(
            "{}-v{}-{}.tar.gz",
            product.source.product, version, platform
        ),
        member: product.source.member.clone(),
    })
}
