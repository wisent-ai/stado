//! The single storage-adapter factory: locators in, `Arc<dyn BlobBackend>`
//! out. Bodies lifted out of `queue/mod.rs` unchanged.

use std::sync::Arc;

use crate::capabilities::StorageAdapter;
use crate::queue::{AzureBlobBackend, GcsBackend, LocalBackend, S3Backend, StadoObjectBackend};

use super::{BlobBackend, StorageError};

/// Concrete locators consumed by the single storage-adapter factory.
///
/// The catalog selects the adapter; callers supply only endpoint values. This
/// keeps constructor ownership here instead of duplicating backend-name
/// switches in the queue facade, copier, doctor, and recovery commands.
pub(crate) struct BackendLocator<'a> {
    pub bucket: &'a str,
    /// An Azure storage account, or the account a named S3 API provider's
    /// endpoint names.
    pub account: &'a str,
    pub container: &'a str,
    pub region: &'a str,
    pub path: &'a str,
    /// The S3 API endpoint URL of an `s3-compatible` store or a provider the
    /// deployment runs itself.
    pub endpoint: &'a str,
    /// `path` or `virtual-hosted` for an `s3-compatible` store.
    pub addressing: &'a str,
    /// The credential role holding an S3 API store's key pair.
    pub credential_role: &'a str,
}

pub(crate) async fn construct_backend(
    adapter: StorageAdapter,
    locator: BackendLocator<'_>,
) -> Result<Arc<dyn BlobBackend>, StorageError> {
    match adapter {
        StorageAdapter::Gcs => Ok(Arc::new(GcsBackend::new(locator.bucket).await?)),
        StorageAdapter::AzureBlob => Ok(Arc::new(AzureBlobBackend::new(
            locator.account,
            locator.container,
        )?)),
        StorageAdapter::S3 => Ok(Arc::new(
            S3Backend::new(locator.bucket, locator.region).await?,
        )),
        StorageAdapter::S3Compatible | StorageAdapter::S3Vendor(_) => Ok(Arc::new(
            S3Backend::compatible(crate::queue::S3CompatibleLocator {
                vendor: adapter.vendor(),
                endpoint: locator.endpoint,
                bucket: locator.bucket,
                region: locator.region,
                account: locator.account,
                addressing: locator.addressing,
                credential_role: locator.credential_role,
            })
            .await?,
        )),
        StorageAdapter::StadoObject => Ok(Arc::new(StadoObjectBackend::new(
            &crate::config::wc_stado_storage_url().map_err(crate::queue::StorageError::Other)?,
            crate::config::wc_stado_storage_namespace(),
            crate::config::wc_stado_storage_token_file(),
            crate::config::wc_stado_storage_ca_file(),
        )?)),
        StorageAdapter::Local => Ok(Arc::new(LocalBackend::new(locator.path)?)),
    }
}
