//! Provider-neutral object names for product data stored behind Stado.
//!
//! Ecosystem callers use `stado://<namespace>/<key>`. The selected cloud
//! backend, account, bucket, and container never cross this boundary. Stado
//! stores these objects below `ecosystem/`, outside the queue lifecycle
//! prefixes, so product data cannot collide with scheduler state.

use std::collections::BTreeMap;
use std::fmt;

use crate::queue::StorageError;

pub const ROOT_PREFIX: &str = "ecosystem/";
/// Exact body limit shared by the object API server and client.
pub fn max_object_bytes() -> usize {
    (u32::MAX as usize / u8::BITS as usize).saturating_add(usize::from(true))
}

/// Largest independently authenticated chunk accepted by the object API.
///
/// The client and server share this value: raising it on only one side either
/// restores the stalled single-request upload or makes valid chunks fail
/// composition.
pub const OBJECT_API_CHUNK_BYTES: usize = 3 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ObjectRef {
    namespace: String,
    key: String,
}

impl ObjectRef {
    pub fn new(namespace: &str, key: &str) -> Result<Self, StorageError> {
        let namespace = namespace.trim();
        let key = key.trim_matches('/');
        if namespace == "public" {
            return Err(StorageError::Other(
                "the stado://public namespace is retired; use authenticated product namespaces or the dedicated stado://releases software channel"
                    .to_string(),
            ));
        }
        if namespace.is_empty()
            || !namespace
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(StorageError::Other(
                "Stado object namespace must contain only lowercase letters, digits, and '-'"
                    .to_string(),
            ));
        }
        if key.is_empty()
            || key
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || key.contains('\0')
            || key.contains('\\')
        {
            return Err(StorageError::PathEscape(format!("{namespace}/{key}")));
        }
        Ok(Self {
            namespace: namespace.to_string(),
            key: key.to_string(),
        })
    }

    pub fn parse(value: &str) -> Result<Self, StorageError> {
        let value = value.trim();
        let value = value.strip_prefix("stado://").unwrap_or(value);
        let (namespace, key) = value.split_once('/').ok_or_else(|| {
            StorageError::Other(
                "Stado object must be stado://<namespace>/<key> or <namespace>/<key>".to_string(),
            )
        })?;
        Self::new(namespace, key)
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn storage_path(&self) -> String {
        format!("{ROOT_PREFIX}{}/{}", self.namespace, self.key)
    }

    /// The storage prefix one namespace listing scans.
    ///
    /// A trailing `/` is kept, because it is the whole difference between
    /// "inside this directory" and "every sibling whose name starts with
    /// these letters". Trimming it here made `prefix=queue/` scan
    /// `queue_priority/` too: on 2026-09-02 at 23:32 a migration created that
    /// sibling, and the next release train died in `release-capacity` because
    /// `list_jobs("queue")` was handed 9026 priority markers to parse as jobs
    /// and reported the queue store unreadable. Leading slashes are noise and
    /// are still stripped.
    pub fn namespace_prefix(namespace: &str, prefix: &str) -> Result<String, StorageError> {
        let sentinel = Self::new(namespace, "sentinel")?;
        let prefix = prefix.trim_start_matches('/');
        let path = prefix.trim_end_matches('/');
        if (!path.is_empty()
            && path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".."))
            || prefix.contains('\0')
            || prefix.contains('\\')
        {
            return Err(StorageError::PathEscape(format!("{namespace}/{prefix}")));
        }
        Ok(if path.is_empty() {
            format!("{ROOT_PREFIX}{}/", sentinel.namespace)
        } else {
            format!("{ROOT_PREFIX}{}/{prefix}", sentinel.namespace)
        })
    }

    pub fn from_storage_path(path: &str) -> Result<Self, StorageError> {
        let relative = path.strip_prefix(ROOT_PREFIX).ok_or_else(|| {
            StorageError::Other(format!(
                "object is outside the Stado ecosystem namespace: {path}"
            ))
        })?;
        Self::parse(relative)
    }
}

pub fn metadata(object: &ObjectRef, content_type: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("content-type".to_string(), content_type.to_string()),
        (
            "stado-namespace".to_string(),
            object.namespace().to_string(),
        ),
        ("stado-uri".to_string(), object.to_string()),
    ])
}

/// The release policy key this object is authorized against, or `None` when the
/// object is not release-governed.
///
/// The server's authorization and the client's credential resolution both read
/// this one function, so a route cannot be governed on one end and unsigned on
/// the other. That split is exactly how an immutable release write left the CLI
/// carrying the coordinator storage token and came back as a `401` naming
/// neither the policy table nor the item it wanted.
pub fn release_policy_key(namespace: &str, key: &str) -> Option<String> {
    match namespace {
        "releases" | "sources" => Some(key.to_string()),
        "system" => {
            let product = key
                .strip_prefix("release-catalog/")?
                .strip_suffix(".json")?;
            if product.is_empty() || product.contains('/') {
                None
            } else {
                Some(format!("{product}/catalog.json"))
            }
        }
        _ => None,
    }
}

impl fmt::Display for ObjectRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "stado://{}/{}", self.namespace, self.key)
    }
}
