//! The single error every quota read — live cloud limits, reservation
//! overlay, running counts — folds its provider-specific failure into.

use crate::providers::azure::AzureError;
use crate::providers::gcp::GceError;
use crate::providers::ProviderError;
use crate::queue::StorageError;

/// Quota-read error.
#[derive(Debug, thiserror::Error)]
pub enum QuotaError {
    /// GCP (GCE REST) failures from the live regions.get fan-out.
    #[error(transparent)]
    Gcp(#[from] GceError),
    /// Azure ARM failures from the regional usages fan-out.
    #[error(transparent)]
    Azure(#[from] AzureError),
    /// Storage failures reading the reservation overlay.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Python `json.JSONDecodeError` on a corrupt `config/quotas.json`.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Provider failures from `list_running_instances`.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}
