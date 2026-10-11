//! Whether a job's declared secrets can be resolved: by a builder's published
//! `secret_fields`, and by the items the `stado` consumer can see at all.

use std::collections::BTreeMap;

use crate::cli::CmdError;

/// The first `role#field` the job declares that this host's published
/// `secret_fields` does not allow. A publication without the list comes from
/// an agent older than it and is not judged here; that agent's own
/// claim-time probe still declines what it cannot resolve.
pub(super) fn missing_secret_field(
    publication: &serde_json::Value,
    secret_env: &BTreeMap<String, String>,
) -> Option<String> {
    let allowed = publication.get("secret_fields")?.as_array()?;
    secret_env
        .values()
        .find(|reference| {
            !allowed
                .iter()
                .any(|field| field.as_str() == Some(reference.as_str()))
        })
        .map(|reference| format!("{reference} is not in its agent.skarbiec.secret_fields"))
}

/// Refuse a platform job that asks for a role no item the `stado` consumer
/// can see plays, before anything is queued.
///
/// [`missing_secret_field`] judges a builder by the `secret_fields` it
/// declares, and a declaration is not a vault: a job whose role no item
/// plays would sit queued while every builder's claim-time probe declines
/// it. The builder resolves a job's secrets as consumer `stado`, so that
/// consumer's metadata listing — ids and tags, no values — decides whether
/// any host could ever claim the job.
pub(crate) async fn refuse_unheld_secret_items(
    product: &str,
    platform: &str,
    secret_env: &BTreeMap<String, String>,
) -> Result<(), CmdError> {
    if secret_env.is_empty() {
        return Ok(());
    }
    let listing_failed = |error: crate::skarbiec::SkarbiecError| {
        CmdError::click(format!(
            "{product}'s {platform} build asks for vault roles and the stado consumer's item \
             listing could not be read, so whether any builder can claim it is unknown: {error}"
        ))
        .stating(error.failure_code())
    };
    let visible = crate::skarbiec::Client::configured()
        .map_err(listing_failed)?
        .list_items()
        .await
        .map_err(listing_failed)?;
    let unheld: Vec<String> = secret_env
        .iter()
        .filter_map(|(env, reference)| {
            let role = reference
                .split_once('#')
                .map_or(reference.as_str(), |(role, _)| role);
            crate::skarbiec::roles::item_for_role(&visible, role)
                .err()
                .map(|refusal| format!("{env}: {refusal}"))
        })
        .collect();
    if unheld.is_empty() {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "{product}'s {platform} build asks for roles the stado consumer cannot select, so no \
         builder could ever claim it: {}. Store the secret with `stado credentials item put \
         --host <vault owner> --role <ROLE>` and grant it to stado before building.",
        unheld.join("; ")
    )))
}
