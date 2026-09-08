use serde_json::Value;

use crate::deploy::products::Product;
use crate::deploy::DeployError;

/// One immutable coordinate, one build: the sentence to refuse with when the
/// coordinate's two publishers disagree about which revision they built.
///
/// This is its own function because two callers ask the same question at
/// different moments. `pipeline_catalog_identity` asks it while delivering,
/// with the signed manifest already in hand. `doctor`'s release-channel audit
/// asks it about every recent coordinate, so a version poisoned at publication
/// time is named by the standing audit rather than waiting for the next
/// delivery to discover it — which is how 0.13.27 was found on 2026-09-01, and
/// how 0.13.49 was found on 2026-09-03 only after its train had been re-run to
/// completion into a coordinate that can never be delivered.
pub(super) fn revision_conflict(
    product: &str,
    version: &str,
    platform: &str,
    signed_revision: &str,
    sidecar_uri: &str,
    sidecar: &Value,
) -> Option<String> {
    let sidecar_commit = sidecar.get("source_commit").and_then(Value::as_str)?;
    if sidecar_commit.is_empty() || sidecar_commit == signed_revision {
        return None;
    }
    Some(format!(
        "{product} {version} on {platform} was published twice from two different revisions: \
         the signed release manifest was built from {signed_revision}, and {sidecar_uri} from \
         {sidecar_commit}. Delivery would install the signed one, which is not the build the \
         second publisher put there. Release objects are immutable, so this version can never \
         be made to mean one build; publish a new version instead"
    ))
}

/// Read both publishers of one published coordinate and report the conflict
/// between them, delivering nothing.
///
/// A coordinate with one publisher is not a conflict, and neither is an absent
/// sidecar or an absent signed manifest: the only question answered here is
/// whether the two documents that do exist name the same build. An unreadable
/// store is an error rather than agreement, because a listing that cannot be
/// read is not a coordinate that agrees with itself.
///
/// The two reads run together rather than in sequence, because the standing
/// audit in `doctor` calls this once per coordinate under a fixed deadline
/// whose arithmetic is one round trip per coordinate, not two.
pub(crate) async fn coordinate_revision_conflict(
    product: &Product,
    version: &str,
    platform: &str,
) -> Result<Option<String>, DeployError> {
    let base = format!(
        "stado://releases/{}/{version}/{platform}",
        product.source.product
    );
    let sidecar_uri = format!("{base}/release-manifest-{platform}.json");
    let signed_uri = format!("{base}/{}", crate::release_control::RELEASE_MANIFEST_NAME);
    let (sidecar, signed) = tokio::join!(
        crate::cli::storage::fetch_object(&sidecar_uri),
        crate::cli::storage::fetch_object(&signed_uri),
    );
    let (Ok(sidecar), Ok(signed)) = (sidecar, signed) else {
        return Ok(None);
    };
    let signed: crate::release_control::ReleaseManifest = serde_json::from_slice(&signed)
        .map_err(|error| DeployError(format!("{signed_uri} is invalid: {error}")))?;
    let sidecar: Value = serde_json::from_slice(&sidecar).map_err(|error| {
        DeployError(format!(
            "{sidecar_uri} sits beside a signed release manifest and is invalid: {error}"
        ))
    })?;
    Ok(revision_conflict(
        &product.source.product,
        version,
        platform,
        &signed.source_revision,
        &sidecar_uri,
        &sidecar,
    ))
}
