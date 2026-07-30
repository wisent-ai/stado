//! Pluggable credential store, selected by `STADO_CREDENTIAL_STORE`.
//!
//! - unset, empty, `skarbiec`, or `skarbiec://<base-url>` — the Skarbiec
//!   client (the default; behavior is byte-identical to calling the client
//!   directly, with the optional URL overriding the configured one);
//! - `file://<path>` or a bare absolute path — a JSON file shaped as a flat
//!   map `{"<item-id>": {"<field>": "<value>", ...}, ...}` held in a regular,
//!   non-symlink file owned by the current user with owner-only mode bits
//!   (the same posture as the Skarbiec grant file);
//! - anything else — a hard error naming the unsupported scheme. No scheme is
//!   ever accepted quietly.
//!
//! This module is the single resolution point for provider/billing/tooling
//! credential READ paths. The Skarbiec client itself is untouched, and the
//! serve-side verifier grants (object/release/machine/service/…) keep using
//! `skarbiec::Client` directly — an auth boundary is never store-switchable.
//! Store-selection failures report through the existing
//! `SkarbiecError::Deployment` variant so callers keep one error type.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde_json::Value;

use crate::skarbiec::{Client, SkarbiecError};

const ENV_STORE: &str = "STADO_CREDENTIAL_STORE";
const CONFIG_KEY: &str = "credential_store";

/// The store is declared in the stado config file (`credential_store` key);
/// `STADO_CREDENTIAL_STORE` exists only as an explicit per-process override.
/// Nothing about a backend may live solely in the environment.
fn declared() -> String {
    if let Ok(raw) = std::env::var(ENV_STORE) {
        let raw = raw.trim();
        if !raw.is_empty() {
            return raw.to_string();
        }
    }
    let config_path = std::env::var("STADO_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|home| PathBuf::from(home).join(".config/stado/config.json"))
                .unwrap_or_default()
        });
    std::fs::read_to_string(config_path)
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .and_then(|doc| doc.get(CONFIG_KEY).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;

#[derive(Debug, PartialEq, Eq)]
enum Backend {
    Skarbiec { url: Option<String> },
    File { path: PathBuf },
}

fn scheme_of(raw: &str) -> String {
    raw.split_once("://")
        .map(|(scheme, _)| scheme)
        .unwrap_or(raw)
        .to_string()
}

fn unsupported(scheme: &str) -> SkarbiecError {
    SkarbiecError::Deployment(format!(
        "unsupported credential store {scheme:?}; set STADO_CREDENTIAL_STORE to skarbiec or file://<path>"
    ))
}

fn selected() -> Result<Backend, SkarbiecError> {
    let raw = declared();
    let raw = raw.trim();
    if raw.is_empty() || raw == "skarbiec" {
        return Ok(Backend::Skarbiec { url: None });
    }
    if let Some(rest) = raw.strip_prefix("skarbiec://") {
        let rest = rest.trim();
        let url = (!rest.is_empty()).then(|| rest.to_string());
        return Ok(Backend::Skarbiec { url });
    }
    if let Some(path) = raw.strip_prefix("file://") {
        return file_backend(path);
    }
    if raw.starts_with('/') {
        return file_backend(raw);
    }
    Err(unsupported(&scheme_of(raw)))
}

fn file_backend(path: &str) -> Result<Backend, SkarbiecError> {
    let path = path.trim();
    if path.is_empty() || !path.starts_with('/') {
        return Err(unsupported("file (an absolute path is required)"));
    }
    Ok(Backend::File {
        path: PathBuf::from(path),
    })
}

/// Effective uid of this process, resolved once via `id` (no numeric literal
/// and no extra crate feature; matches the Skarbiec-side precedent).
#[cfg(unix)]
fn current_uid() -> Result<u32, SkarbiecError> {
    static UID: LazyLock<Result<u32, String>> = LazyLock::new(|| {
        let output = std::process::Command::new("id")
            .arg("-u")
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err("id -u exited non-zero".to_string());
        }
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .map_err(|error| format!("cannot parse id -u output: {error}"))
    });
    UID.as_ref()
        .copied()
        .map_err(|detail| {
            SkarbiecError::Deployment(format!("cannot determine current uid: {detail}"))
        })
}

