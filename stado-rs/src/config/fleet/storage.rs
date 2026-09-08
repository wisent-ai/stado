//! Queue storage backend selection, including the disaster-recovery endpoint.

use std::sync::LazyLock;

use crate::config::{resolve_storage_backend, resolve_storage_binding};
use crate::config_file::expand_tilde;

static WC_STORAGE_BACKEND: LazyLock<String> = LazyLock::new(|| resolve_storage_backend(false));
static WC_AZURE_STORAGE_ACCOUNT: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::AzureBlob,
        "account",
        false,
        "",
    )
});
/// An Azure deployment must name its container explicitly. Exported so
/// doctor/deploy preflight can distinguish configured state from the
/// provider-neutral empty default.
pub const DEFAULT_AZURE_CONTAINER: &str = "";
static WC_AZURE_CONTAINER: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::AzureBlob,
        "container",
        false,
        DEFAULT_AZURE_CONTAINER,
    )
});
static WC_S3_BUCKET: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(crate::capabilities::StorageAdapter::S3, "bucket", false, "")
});
static WC_S3_REGION: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::S3,
        "region",
        false,
        "us-east-1",
    )
});
static WC_STADO_STORAGE_URL: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::StadoObject,
        "url",
        false,
        "",
    )
});
static WC_STADO_STORAGE_TOKEN_FILE: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::StadoObject,
        "token-file",
        false,
        "",
    )
});
static WC_STADO_STORAGE_NAMESPACE: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::StadoObject,
        "namespace",
        false,
        "",
    )
});
static WC_STADO_STORAGE_CA_FILE: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::StadoObject,
        "ca-file",
        false,
        "",
    )
});
static WC_LOCAL_STORAGE_PATH: LazyLock<String> = LazyLock::new(|| {
    let default = expand_tilde("~/.stado/local-storage");
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::Local,
        "path",
        false,
        &default.to_string_lossy(),
    )
});
static WC_BACKUP_STORAGE_BACKEND: LazyLock<String> =
    LazyLock::new(|| resolve_storage_backend(true));
static WC_BACKUP_BUCKET: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(crate::capabilities::StorageAdapter::S3, "bucket", true, "")
});
static WC_BACKUP_AZURE_STORAGE_ACCOUNT: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::AzureBlob,
        "account",
        true,
        "",
    )
});
static WC_BACKUP_AZURE_CONTAINER: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::AzureBlob,
        "container",
        true,
        "",
    )
});
static WC_BACKUP_S3_REGION: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(crate::capabilities::StorageAdapter::S3, "region", true, "")
});
static WC_BACKUP_LOCAL_STORAGE_PATH: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(crate::capabilities::StorageAdapter::Local, "path", true, "")
});

/// Queue storage backend (env `WC_STORAGE_BACKEND`). "gcs", "azure", and
/// "s3" support shared workers; "local" is a device-local deployment
/// rooted at [`wc_local_storage_path`].
pub fn wc_storage_backend() -> &'static str {
    WC_STORAGE_BACKEND.as_str()
}

/// Azure storage account for the queue backend (env
/// `WC_AZURE_STORAGE_ACCOUNT`).
pub fn wc_azure_storage_account() -> &'static str {
    WC_AZURE_STORAGE_ACCOUNT.as_str()
}

/// Azure blob container for the queue backend (env `WC_AZURE_CONTAINER`).
pub fn wc_azure_container() -> &'static str {
    WC_AZURE_CONTAINER.as_str()
}

/// S3 bucket for the queue backend (env `WC_S3_BUCKET`).
pub fn wc_s3_bucket() -> &'static str {
    WC_S3_BUCKET.as_str()
}

/// S3 region (env `WC_S3_REGION`, falling back to `AWS_REGION`, then
/// us-east-1).
pub fn wc_s3_region() -> &'static str {
    WC_S3_REGION.as_str()
}

/// HTTPS origin of the Stado object API used as shared queue storage.
pub fn wc_stado_storage_url() -> &'static str {
    WC_STADO_STORAGE_URL.as_str()
}

/// Owner-only file containing the scoped Stado object API bearer token.
pub fn wc_stado_storage_token_file() -> &'static str {
    WC_STADO_STORAGE_TOKEN_FILE.as_str()
}

/// Object namespace containing this deployment's complete queue state.
pub fn wc_stado_storage_namespace() -> &'static str {
    WC_STADO_STORAGE_NAMESPACE.as_str()
}

/// PEM root certificate that signs the Stado object API's HTTPS endpoint.
///
/// A fleet that publishes its object API on the tailnet is served by a private
/// certificate authority the operating system has never heard of. Without this the
/// client has only the system roots, every request to that endpoint dies in the
/// handshake as "error sending request", and the sole configuration left standing
/// is a loopback URL -- so each host addresses its own store and the fleet stops
/// sharing one registry. Empty means a publicly trusted authority, or loopback.
pub fn wc_stado_storage_ca_file() -> &'static str {
    WC_STADO_STORAGE_CA_FILE.as_str()
}

/// Root directory of the device-local storage backend (env
/// `WC_LOCAL_STORAGE_PATH`).
pub fn wc_local_storage_path() -> &'static str {
    WC_LOCAL_STORAGE_PATH.as_str()
}

/// Disaster-recovery storage backend. Empty means no backup is configured.
///
/// Queue mutations commit to the configured primary and are then mirrored
/// best-effort to this endpoint. Reads consult it only when the primary
/// returns an error; an authoritative primary `absent` result never falls
/// through, so the backup cannot become a second writer or dispatch queue.
pub fn wc_backup_storage_backend() -> &'static str {
    WC_BACKUP_STORAGE_BACKEND.as_str()
}

/// GCS or S3 bucket used by the disaster-recovery endpoint.
pub fn wc_backup_bucket() -> &'static str {
    WC_BACKUP_BUCKET.as_str()
}

/// Azure account used by the disaster-recovery endpoint.
pub fn wc_backup_azure_storage_account() -> &'static str {
    WC_BACKUP_AZURE_STORAGE_ACCOUNT.as_str()
}

/// Azure container used by the disaster-recovery endpoint.
pub fn wc_backup_azure_container() -> &'static str {
    WC_BACKUP_AZURE_CONTAINER.as_str()
}

/// S3 region used by the disaster-recovery endpoint.
pub fn wc_backup_s3_region() -> &'static str {
    WC_BACKUP_S3_REGION.as_str()
}

/// Local path used by the disaster-recovery endpoint.
pub fn wc_backup_local_storage_path() -> &'static str {
    WC_BACKUP_LOCAL_STORAGE_PATH.as_str()
}
