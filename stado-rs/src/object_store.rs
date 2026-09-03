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

    /// The storage prefix one namespace listing walks.
    ///
    /// A trailing `/` is part of the question. It used to be trimmed off, so
    /// `prefix=queue/` asked the store for `…/probierz/queue` and every
    /// backend answered with a plain `starts_with`: `queue_priority/` and
    /// `queue_workdirs/` came back inside a listing of `queue/`. The client
    /// then refused the whole answer — "Stado object API returned an
    /// inconsistent object-list item", correctly, because those keys are not
    /// under the prefix it asked for — and `JobStorage::list_jobs("queue")`
    /// became permanently unreadable against this fleet's 9,026-object store.
    ///
    /// That is what stopped the release trains on 2026-09-03: `stado host
    /// reclaim` builds its keep-list from `queue/` and `running/`, and back
    /// then it refused the whole reclamation when the store could not be
    /// read, so every `release-capacity` barrier failed on it — 0.13.49 and
    /// 0.13.50 both died there with publication never attempted. Today an
    /// unreadable store costs only the `queue_workdirs` stage, which skips
    /// and names the store's own error.
    ///
    /// So the boundary the caller wrote is the boundary the store is asked
    /// about, and a caller that deliberately passes a partial name still gets
    /// the stem match it asked for.
    pub fn namespace_prefix(namespace: &str, prefix: &str) -> Result<String, StorageError> {
        let sentinel = Self::new(namespace, "sentinel")?;
        let prefix = prefix.trim_start_matches('/');
        let bounded = prefix.ends_with('/');
        let inner = prefix.trim_end_matches('/');
        if (!inner.is_empty()
            && inner
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".."))
            || prefix.contains('\0')
            || prefix.contains('\\')
        {
            return Err(StorageError::PathEscape(format!("{namespace}/{prefix}")));
        }
        Ok(if inner.is_empty() {
            format!("{ROOT_PREFIX}{}/", sentinel.namespace)
        } else if bounded {
            format!("{ROOT_PREFIX}{}/{inner}/", sentinel.namespace)
        } else {
            format!("{ROOT_PREFIX}{}/{inner}", sentinel.namespace)
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
