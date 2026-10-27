use serde_json::Value;

use crate::queue::VersionedText;
use crate::targets;

use crate::cli::registry;
use crate::cli::CmdError;

use crate::cli::resolver::directory::source::{current_target, snapshot_source};

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

/// The canonical registry document as the service directory's registry
/// authority serves it over SSH, with the store version it was read under:
/// the same `stado resolver snapshot` read the resolver itself keeps its
/// directory with, located through this host's last-known-good copy.
///
/// A reader whose store is another host's object API reads the registry
/// through that API's authorization, and the API authorizes each object
/// request against the vault. A vault that stops answering therefore left
/// every registry read, and every command that resolves anything through
/// one — a host command, `database resolve`, `service directory connect`,
/// `registry self` — waiting with no answer, including the declared repairs
/// of that vault. The authority's snapshot is the current document, read on
/// the authority from its own store over native SSH, so it does not depend
/// on the vault. [`crate::targets::RegistryStore`] reads it before its store
/// whenever that store is an object API, so every registry read of this
/// program goes this way through one place.
///
/// `Ok(None)` when this host keeps no copy, the copy declares no service
/// directory, or this host is the authority: the caller reads its store. A
/// copy that cannot be read is the error, which the caller says before it
/// reads its store; taken for no copy, a vault-dependent store read would
/// stand in for the authority without a word.
pub(crate) async fn authority_snapshot() -> Result<Option<VersionedText>, CmdError> {
    let Some((copy, local_target)) = remote_authority()? else {
        return Ok(None);
    };
    let source = snapshot_source(None, &copy, &local_target).map_err(CmdError::declaration)?;
    let (document, version, _) = source
        .fetch(crate::monitor::host_silence::READER_CLI)
        .await?;
    Ok(Some(VersionedText {
        content: document.to_string(),
        version,
    }))
}

/// This host's last-known-good copy and this host's target, when the copy
/// names a registry authority that is another host; `None` when there is no
/// copy, the copy declares no service directory, or this host is the
/// authority, whose readers read its store.
fn remote_authority() -> Result<Option<(Value, String)>, CmdError> {
    let Some(copy) = kept_last_good_document().map_err(CmdError::declaration)? else {
        return Ok(None);
    };
    let Some(directory) =
        crate::service_resolution::directory(&copy).map_err(CmdError::declaration)?
    else {
        return Ok(None);
    };
    let local_target = current_target(&copy).map_err(CmdError::declaration)?;
    if directory.authority.target == local_target {
        return Ok(None);
    }
    Ok(Some((copy, local_target)))
}

/// The registry a reader on this host takes when the authority is another
/// host: the generation this host's own resolver holds, which it refreshes
/// from the authority on its declared interval and stops answering past its
/// declared max-stale; else the authority's snapshot this command already
/// read less than one refresh interval ago
/// ([`super::remembered::recalled`]); and the authority's snapshot only when
/// neither answers, kept for the next read of the same command. A host whose
/// authority is slow or down still reads its registry at once, one command
/// stands one authority read however often it reads the registry, and
/// stderr names which of them answered. On the authority itself `None`
/// sends the reader to its store, which is what that host's resolver reads,
/// never to the resolver's own held generation. The checks whether the
/// authority itself answers (`placement move --authority`, `registry push
/// --last-known-good`) keep [`authority_snapshot`].
pub(crate) async fn held_or_authority_snapshot() -> Result<Option<VersionedText>, CmdError> {
    let Some((copy, _)) = remote_authority()? else {
        return Ok(None);
    };
    match super::held_snapshot().await {
        Ok(((document, version, generation), read_from)) => {
            let refreshed = match read_from {
                Some(authority) => {
                    format!("which refreshes it from the registry authority {authority}")
                }
                None => "which holds it as the registry authority".to_string(),
            };
            eprintln!(
                "stado: registry generation {generation} read from this host's resolver, \
                 {refreshed}; the authority was not asked"
            );
            return Ok(Some(VersionedText {
                content: document.to_string(),
                version,
            }));
        }
        Err(refusal) => eprintln!(
            "stado: this host's resolver holds no registry generation to answer from \
             ({refusal}); asking the registry authority"
        ),
    }
    let authority = crate::service_resolution::directory(&copy)
        .map_err(CmdError::declaration)?
        .map(|directory| directory.authority.target);
    if let Some(recalled) = authority
        .as_deref()
        .and_then(super::remembered::recalled::recall)
    {
        return Ok(Some(recalled));
    }
    let snapshot = authority_snapshot().await?;
    if let (Some(authority), Some(snapshot)) = (authority.as_deref(), snapshot.as_ref()) {
        super::remembered::recalled::keep(authority, snapshot);
    }
    Ok(snapshot)
}

/// The registry document a command resolves services from, given the copy
/// it read from `store` (`bootstrap`) and this host's target: the generation
/// this host's resolver holds, else the snapshot kept less than one refresh
/// interval ago ([`held_or_authority_snapshot`]); the authority, or this
/// host's own store when it is the authority, only when neither answers.
/// `resolver resolve` and the database plane each asked the authority again
/// after the store read had often asked it already, so one command stood two
/// authority reads.
pub(crate) async fn directory_document(
    store: std::sync::Arc<targets::RegistryStore>,
    bootstrap: &Value,
    local_target: &str,
) -> Result<Value, CmdError> {
    if let Some(snapshot) = held_or_authority_snapshot().await? {
        return serde_json::from_str(&snapshot.content).map_err(|error| {
            CmdError::declaration(format!("the registry this host read is not JSON: {error}"))
        });
    }
    let source =
        snapshot_source(Some(store), bootstrap, local_target).map_err(CmdError::declaration)?;
    let (document, _, _) = source
        .fetch(crate::monitor::host_silence::READER_CLI)
        .await?;
    Ok(document)
}

/// One attempt at everything the resolver must read before it can bind.
///
pub(crate) fn last_good_document() -> Result<Value, String> {
    match kept_last_good_document()? {
        Some(document) => Ok(document),
        None => Err(match targets::registry_last_good_path() {
            Some(path) => format!(
                "this host keeps no last-known-good registry at {}",
                path.display()
            ),
            None => "last-known-good registry path is unavailable".to_string(),
        }),
    }
}

/// This host's last-known-good registry copy; `None` when it keeps none. A
/// copy that exists and cannot be read or parsed is the error naming it.
pub(crate) fn kept_last_good_document() -> Result<Option<Value>, String> {
    let path = targets::registry_last_good_path()
        .ok_or_else(|| "last-known-good registry path is unavailable".to_string())?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("{} is not valid registry JSON: {error}", path.display()))
}
