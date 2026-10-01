use serde_json::Value;

use crate::targets;

use crate::cli::registry;
use crate::cli::CmdError;

use crate::cli::resolver::directory::source::current_target;

/// Fetch the canonical registry document directly from the configured Stado
/// registry store. Release agents never depend on an SSH hop through the
/// service-directory authority.
pub async fn canonical_document(local_target: &str) -> Result<Value, CmdError> {
    let (document, _) = registry::fetch_versioned_document().await?;
    verify_document_target(document, local_target)
}

/// [`canonical_document`], falling back to this host's last-known-good copy
/// when the authority cannot be read.
///
/// The release agent publishes every product's stable bind, and it learns
/// which ports those are from `release_control` in the canonical registry,
/// which it reads through the object API. When the object API's object
/// boundary closes — a namespace declared on the host without the object
/// verifier's grant covering its item — every registry read answers `503
/// object authorization unavailable`. An agent with no fallback exits on
/// that 503 every tick, publishes nothing, and the stable binds of the
/// vault and the gateway stay unbound: the gateway's public health answers
/// 502 for hours, and the object API needs the vault's stable bind to open
/// the very boundary that was closed. A restart of the agent is all it
/// takes to enter that loop and nothing in the product can leave it.
///
/// The resolver already had exactly this fallback and exactly this outage
/// could not touch it — see [`load_startup`], which bootstraps routing from
/// [`last_good_document`] and keeps retrying the authority. This gives the
/// agent the same footing: a start that cannot read the authority uses the
/// last-known-good copy, says so, and publishes the ports the fleet needs
/// while the authority is repaired. It never becomes the steady state, because
/// each tick asks the authority first.
pub async fn canonical_document_or_last_good(local_target: &str) -> Result<Value, CmdError> {
    match registry::fetch_versioned_document().await {
        Ok((document, _)) => verify_document_target(document, local_target),
        Err(authority_error) => {
            let document = last_good_document().map_err(|cache_error| {
                CmdError::click(format!(
                    "registry authority failed ({authority_error}); recovery registry failed ({cache_error})"
                ))
            })?;
            eprintln!(
                "release agent recovery: registry authority failed ({authority_error}); reconciling from the last-known-good registry"
            );
            verify_document_target(document, local_target)
        }
    }
}

/// One document, refused unless it describes the host asking for it.
fn verify_document_target(document: Value, local_target: &str) -> Result<Value, CmdError> {
    let detected = current_target(&document).map_err(CmdError::click)?;
    if detected != local_target {
        return Err(CmdError::click(format!(
            "release agent target {local_target:?} does not match this host {detected:?}"
        )));
    }
    Ok(document)
}

/// One attempt at everything the resolver must read before it can bind.
///
pub(crate) fn last_good_document() -> Result<Value, String> {
    let path = targets::registry_last_good_path()
        .ok_or_else(|| "last-known-good registry path is unavailable".to_string())?;
    let bytes =
        std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is not valid registry JSON: {error}", path.display()))
}
