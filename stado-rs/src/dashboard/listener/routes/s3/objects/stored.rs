//! What the store holds about one object, in the S3 API's terms: its
//! listing entry, its ETag, its user metadata, its dates, and the
//! preconditions a client may set on them.
//!
//! The ETag is the store's own compare-and-swap version for an object whose
//! bytes are text, so `If-Match` on a write compares exactly what the store
//! compares; for a binary object, which no store versions, it is the
//! SHA-256 of the bytes. User metadata travels as `x-amz-meta-<name>` on
//! the wire and is kept as `stado-s3-meta-<name>`, inside the `stado-*`
//! metadata space the object API already reserves for custom fields.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use reqwest::StatusCode;

use crate::dashboard::listener::{Dashboard, Request};
use crate::queue::BlobInfo;
use crate::remote::sigv4::hex_sha256;

use super::super::wire::{invalid, storage, S3Error};

const WIRE_META: &str = "x-amz-meta-";
const STORED_META: &str = "stado-s3-meta-";

/// An RFC 9110 HTTP date, what `Last-Modified` carries.
pub(crate) fn http_date(at: DateTime<Utc>) -> String {
    at.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// An ISO 8601 time with milliseconds, what S3 XML documents carry.
pub(crate) fn iso_time(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// The request's `x-amz-meta-*` headers as stored metadata fields.
pub(crate) fn user_metadata(request: &Request) -> BTreeMap<String, String> {
    request
        .headers
        .iter()
        .filter_map(|(name, value)| {
            let field = name.strip_prefix(WIRE_META)?;
            Some((format!("{STORED_META}{field}"), value.clone()))
        })
        .collect()
}

/// The stored user metadata of `blob`, kept by its stored names.
pub(crate) fn stored_user_metadata(blob: &BlobInfo) -> BTreeMap<String, String> {
    blob.metadata
        .iter()
        .filter(|(name, _)| name.starts_with(STORED_META))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// The `x-amz-meta-*` headers `blob` answers with.
pub(crate) fn metadata_headers(blob: &BlobInfo) -> Vec<(String, String)> {
    blob.metadata
        .iter()
        .filter_map(|(name, value)| {
            let field = name.strip_prefix(STORED_META)?;
            Some((format!("{WIRE_META}{field}"), value.clone()))
        })
        .collect()
}

/// The preconditions one request sets on an object, read from the request
/// headers under `prefix` (`if-` on a read, `x-amz-copy-source-if-` on a
/// copy source).
pub(crate) struct Preconditions {
    matching: Option<String>,
    not_matching: Option<String>,
    modified_since: Option<DateTime<Utc>>,
    unmodified_since: Option<DateTime<Utc>>,
}

fn http_date_header(request: &Request, name: &str) -> Result<Option<DateTime<Utc>>, S3Error> {
    match request.header(name) {
        None => Ok(None),
        Some(raw) => DateTime::parse_from_rfc2822(raw)
            .map(|at| Some(at.with_timezone(&Utc)))
            .map_err(|error| invalid(format!("{name} {raw:?} is not an HTTP date: {error}"))),
    }
}

fn unquoted(value: &str) -> String {
    value.trim().trim_matches('"').to_string()
}

impl Preconditions {
    pub(crate) fn read(request: &Request, prefix: &str) -> Result<Self, S3Error> {
        Ok(Self {
            matching: request.header(&format!("{prefix}match")).map(unquoted),
            not_matching: request.header(&format!("{prefix}none-match")).map(unquoted),
            modified_since: http_date_header(request, &format!("{prefix}modified-since"))?,
            unmodified_since: http_date_header(request, &format!("{prefix}unmodified-since"))?,
        })
    }

    /// Whether checking these needs the object's ETag: an `if-match` or an
    /// `if-none-match` is stated. Only then is a copy source read to version
    /// it before the copy.
    pub(crate) fn names_etag(&self) -> bool {
        self.matching.is_some() || self.not_matching.is_some()
    }

    /// Whether the object at `etag`, last modified at `updated`, passes.
    /// A failed `If-None-Match` or `If-Modified-Since` on a read is answered
    /// 304, every other failure 412.
    pub(crate) fn check(
        &self,
        etag: &str,
        updated: Option<DateTime<Utc>>,
        read: bool,
    ) -> Result<(), S3Error> {
        let failed = |what: String| {
            Err(S3Error::new(
                StatusCode::PRECONDITION_FAILED,
                "PreconditionFailed",
                what,
            ))
        };
        if let Some(expected) = &self.matching {
            if expected != "*" && expected != etag {
                return failed(format!(
                    "the object's ETag is {etag}, the request requires {expected}"
                ));
            }
        }
        if let (Some(since), Some(updated)) = (self.unmodified_since, updated) {
            if updated > since {
                return failed(format!(
                    "the object changed at {}, after {}",
                    http_date(updated),
                    http_date(since)
                ));
            }
        }
        let unchanged = |what: String| {
            if read {
                Err(S3Error::new(StatusCode::NOT_MODIFIED, "NotModified", what))
            } else {
                failed(what)
            }
        };
        if let Some(refused) = &self.not_matching {
            if refused == "*" || refused == etag {
                return unchanged(format!("the object's ETag is {etag}"));
            }
        }
        if let (Some(since), Some(updated)) = (self.modified_since, updated) {
            if updated <= since {
                return unchanged(format!(
                    "the object has not changed since {}",
                    http_date(since)
                ));
            }
        }
        Ok(())
    }
}

impl Dashboard {
    /// The store's listing entry for exactly `path`, or `None`.
    pub(crate) async fn s3_stat(&self, path: &str) -> Result<Option<BlobInfo>, S3Error> {
        Ok(self
            .store
            .list_blobs_with_meta(path)
            .await
            .map_err(storage)?
            .into_iter()
            .find(|blob| blob.name == path))
    }

    /// The ETag of the object at `path` holding `bytes`, unquoted.
    pub(crate) async fn s3_etag(&self, path: &str, bytes: &[u8]) -> Result<String, S3Error> {
        if std::str::from_utf8(bytes).is_err() {
            return Ok(hex_sha256(bytes));
        }
        match self
            .store
            .read_text_versioned(path)
            .await
            .map_err(storage)?
        {
            Some(versioned) => Ok(versioned.version),
            None => Err(S3Error::new(
                StatusCode::CONFLICT,
                "OperationAborted",
                format!("{path} was removed while it was read; repeat the request"),
            )),
        }
    }
}
