//! The signing identity a product's release is published under, and the one
//! qualification the registry can refuse before the first build.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use crate::cli::registry;
use crate::cli::CmdError;
use crate::release_control;
use crate::release_pipeline::{ReleasePipelineManifest, PRODUCT_MANIFEST};

pub(crate) async fn signing(product: &str) -> Result<(String, Vec<u8>), CmdError> {
    let (document, _) = registry::fetch_versioned_document().await?;
    let control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    let policy = control.products.get(product);
    let item = policy
        .map(|value| value.signing_key_item.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(crate::config::release_signing_key_item);
    let key_id = policy
        .map(|value| value.signing_key_id.clone())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(crate::config::release_signing_key_id);
    let encoded = crate::skarbiec::read_release_signing_key(&item)
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "cannot read signing key {item:?} as {}: {error}",
                crate::config::release_signing_skarbiec_consumer()
            ))
        })?
        .ok_or_else(|| {
            CmdError::click(format!(
                "Skarbiec item {item:?} field private_key is required"
            ))
        })?;
    let private = BASE64
        .decode(encoded)
        .map_err(|_| CmdError::click("release signing key is not base64"))?;
    let public =
        BASE64.encode(release_control::signing_public_key(&private).map_err(CmdError::click)?);
    if control.trusted_keys.get(&key_id) != Some(&public) {
        return Err(CmdError::click(format!(
            "release key {key_id:?} is not trusted by registry"
        )));
    }
    Ok((key_id, private))
}

/// Refuse a release whose runtime declaration omits the version it would
/// replace, before anything is built, signed or published.
///
/// The host enforces the same rule at rollout, and enforcing it only there is
/// expensive: `release_agent` quarantines the candidate digest as
/// `rollback_compatibility_undeclared`, and a quarantined immutable coordinate
/// is spent -- the version can be abandoned but never retried, because a
/// rebuild of the same version writes different bytes to a coordinate that
/// refuses to differ. Brama burnt 0.2.40, 0.2.44, 0.2.54 and 0.2.59 exactly
/// that way, each time because a hand-kept list had not been told about the
/// release that shipped before it. Both sides of the comparison are readable
/// here, one document read before the first build, so the answer arrives while
/// it is still free and names the edit that fixes it.
pub(crate) async fn require_rollback_compatibility(
    manifest: &ReleasePipelineManifest,
    version: &str,
) -> Result<(), CmdError> {
    let Some(runtime) = manifest.runtime.as_ref() else {
        return Ok(());
    };
    let (document, _) = registry::fetch_versioned_document().await?;
    let Some(control) = release_control::control(&document)? else {
        return Ok(());
    };
    let Some(policy) = control.products.get(&manifest.product) else {
        return Ok(());
    };
    let Some(desired) = policy.desired.as_ref() else {
        return Ok(());
    };
    if desired.version == version
        || runtime
            .rollback_compatible_with
            .iter()
            .any(|declared| declared == &desired.version)
    {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{} {version} does not declare rollback compatibility with {}, the release it would \
         replace; add \"{}\" to runtime.rollback_compatible_with in {PRODUCT_MANIFEST}. Without \
         it every rollout target quarantines this digest and the coordinate is spent.",
        manifest.product, desired.version, desired.version
    )))
}
