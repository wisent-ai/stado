//! Client for the separate Skarbiec credential service.
//!
//! Skarbiec is reached over loopback HTTP or a TLS-protected remote endpoint
//! and enforces scoped consumer grants. It is the default backend; every
//! application-credential CRUD call routes through `crate::credential_store`,
//! selected by `STADO_CREDENTIALS_STORE`. Backend bootstrap grants remain
//! direct because a manager cannot store the credential required to unlock
//! itself.
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
pub(crate) mod envelope;
mod error;
mod gcp;
pub mod roles;
mod tokens;
pub mod validate;
mod verifiers;

pub use client::Client;
pub use error::SkarbiecError;
pub use gcp::gcp_provider;
pub use tokens::*;
pub use validate::*;

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

/// Which version of one item field a read answers: the item that answered
/// (a role or a rename can move it), the item's identity (new after a purge
/// and recreate; `None` on an item Skarbiec has not stamped) and the revision
/// of its current value, which every new value raises. Two equal versions
/// name the same value, so a holder of a value reads it again only when the
/// vault's version differs from the one it read it under.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ItemVersion {
    pub item: String,
    pub item_uid: Option<String>,
    pub revision: u64,
}

/// One field value with the version it was read under; `version` is `None`
/// when the store keeps no versions (the file backend), so the value has to
/// be read again for every check.
#[derive(Debug, Clone)]
pub struct VersionedValue {
    pub value: String,
    pub version: Option<ItemVersion>,
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
    if crate::primitives::file_mode::open_to_others(metadata.permissions().mode()) {
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

/// Bearers of transient handoffs, keyed by consumer and grant file. A transient
/// grant is handed off once and erased from disk, so this cache is what keeps
/// the process able to authenticate for the rest of its life.
pub(crate) static TRANSIENT_GRANTS: LazyLock<Mutex<HashMap<(String, String), String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The directory the cloud agent extensions hand a grant off in. It is created
/// and populated by the platform, not named by whoever wrote the deployment
/// configuration, and a file in it exists to be consumed once and removed.
const TRANSIENT_AGENT_GRANT_DIR: &str = "/run/stado-agent-credentials";

/// How a client's grant file is meant to be consumed.
///
/// Stated by each construction site, never inferred inside the read path. It
/// used to be inferred twice, from two different facts, and the two could
/// disagree: `request_token` keyed caching and erasure on the consumer name
/// ending in `-agent`, while the erase itself re-derived the same decision from
/// the grant file's parent directory. Both disagreements were reachable — a
/// `-agent` consumer whose grant lives outside the handoff directory cached its
/// bearer for the life of the process while the erase silently did nothing, so
/// a rotated grant was never picked up; and a consumer not named `-agent` whose
/// grant *is* the handoff re-read a file that was never erased, leaving a
/// one-shot bearer readable on disk for the life of the machine. One declared
/// fact, honoured in one place, is why they can no longer disagree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GrantMode {
    /// A one-shot handoff: read once, kept in this process for the rest of its
    /// life, and erased from disk immediately. The file is gone after the first
    /// read, so re-reading it would fail every later request.
    TransientHandoff,
    /// A grant file that stays where it is: re-read on every request, so a
    /// rotated grant is picked up without a restart. Never erased.
    RereadPerRequest,
}

impl GrantMode {
    /// The mode implied by where the platform put the grant file.
    ///
    /// A site that knows nothing about the grant beyond the path it was
    /// configured with can state this: the cloud agent extensions write a
    /// one-shot bearer into [`TRANSIENT_AGENT_GRANT_DIR`] and nothing else
    /// does, so the placement is a fact about the grant rather than a guess
    /// about its name. This is a way for a caller to *state* a mode; it is not
    /// a gate inside the read path.
    pub fn for_grant_file(token_file: &str) -> Self {
        if Path::new(token_file.trim()).parent() == Some(Path::new(TRANSIENT_AGENT_GRANT_DIR)) {
            return Self::TransientHandoff;
        }
        Self::RereadPerRequest
    }
}

/// Erase a one-shot handoff once its bearer has been cached.
///
/// The caller has already declared that this file is a transient handoff, so
/// there is no second gate here. Re-deriving that fact from the path — which is
/// what this function used to do — is exactly the silent veto that let caching
/// and erasure disagree, and it made a declared mode unenforceable.
pub(crate) fn erase_transient_grant(path: &str, byte_count: usize) {
    let path = Path::new(path);
    if let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = std::io::copy(
            &mut std::io::repeat(b'\0').take(u64::try_from(byte_count).unwrap_or(u64::MAX)),
            &mut file,
        );
        let _ = file.sync_all();
    }
    let _ = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path);
    let _ = std::fs::remove_file(path);
}

/// Resolve one optional string field from the selected credential store. A
/// missing item is `None`; authentication, transport, schema, and authorization
/// failures remain explicit and never trigger an alternate source.
pub async fn read_string(id: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    crate::credential_store::read_string(id, field).await
}
