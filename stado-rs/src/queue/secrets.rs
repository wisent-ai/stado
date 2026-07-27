//! Operator secret storage on the configured queue backend.
//!
//! NO Python original. The Python tree has no secret store of its own: its
//! only secret path is GCP Secret Manager, read directly by
//! `monitor/billing.py::_fetch_azure_sp` and
//! `providers/vast.py::_fetch_secret_manager_key`. That is the exact
//! cross-cloud coupling this module exists to break — the AZURE billing
//! service principal lived in GCP Secret Manager, so a GCP billing shutdown
//! also blinded the Azure credit tracker (see
//! `monitor/billing.rs::azure_section`).
//!
//! A secret is a plain blob at `secrets/<name>` on whatever backend
//! `JobStorage` resolved, so it survives wherever the queue survives and it
//! travels with a backend migration (`secrets/` is listed in
//! `queue/copy.rs::CANONICAL_PREFIXES`).
//!
//! # This is at-rest storage, NOT a KMS — do not sell it as one
//!
//! The only confidentiality this layer provides is the confidentiality the
//! bucket/container already has: its own access control plus whatever
//! server-side encryption at rest the provider applies to every other
//! object. This module adds nothing on top. Concretely, there is:
//!
//! - no envelope encryption and no per-secret key — the value is stored as
//!   the literal bytes handed in, readable by anyone who can read the queue
//!   bucket (which is everyone who can read a job payload);
//! - no key rotation, no secret versioning, no expiry, no split knowledge,
//!   no HSM, no separate audit trail beyond the bucket's own object access
//!   logging;
//! - no protection from an operator or service account that already holds
//!   queue read access.
//!
//! It is the right home for credentials whose blast radius is already the
//! queue itself (a read-only billing service principal). Material that
//! genuinely needs a KMS still belongs in one.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::Serialize;

use super::{BlobBackend, JobStorage, StorageError};

/// Blob prefix every secret lives under.
pub const PREFIX: &str = "secrets/";

/// Blob metadata key carrying the value length, stamped by [`write`] so
/// [`list`] can report a size without ever downloading secret material.
const SIZE_METADATA_KEY: &str = "bytes";

/// The character gate of `queue/leases.rs::SAFE_JOB_ID`, applied to secret
/// names for the same reason it is applied to job ids: with the separator
/// excluded a name is always exactly one path component, so a name can
/// never address a blob outside [`PREFIX`].
static SAFE_SECRET_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]+$").expect("static regex compiles"));

/// One stored secret as [`list`] reports it. Deliberately carries no value:
/// nothing in the listing path ever holds secret material.
#[derive(Debug, Clone, Serialize)]
pub struct SecretInfo {
    pub name: String,
    /// Value length in bytes, from the [`SIZE_METADATA_KEY`] stamp; `None`
    /// when the backend has no metadata for the blob. Recovering the length
    /// by downloading the value is deliberately NOT done — a listing must
    /// not pull secrets into this process.
    pub bytes: Option<usize>,
    /// Backend last-modified time; `None` when the backend reports none.
    pub updated: Option<DateTime<Utc>>,
}

/// `secrets/<name>`, once the name is proven to be a single safe path
/// component. Public so callers that report where a secret was looked for
/// (`monitor/billing.rs::no_credentials_section`) name the real blob.
pub fn blob_path(name: &str) -> Result<String, StorageError> {
    if !SAFE_SECRET_NAME.is_match(name) {
        return Err(StorageError::Other(format!(
            "secret name {name:?} is unsafe for secret storage \
             (allowed: letters, digits, '.', '_' and '-')"
        )));
    }
    // The character gate admits the relative path components, which resolve
    // above the prefix instead of naming something inside it.
    if name.chars().all(|character| character == '.') {
        return Err(StorageError::Other(format!(
            "secret name {name:?} is a relative path component, not a name"
        )));
    }
    Ok(format!("{PREFIX}{name}"))
}

/// The stored value, or `None` when no such secret exists.
pub async fn read(store: &JobStorage, name: &str) -> Result<Option<String>, StorageError> {
    store.download_text(&blob_path(name)?).await
}

/// Create or overwrite a secret. Last writer wins: there is one version of
/// a secret, and replacing it destroys the previous value.
pub async fn write(store: &JobStorage, name: &str, value: &str) -> Result<(), StorageError> {
    let path = blob_path(name)?;
    store.upload_text(&path, value).await?;
    let meta = BTreeMap::from([(SIZE_METADATA_KEY.to_string(), value.len().to_string())]);
    store.backend().set_metadata(&path, &meta).await
}

/// Every stored secret, by name. Never reads a value.
pub async fn list(store: &JobStorage) -> Result<Vec<SecretInfo>, StorageError> {
    let mut secrets: Vec<SecretInfo> = store
        .list_blobs_with_meta(PREFIX)
        .await?
        .into_iter()
        .filter_map(|info| {
            let name = info.name.strip_prefix(PREFIX)?;
            if name.is_empty() {
                return None;
            }
            Some(SecretInfo {
                name: name.to_string(),
                bytes: info
                    .metadata
                    .get(SIZE_METADATA_KEY)
                    .and_then(|raw| raw.parse().ok()),
                updated: info.updated,
            })
        })
        .collect();
    secrets.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(secrets)
}

/// Remove a secret. Idempotent; the flag reports whether it was there.
pub async fn delete(store: &JobStorage, name: &str) -> Result<bool, StorageError> {
    let path = blob_path(name)?;
    let existed = store.backend().exists(&path).await?;
    store.delete_blob(&path).await?;
    Ok(existed)
}
