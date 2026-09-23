//! An API's authority belongs to that listener, not to the resident worker.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use crate::capabilities::StorageAdapter;
use crate::queue::copy::Endpoint;
use crate::queue::{LocalBackend, StorageError};

use super::JobStorage;

/// Explicit primary and mirror for an API sharing a process with other roles.
/// No environment mutation is needed to select the listener's authority.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerStorage {
    pub primary: Endpoint,
    #[serde(default)]
    pub backup: Option<Endpoint>,
}

impl FromStr for ServerStorage {
    type Err = serde_json::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(value)
    }
}

impl fmt::Display for ServerStorage {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        let json = serde_json::to_string(self).map_err(|_| fmt::Error)?;
        output.write_str(&json)
    }
}

impl JobStorage {
    /// Bind only the API to these endpoints. Reads never fail over from its
    /// authoritative primary; the worker keeps its own configured storage.
    pub async fn for_server_storage(profile: ServerStorage) -> Result<Self, StorageError> {
        let ServerStorage { mut primary, backup } = profile;
        if primary.adapter() == Some(StorageAdapter::StadoObject) {
            return Err(StorageError::Other(
                "the Stado API server requires a direct authoritative primary; a stado endpoint names an API, not its backing store".into(),
            ));
        }
        let local_path = if primary.adapter() == Some(StorageAdapter::Local) {
            primary.path = LocalBackend::resolved_root(&primary.path)?
                .to_string_lossy()
                .into_owned();
            Some(Arc::from(primary.path.as_str()))
        } else {
            None
        };
        let backend = primary.build().await.map_err(|error| {
            StorageError::Other(format!(
                "constructing API primary {}: {error}", primary.describe()
            ))
        })?;
        let mut storage = Self::with_backend_and_bucket(backend, &primary.kind, &primary.bucket);
        storage.local_path = local_path;
        storage.ensure_layout().await.map_err(|error| {
            StorageError::Other(format!(
                "checking API primary layout {}: {error}", primary.describe()
            ))
        })?;
        storage
            .with_read_failover(primary, backup, super::failover::ReadMode::PrimaryOnly)
            .await
    }
}
