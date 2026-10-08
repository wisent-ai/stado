//! Store reads for callers that already carry their own Skarbiec coordinates.

use serde_json::Value;

use crate::skarbiec::{Client, ItemVersion, SkarbiecError, VersionedValue};

use super::{file, selected, Backend};

/// Read one item through the selected store for callers that already carry
/// their own Skarbiec coordinates. Under the skarbiec backend the supplied
/// triple is used exactly as before; under the file backend the item comes
/// from the store file instead.
///
/// The mode travels with the coordinates because this function cannot know it:
/// it is handed someone else's grant file and has no basis to decide whether
/// that file is a one-shot handoff.
pub async fn read_item_with(
    url: &str,
    consumer: &str,
    token_file: &str,
    grant_mode: crate::skarbiec::GrantMode,
    id: &str,
) -> Result<Value, SkarbiecError> {
    match selected()? {
        Backend::Skarbiec { url: store_url } => {
            Client::direct(
                store_url.as_deref().unwrap_or(url),
                consumer,
                token_file,
                grant_mode,
            )?
            .read_item(id)
            .await
        }
        Backend::File { path } => file::file_read_item(&path, id),
    }
}

/// Resolve one optional string field for a caller that carries its own Skarbiec
/// coordinates — the field-level counterpart of [`read_item_with`].
///
/// A worker host holds its own grant and never a control-plane bearer, so a
/// credential a worker legitimately needs cannot be read through the configured
/// (control-plane) consumer there. Asking with the caller's own triple is how a
/// worker reads its own field, and one field is the smaller disclosure than the
/// whole item.
pub async fn read_string_with(
    url: &str,
    consumer: &str,
    token_file: &str,
    grant_mode: crate::skarbiec::GrantMode,
    id: &str,
    field: &str,
) -> Result<Option<String>, SkarbiecError> {
    match selected()? {
        Backend::Skarbiec { url: store_url } => {
            Client::direct(
                store_url.as_deref().unwrap_or(url),
                consumer,
                token_file,
                grant_mode,
            )?
            .read_string(id, field)
            .await
        }
        Backend::File { path } => file::file_read_string(&path, id, field),
    }
}

/// [`read_string_with`] for the item a boundary declaration names: read as
/// named, never selected by role. The file backend already reads by id.
pub async fn read_declared_string_with(
    url: &str,
    consumer: &str,
    token_file: &str,
    grant_mode: crate::skarbiec::GrantMode,
    item: &str,
    field: &str,
) -> Result<Option<String>, SkarbiecError> {
    match selected()? {
        Backend::Skarbiec { url: store_url } => {
            Client::direct(
                store_url.as_deref().unwrap_or(url),
                consumer,
                token_file,
                grant_mode,
            )?
            .read_declared_string(item, field)
            .await
        }
        Backend::File { path } => file::file_read_string(&path, item, field),
    }
}

/// The vault a routed read goes to: the store's own URL when the selected
/// backend declares one, the caller's otherwise — the precedence every read
/// above applies.
fn vault_url<'a>(store_url: &'a Option<String>, url: &'a str) -> &'a str {
    match store_url.as_deref() {
        Some(declared) => declared,
        None => url,
    }
}

/// [`read_declared_string_with`] with the version the value was read under.
/// The file backend keeps no versions: its value comes back with none, so
/// a holder reads it again for every check (the file is local; no vault is
/// asked).
pub async fn read_declared_versioned_with(
    url: &str,
    consumer: &str,
    token_file: &str,
    grant_mode: crate::skarbiec::GrantMode,
    item: &str,
    field: &str,
) -> Result<Option<VersionedValue>, SkarbiecError> {
    match selected()? {
        Backend::Skarbiec { url: store_url } => {
            Client::direct(vault_url(&store_url, url), consumer, token_file, grant_mode)?
                .read_declared_versioned(item, field)
                .await
        }
        Backend::File { path } => Ok(file::file_read_string(&path, item, field)?
            .map(|value| VersionedValue { value, version: None })),
    }
}

/// The version a read of the declared item's field would answer now. The
/// file backend keeps no versions and answers `None`, which a holder reads
/// as "read the value again".
pub async fn read_declared_revision_with(
    url: &str,
    consumer: &str,
    token_file: &str,
    grant_mode: crate::skarbiec::GrantMode,
    item: &str,
    field: &str,
) -> Result<Option<ItemVersion>, SkarbiecError> {
    match selected()? {
        Backend::Skarbiec { url: store_url } => {
            Client::direct(vault_url(&store_url, url), consumer, token_file, grant_mode)?
                .read_declared_revision(item, field)
                .await
        }
        Backend::File { .. } => Ok(None),
    }
}