/// The store file must be a regular, non-symlink file owned by the current
/// user with owner-only mode bits — the checks `skarbiec-vault-publish`'s
/// `checkedOwnerFile` applies to the vault before publishing it.
#[cfg(unix)]
fn checked_owner_file(path: &Path) -> Result<(), SkarbiecError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let insecure = |reason: &str| {
        SkarbiecError::Deployment(format!(
            "credential store file {}: {reason}",
            path.display()
        ))
    };
    let metadata = std::fs::symlink_metadata(path).map_err(|source| {
        SkarbiecError::Deployment(format!(
            "cannot read credential store file {}: {source}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(insecure("must be a regular file, not a symlink or special file"));
    }
    if metadata.uid() != current_uid()? {
        return Err(insecure("must be owned by the current user"));
    }
    let non_owner_mask = u32::from(u8::MAX >> (u16::BITS / u8::BITS));
    if metadata.permissions().mode() & non_owner_mask != u32::MIN {
        return Err(insecure(
            "must not be accessible by group or other users",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn checked_owner_file(_path: &Path) -> Result<(), SkarbiecError> {
    Ok(())
}

fn read_store_file(path: &Path) -> Result<Value, SkarbiecError> {
    checked_owner_file(path)?;
    let body = std::fs::read_to_string(path).map_err(|source| {
        SkarbiecError::Deployment(format!(
            "cannot read credential store file {}: {source}",
            path.display()
        ))
    })?;
    let doc: Value = serde_json::from_str(&body).map_err(|source| {
        SkarbiecError::Deployment(format!(
            "credential store file {} is not a JSON object of items: {source}",
            path.display()
        ))
    })?;
    if !doc.is_object() {
        return Err(SkarbiecError::Deployment(format!(
            "credential store file {} top level must be an object of items",
            path.display()
        )));
    }
    Ok(doc)
}

/// A missing item mirrors the Skarbiec read path: `read_item` reports
/// `MissingValue`, while `read_string` resolves it to `None`.
fn file_read_item(path: &Path, id: &str) -> Result<Value, SkarbiecError> {
    read_store_file(path)?
        .get(id)
        .cloned()
        .ok_or_else(|| SkarbiecError::MissingValue(id.to_string()))
}

fn file_read_string(path: &Path, id: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Ok(read_store_file(path)?
        .get(id)
        .and_then(|item| item.get(field))
        .and_then(Value::as_str)
        .map(str::to_string))
}

fn configured_client(url: Option<&str>) -> Result<Client, SkarbiecError> {
    match url {
        Some(url) => Client::new(
            url,
            crate::config::skarbiec_consumer(),
            crate::config::skarbiec_token_file(),
        ),
        None => Client::configured(),
    }
}

/// Read one item with the configured consumer grant through the selected
/// store. This is what `Client::configured_item` callers route through.
pub async fn read_item(id: &str) -> Result<Value, SkarbiecError> {
    match selected()? {
        Backend::Skarbiec { url } => configured_client(url.as_deref())?.read_item(id).await,
        Backend::File { path } => file_read_item(&path, id),
    }
}

/// Resolve one optional string field through the selected store. A missing
/// item or field is `None`; transport, schema, and authorization failures
/// remain explicit errors.
pub async fn read_string(id: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    match selected()? {
        Backend::Skarbiec { url } => {
            configured_client(url.as_deref())?.read_string(id, field).await
        }
        Backend::File { path } => file_read_string(&path, id, field),
    }
}

/// Read one item through the selected store for callers that already carry
/// their own Skarbiec coordinates. Under the skarbiec backend the supplied
/// triple is used exactly as before; under the file backend the item comes
/// from the store file instead.
pub async fn read_item_with(
    url: &str,
    consumer: &str,
    token_file: &str,
    id: &str,
) -> Result<Value, SkarbiecError> {
    match selected()? {
        Backend::Skarbiec { url: store_url } => {
            let base = store_url.as_deref().unwrap_or(url);
            Client::new(base, consumer, token_file)?.read_item(id).await
        }
        Backend::File { path } => file_read_item(&path, id),
    }
}
