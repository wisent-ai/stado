//! An S3 API endpoint other than Amazon's, named by URL: Cloudflare R2,
//! Backblaze B2, Wasabi, MinIO, DigitalOcean Spaces, Hetzner, Scaleway,
//! OVHcloud, IDrive e2, Tigris, Ceph, Garage, and Stado's own `/s3`
//! endpoint. Every object operation is the S3 backend's own (`objects`);
//! what differs is only how the client reaches the endpoint: its URL, the
//! region it signs for, where the bucket goes, and a static key pair read
//! from the credential store by role rather than an AWS identity.
//!
//! The SDK computes request checksums only where the operation requires one
//! and validates response checksums only where it does: several S3 API
//! providers refuse the trailing checksums the SDK otherwise sends on every
//! upload.

use crate::queue::StorageError;

use super::S3Backend;

/// Where the bucket name goes in the request: the path (what MinIO, Ceph and
/// Stado's own endpoint serve) or the host name (what most hosted providers
/// prefer). The configuration names one of these two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Addressing {
    Path,
    VirtualHosted,
}

/// The credential item fields the key pair is read from.
const ACCESS_KEY_FIELD: &str = "access_key_id";
const SECRET_KEY_FIELD: &str = "secret_access_key";

/// Everything one `s3-compatible` store, or one named S3 API provider's, is
/// reached through. A named provider (`vendor`) builds its endpoint, region
/// and addressing from its profile and reads `account` and `region` only
/// where the profile asks; the generic store reads `endpoint`, `region` and
/// `addressing` as declared.
pub struct S3CompatibleLocator<'a> {
    pub vendor: Option<super::StorageVendor>,
    pub endpoint: &'a str,
    pub bucket: &'a str,
    pub region: &'a str,
    pub account: &'a str,
    pub addressing: &'a str,
    pub credential_role: &'a str,
}

/// The binding of one `s3-compatible` field, named for a refusal.
fn binding(key: &str) -> String {
    match crate::capabilities::config_field(
        crate::capabilities::RuntimeFacet::Storage,
        crate::capabilities::StorageAdapter::S3Compatible.id(),
        key,
    ) {
        Some(field) => format!("{} ({})", field.env, field.path),
        None => key.to_string(),
    }
}

fn required<'a>(value: &'a str, key: &str, backend: &str) -> Result<&'a str, StorageError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(StorageError::Other(format!(
            "the {backend} storage backend needs {}; nothing declares it",
            binding(key)
        )));
    }
    Ok(value)
}

async fn credential_field(role: &str, field: &str) -> Result<String, StorageError> {
    match crate::credential_store::read_string(role, field).await {
        Ok(Some(value)) if !value.trim().is_empty() => Ok(value.trim().to_string()),
        Ok(_) => Err(StorageError::Other(format!(
            "the credential playing role {role} holds no {field}; the s3-compatible backend \
             reads its key pair from the {ACCESS_KEY_FIELD} and {SECRET_KEY_FIELD} fields"
        ))),
        Err(error) => Err(StorageError::Other(format!(
            "reading {field} of the credential playing role {role} failed: {error}"
        ))),
    }
}

impl S3Backend {
    /// Build a backend on an S3-compatible endpoint: a named provider's, or
    /// one the configuration names. The endpoint must be an absolute web URL,
    /// and a generic store's addressing must be `path` or `virtual-hosted`;
    /// each refusal names the variable and configuration key that declares
    /// the missing or wrong value.
    pub async fn compatible(locator: S3CompatibleLocator<'_>) -> Result<Self, StorageError> {
        let backend = match locator.vendor {
            Some(vendor) => vendor.id(),
            None => crate::capabilities::StorageAdapter::S3Compatible.id(),
        };
        let bucket = required(locator.bucket, "bucket", backend)?;
        let role = required(locator.credential_role, "credential-role", backend)?;
        let (endpoint, region, path_style) = match locator.vendor {
            Some(vendor) => {
                let reached = super::vendor::resolve(
                    vendor,
                    locator.endpoint,
                    locator.account,
                    locator.region,
                )
                .map_err(StorageError::Other)?;
                (reached.endpoint, reached.region, reached.path_style)
            }
            None => {
                let endpoint = required(locator.endpoint, "endpoint", backend)?;
                let region = required(locator.region, "region", backend)?;
                let addressing = required(locator.addressing, "addressing", backend)?;
                let addressing = <Addressing as serde::Deserialize>::deserialize(
                    serde_json::Value::String(addressing.to_string()),
                )
                .map_err(|error| {
                    StorageError::Other(format!(
                        "{} is {addressing:?}; use \"path\" (the bucket in the path) or \
                         \"virtual-hosted\" (the bucket in the host name): {error}",
                        binding("addressing")
                    ))
                })?;
                (
                    endpoint.to_string(),
                    region.to_string(),
                    addressing == Addressing::Path,
                )
            }
        };
        let parsed = url::Url::parse(&endpoint).map_err(|error| {
            StorageError::Other(format!(
                "{} {endpoint:?} is not a URL: {error}",
                binding("endpoint")
            ))
        })?;
        if !crate::primitives::http::origin::is_web(&parsed) || parsed.host_str().is_none() {
            return Err(StorageError::Other(format!(
                "{} {endpoint:?} must be an absolute http:// or https:// URL",
                binding("endpoint")
            )));
        }
        let access_key = credential_field(role, ACCESS_KEY_FIELD).await?;
        let secret_key = credential_field(role, SECRET_KEY_FIELD).await?;
        let credentials = aws_sdk_s3::config::Credentials::new(
            access_key,
            secret_key,
            None,
            None,
            "stado-credential-store",
        );
        let store = format!("the {backend} bucket {bucket} at {endpoint}");
        let config = aws_sdk_s3::config::Builder::new()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(region))
            .endpoint_url(endpoint)
            .force_path_style(path_style)
            .credentials_provider(credentials)
            .request_checksum_calculation(
                aws_sdk_s3::config::RequestChecksumCalculation::WhenRequired,
            )
            .response_checksum_validation(
                aws_sdk_s3::config::ResponseChecksumValidation::WhenRequired,
            )
            .build();
        Ok(Self::assemble(
            aws_sdk_s3::Client::from_conf(config),
            bucket,
            super::Conditional::Proved {
                store,
                outcome: tokio::sync::OnceCell::new(),
            },
        ))
    }
}
