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
/// Registry authorization or transport failure must not discard the host's
/// existing release-control declaration. Use the last-known-good document,
/// report the authority failure, and still verify that it names this host.
/// Every invocation asks the authority first; cached recovery is not a new
/// source of truth. An unreadable cache retains both failure causes.
pub async fn canonical_document_or_last_good(local_target: &str) -> Result<Value, CmdError> {
    match registry::fetch_versioned_document().await {
        Ok((document, _)) => verify_document_target(document, local_target),
        Err(authority_error) => {
            let document = last_good_document().map_err(|cache_error| {
                CmdError::click(format!(
                    "registry authority failed ({authority_error}); recovery registry failed ({cache_error})"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
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
    let detected = current_target(&document).map_err(CmdError::declaration)?;
    if detected != local_target {
        return Err(CmdError::click(format!(
            "release agent target {local_target:?} does not match this host {detected:?}"
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
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
