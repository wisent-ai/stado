//! The constructor, the bucket every request is bound to, and the two SDK
//! answers decoded back into crate values.
//!
//! The SDK client is itself the sender the sibling components leave through,
//! so what is written here is its construction and the wire vocabulary it
//! answers in: the quoted ETag that becomes the version token once unquoted,
//! and the SDK timestamp behind every last-modified field.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::queue::StorageError;

use super::{Inner, S3Backend};

impl S3Backend {
    /// Build a backend for `bucket` using the AWS adapter host's IMDSv2
    /// identity. Empty `bucket` is the Python RuntimeError
    /// ("WC_S3_BUCKET is required for S3 storage"). Empty `region` lets the SDK
    /// choose its default region.
    pub async fn new(bucket: &str, region: &str) -> Result<Self, StorageError> {
        if bucket.is_empty() {
            return Err(StorageError::Other(
                "WC_S3_BUCKET is required for S3 storage".into(),
            ));
        }
        let shared = crate::providers::aws::sdk_config(region)
            .await
            .map_err(|err| StorageError::Other(err.to_string()))?;
        Ok(Self::assemble(aws_sdk_s3::Client::new(&shared), bucket))
    }

    /// Assemble from an explicit client (tests bind a loopback endpoint).
    fn assemble(client: aws_sdk_s3::Client, bucket: &str) -> Self {
        Self {
            inner: Arc::new(Inner {
                client,
                bucket: bucket.to_string(),
            }),
        }
    }

    /// The bucket this backend is bound to (Python `self.bucket`).
    pub fn bucket(&self) -> &str {
        &self.inner.bucket
    }
}

/// Strip the surrounding quotes from an S3 ETag (Python `.strip('"')`).
/// S3 ETags always arrive quoted; the CAS version token is unquoted.
pub(super) fn unquote_etag(etag: &str) -> &str {
    etag.trim_matches('"')
}

/// Convert an SDK timestamp to chrono (nanosecond precision preserved).
pub(super) fn to_utc(dt: &aws_sdk_s3::primitives::DateTime) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(dt.secs(), dt.subsec_nanos())
}
