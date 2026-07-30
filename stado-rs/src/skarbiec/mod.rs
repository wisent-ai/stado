//! Client for the separate Skarbiec credential service.
//!
//! Skarbiec is reached over its loopback HTTP API or a TLS-protected remote
//! endpoint. Application credentials are decrypted by Skarbiec, authorized by
//! an action-scoped consumer grant, and held only in the requesting Stado
//! process. Skarbiec is the default credential store; `STADO_CREDENTIAL_STORE`
//! (see `crate::credential_store`) may select the guarded JSON file backend
//! for the two plain read helpers (`read_string`, `Client::configured_item`),
//! while every verifier grant below always talks to Skarbiec.
//!
//! This module is a directory split of the former single `skarbiec.rs`: the
//! public API surface is re-exported unchanged so every `crate::skarbiec::…`
//! import compiles exactly as before.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

mod client;
mod gcp;
mod tokens;
pub mod validate;
mod verifiers;

pub use client::Client;
pub use gcp::gcp_provider;
pub use tokens::*;
pub use validate::*;

#[derive(Debug, thiserror::Error)]
pub enum SkarbiecError {
    #[error("invalid Skarbiec URL {0:?}; expected loopback HTTP or HTTPS")]
    InvalidUrl(String),
    #[error("Skarbiec consumer is not configured; set WC_SKARBIEC_CONSUMER")]
    MissingConsumer,
    #[error("Skarbiec grant file is not configured; set WC_SKARBIEC_TOKEN_FILE")]
    MissingTokenFile,
    #[error("cannot read Skarbiec grant file {path}: {source}")]
    TokenFile {
        path: String,
        source: std::io::Error,
    },
    #[error("Skarbiec grant file {0} must not be accessible by group or other users")]
    InsecureTokenFile(String),
    #[error("Skarbiec grant file {0} is empty")]
    EmptyToken(String),
    #[error("Skarbiec request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Skarbiec returned HTTP {status}: {detail}")]
    Response { status: u16, detail: String },
    #[error("Skarbiec item {0:?} has no value")]
    MissingValue(String),
    #[error("Skarbiec deployment configuration: {0}")]
    Deployment(String),
    #[error("cannot acquire GCP workload or Skarbiec identity: {0}")]
    GcpAuth(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ItemInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: Option<String>,
    pub tags: Option<Vec<String>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub deleted: Option<bool>,
    pub versions: Option<usize>,
}

pub(crate) fn checked_url(base_url: &str) -> Result<String, SkarbiecError> {
    let base_url = base_url.trim().trim_end_matches('/');
    let parsed =
        url::Url::parse(base_url).map_err(|_| SkarbiecError::InvalidUrl(base_url.to_string()))?;
    let loopback = match parsed.host() {
        Some(url::Host::Ipv4(host)) => host.is_loopback(),
        Some(url::Host::Ipv6(host)) => host.is_loopback(),
        Some(url::Host::Domain(host)) => host == "localhost",
        None => false,
    };
    let transport_allowed = parsed.scheme() == "https" || (parsed.scheme() == "http" && loopback);
    if !transport_allowed
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || (parsed.path() != "/" && !parsed.path().is_empty())
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(SkarbiecError::InvalidUrl(base_url.to_string()));
    }
    Ok(base_url.to_string())
}

#[cfg(unix)]
fn reject_insecure_mode(path: &Path) -> Result<(), SkarbiecError> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path).map_err(|source| SkarbiecError::TokenFile {
        path: path.display().to_string(),
        source,
    })?;
    let non_owner_mask = u32::from(u8::MAX >> (u16::BITS / u8::BITS));
    if metadata.permissions().mode() & non_owner_mask != u32::MIN {
        return Err(SkarbiecError::InsecureTokenFile(path.display().to_string()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn reject_insecure_mode(_path: &Path) -> Result<(), SkarbiecError> {
    Ok(())
}

pub(crate) fn read_grant(path: &str) -> Result<String, SkarbiecError> {
    if path.trim().is_empty() {
        return Err(SkarbiecError::MissingTokenFile);
    }
    let path = Path::new(path);
    reject_insecure_mode(path)?;
    let token = std::fs::read_to_string(path).map_err(|source| SkarbiecError::TokenFile {
        path: path.display().to_string(),
        source,
    })?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(SkarbiecError::EmptyToken(path.display().to_string()));
    }
    Ok(token)
}

pub(crate) static AGENT_GRANTS: LazyLock<Mutex<HashMap<(String, String), String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const TRANSIENT_AGENT_GRANT_DIR: &str = "/run/stado-agent-credentials";

/// Erase the protected-settings handoff after an agent has cached its grant.
/// Persistent local/Darwin agent grants live elsewhere and intentionally
/// survive process restarts.
pub(crate) fn erase_transient_agent_grant(path: &str, byte_count: usize) {
    let path = Path::new(path);
    if path.parent() != Some(Path::new(TRANSIENT_AGENT_GRANT_DIR)) {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = std::io::copy(
            &mut std::io::repeat(u8::MIN).take(u64::try_from(byte_count).unwrap_or(u64::MAX)),
            &mut file,
        );
        let _ = file.set_len(u64::MIN);
        let _ = file.sync_all();
    }
    let _ = std::fs::remove_file(path);
}

/// Resolve one optional string field from a Skarbiec item. A missing item is
/// `None`; authentication, transport, schema, and authorization failures remain
/// explicit errors and never trigger an alternate credential source. The read
/// flows through the credential store selector, so a file backend selected via
/// `STADO_CREDENTIAL_STORE` answers it and the default skarbiec backend is
/// byte-identical to the direct client call.
pub async fn read_string(id: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    crate::credential_store::read_string(id, field).await
}
