//! The flag set that addresses two stores in one command.

use crate::cli::storage::*;

/// The locator flags shared by `copy` and `verify`, so both commands
/// address a pair of stores with an identical flag set.
#[derive(Args, Debug)]
pub struct EndpointArgs {
    /// Source backend.
    #[arg(long, value_parser = parse_storage_kind)]
    pub(crate) from: String,
    /// Destination backend.
    #[arg(long, value_parser = parse_storage_kind)]
    pub(crate) to: String,

    /// Source bucket (gcs, s3, s3-compatible).
    #[arg(long, default_value = "")]
    pub(crate) from_bucket: String,
    /// Destination bucket (gcs, s3, s3-compatible).
    #[arg(long, default_value = "")]
    pub(crate) to_bucket: String,
    /// Source storage account (azure).
    #[arg(long, default_value = "")]
    pub(crate) from_account: String,
    /// Destination storage account (azure).
    #[arg(long, default_value = "")]
    pub(crate) to_account: String,
    /// Source container (azure).
    #[arg(long, default_value = "")]
    pub(crate) from_container: String,
    /// Destination container (azure).
    #[arg(long, default_value = "")]
    pub(crate) to_container: String,
    /// Source root directory (local).
    #[arg(long, default_value = "")]
    pub(crate) from_path: String,
    /// Destination root directory (local).
    #[arg(long, default_value = "")]
    pub(crate) to_path: String,
    /// Source region (s3, s3-compatible); empty defers to the AWS default
    /// chain for s3 and is refused for s3-compatible.
    #[arg(long, default_value = "")]
    pub(crate) from_region: String,
    /// Destination region (s3, s3-compatible); empty defers to the AWS
    /// default chain for s3 and is refused for s3-compatible.
    #[arg(long, default_value = "")]
    pub(crate) to_region: String,
    /// Source S3 API endpoint URL (s3-compatible).
    #[arg(long, default_value = "")]
    pub(crate) from_endpoint: String,
    /// Destination S3 API endpoint URL (s3-compatible).
    #[arg(long, default_value = "")]
    pub(crate) to_endpoint: String,
    /// Source addressing, `path` or `virtual-hosted` (s3-compatible).
    #[arg(long, default_value = "")]
    pub(crate) from_addressing: String,
    /// Destination addressing, `path` or `virtual-hosted` (s3-compatible).
    #[arg(long, default_value = "")]
    pub(crate) to_addressing: String,
    /// Credential role whose item holds the source key pair in its
    /// access_key_id and secret_access_key fields (s3-compatible).
    #[arg(long, default_value = "")]
    pub(crate) from_credential_role: String,
    /// Credential role whose item holds the destination key pair in its
    /// access_key_id and secret_access_key fields (s3-compatible).
    #[arg(long, default_value = "")]
    pub(crate) to_credential_role: String,
}

impl EndpointArgs {
    pub(crate) fn source(&self) -> Endpoint {
        Endpoint {
            kind: self.from.clone(),
            bucket: self.from_bucket.clone(),
            account: self.from_account.clone(),
            container: self.from_container.clone(),
            region: self.from_region.clone(),
            path: self.from_path.clone(),
            endpoint: self.from_endpoint.clone(),
            addressing: self.from_addressing.clone(),
            credential_role: self.from_credential_role.clone(),
        }
    }

    pub(crate) fn destination(&self) -> Endpoint {
        Endpoint {
            kind: self.to.clone(),
            bucket: self.to_bucket.clone(),
            account: self.to_account.clone(),
            container: self.to_container.clone(),
            region: self.to_region.clone(),
            path: self.to_path.clone(),
            endpoint: self.to_endpoint.clone(),
            addressing: self.to_addressing.clone(),
            credential_role: self.to_credential_role.clone(),
        }
    }
}
