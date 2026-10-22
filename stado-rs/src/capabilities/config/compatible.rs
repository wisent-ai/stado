//! The configuration a store on an S3 API endpoint other than Amazon's
//! reads: the generic `s3-compatible` backend and each named provider
//! (`queue::s3::vendor`). Every provider reads the same
//! `storage.s3_compatible.*` keys, each only the ones its endpoint is built
//! from (`queue::s3::vendor::Declares`). None has a default: the endpoint
//! URL, region and account are the provider's, the addressing is what the
//! endpoint serves, and the key pair is read from the credential store by
//! role, from the item's `access_key_id` and `secret_access_key` fields.

use super::field::ConfigField;

const BACKUP_BUCKET_ENV: &str = "WC_BACKUP_BUCKET";
const BACKUP_BUCKET_PATH: &str = "storage.backup.bucket";

const ENDPOINT: ConfigField = ConfigField::scalar(
    "endpoint",
    "WC_S3_COMPATIBLE_ENDPOINT",
    "storage.s3_compatible.endpoint",
)
.required()
.with_backup(
    "WC_BACKUP_S3_COMPATIBLE_ENDPOINT",
    "storage.backup.s3_compatible.endpoint",
    true,
);
const BUCKET: ConfigField = ConfigField::scalar(
    "bucket",
    "WC_S3_COMPATIBLE_BUCKET",
    "storage.s3_compatible.bucket",
)
.required()
.with_backup(BACKUP_BUCKET_ENV, BACKUP_BUCKET_PATH, true);
const REGION: ConfigField = ConfigField::scalar(
    "region",
    "WC_S3_COMPATIBLE_REGION",
    "storage.s3_compatible.region",
)
.required()
.with_backup(
    "WC_BACKUP_S3_COMPATIBLE_REGION",
    "storage.backup.s3_compatible.region",
    true,
);
const ADDRESSING: ConfigField = ConfigField::scalar(
    "addressing",
    "WC_S3_COMPATIBLE_ADDRESSING",
    "storage.s3_compatible.addressing",
)
.required()
.with_backup(
    "WC_BACKUP_S3_COMPATIBLE_ADDRESSING",
    "storage.backup.s3_compatible.addressing",
    true,
);
const CREDENTIAL_ROLE: ConfigField = ConfigField::scalar(
    "credential-role",
    "WC_S3_COMPATIBLE_CREDENTIAL_ROLE",
    "storage.s3_compatible.credential_role",
)
.required()
.with_backup(
    "WC_BACKUP_S3_COMPATIBLE_CREDENTIAL_ROLE",
    "storage.backup.s3_compatible.credential_role",
    true,
);
const ACCOUNT: ConfigField = ConfigField::scalar(
    "account",
    "WC_S3_COMPATIBLE_ACCOUNT",
    "storage.s3_compatible.account",
)
.required()
.with_backup(
    "WC_BACKUP_S3_COMPATIBLE_ACCOUNT",
    "storage.backup.s3_compatible.account",
    true,
);

/// Any S3 API endpoint named by URL.
pub(in crate::capabilities) const S3_COMPATIBLE_CONFIG: &[ConfigField] =
    &[ENDPOINT, BUCKET, REGION, ADDRESSING, CREDENTIAL_ROLE];
/// A provider whose endpoint names the account (Cloudflare R2).
pub(in crate::capabilities) const S3_VENDOR_ACCOUNT_CONFIG: &[ConfigField] =
    &[BUCKET, ACCOUNT, CREDENTIAL_ROLE];
/// A provider whose endpoint names the region (Backblaze B2, Wasabi, …).
pub(in crate::capabilities) const S3_VENDOR_REGION_CONFIG: &[ConfigField] =
    &[BUCKET, REGION, CREDENTIAL_ROLE];
/// A provider whose endpoint names both (OCI, UpCloud).
pub(in crate::capabilities) const S3_VENDOR_ACCOUNT_REGION_CONFIG: &[ConfigField] =
    &[BUCKET, ACCOUNT, REGION, CREDENTIAL_ROLE];
/// A provider with one endpoint and one region for everyone (Filebase, …).
pub(in crate::capabilities) const S3_VENDOR_FIXED_CONFIG: &[ConfigField] =
    &[BUCKET, CREDENTIAL_ROLE];
/// A server the deployment runs (MinIO, Garage, SeaweedFS, Ceph) or was
/// issued an endpoint for (IDrive e2).
pub(in crate::capabilities) const S3_VENDOR_SELF_HOSTED_CONFIG: &[ConfigField] =
    &[ENDPOINT, BUCKET, REGION, CREDENTIAL_ROLE];
