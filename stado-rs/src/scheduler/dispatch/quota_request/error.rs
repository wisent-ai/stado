//! The quota-write error enum and its Python `type(exc).__name__`
//! rendering, which the GCP fan-out stamps into per-target error rows.

use crate::providers::azure::AzureError;

use super::quota_skus::CatalogError;

/// Quota-write error.
#[derive(Debug, thiserror::Error)]
pub enum QuotaRequestError {
    /// GCP Cloud Quotas REST failures.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// Azure ARM failures.
    #[error(transparent)]
    Azure(#[from] AzureError),
    /// Python `ValueError` (unknown accel label).
    #[error("{0}")]
    Value(String),
}

/// Python `type(exc).__name__` for per-target error rows.
pub(super) fn py_type_name(err: &QuotaRequestError) -> &'static str {
    match err {
        QuotaRequestError::Catalog(_) => "GoogleAPICallError",
        QuotaRequestError::Azure(_) => "AzureError",
        QuotaRequestError::Value(_) => "ValueError",
    }
}
