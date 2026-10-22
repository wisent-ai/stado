//! The `storage.s3_compatible.*` bindings every store on an S3 API endpoint
//! other than Amazon's reads: the generic `s3-compatible` backend and each
//! named provider (`queue::s3::vendor`). None has a default: the backend
//! refuses a missing one by its variable and configuration key.

use std::sync::LazyLock;

use crate::capabilities::StorageAdapter;
use crate::config::resolve_storage_binding;
use crate::queue::s3::StorageVendor;

fn binding(key: &str, backup: bool) -> String {
    resolve_storage_binding(StorageAdapter::S3Compatible, key, backup, "")
}

/// The account a named provider's endpoint names. Only the providers whose
/// endpoint carries one declare the field, so it is read through one of them;
/// every provider shares the one `storage.s3_compatible.account` key.
fn account(backup: bool) -> String {
    resolve_storage_binding(
        StorageAdapter::S3Vendor(StorageVendor::CloudflareR2),
        "account",
        backup,
        "",
    )
}

static ENDPOINT: LazyLock<String> = LazyLock::new(|| binding("endpoint", false));
static BUCKET: LazyLock<String> = LazyLock::new(|| binding("bucket", false));
static REGION: LazyLock<String> = LazyLock::new(|| binding("region", false));
static ADDRESSING: LazyLock<String> = LazyLock::new(|| binding("addressing", false));
static CREDENTIAL_ROLE: LazyLock<String> = LazyLock::new(|| binding("credential-role", false));
static ACCOUNT: LazyLock<String> = LazyLock::new(|| account(false));
static BACKUP_ENDPOINT: LazyLock<String> = LazyLock::new(|| binding("endpoint", true));
static BACKUP_REGION: LazyLock<String> = LazyLock::new(|| binding("region", true));
static BACKUP_ADDRESSING: LazyLock<String> = LazyLock::new(|| binding("addressing", true));
static BACKUP_CREDENTIAL_ROLE: LazyLock<String> =
    LazyLock::new(|| binding("credential-role", true));
static BACKUP_ACCOUNT: LazyLock<String> = LazyLock::new(|| account(true));

/// The S3 API endpoint URL of the store.
pub fn wc_s3_compatible_endpoint() -> &'static str {
    ENDPOINT.as_str()
}

/// The bucket the store keeps its objects in.
pub fn wc_s3_compatible_bucket() -> &'static str {
    BUCKET.as_str()
}

/// The region the store's requests are signed for.
pub fn wc_s3_compatible_region() -> &'static str {
    REGION.as_str()
}

/// `path` or `virtual-hosted`: where a generic endpoint expects the bucket.
pub fn wc_s3_compatible_addressing() -> &'static str {
    ADDRESSING.as_str()
}

/// The credential-store role whose item holds the store's key pair.
pub fn wc_s3_compatible_credential_role() -> &'static str {
    CREDENTIAL_ROLE.as_str()
}

/// The account a named provider's endpoint names.
pub fn wc_s3_compatible_account() -> &'static str {
    ACCOUNT.as_str()
}

/// The S3 API endpoint URL of the disaster-recovery store.
pub fn wc_backup_s3_compatible_endpoint() -> &'static str {
    BACKUP_ENDPOINT.as_str()
}

/// The region the disaster-recovery store is signed for.
pub fn wc_backup_s3_compatible_region() -> &'static str {
    BACKUP_REGION.as_str()
}

/// The addressing of the disaster-recovery store.
pub fn wc_backup_s3_compatible_addressing() -> &'static str {
    BACKUP_ADDRESSING.as_str()
}

/// The credential role of the disaster-recovery store.
pub fn wc_backup_s3_compatible_credential_role() -> &'static str {
    BACKUP_CREDENTIAL_ROLE.as_str()
}

/// The account of the disaster-recovery store's named provider.
pub fn wc_backup_s3_compatible_account() -> &'static str {
    BACKUP_ACCOUNT.as_str()
}
