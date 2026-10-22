//! The bucket-level reads: ListBuckets (the one namespace the signer holds),
//! HeadBucket, GetBucketLocation, and ListObjects and ListObjectsV2 over
//! exactly the prefixes the namespace's policy lets it list.
//!
//! A listing of a prefix no single `list` grant covers, such as the whole
//! bucket, is the union of every granted prefix under it, so `aws s3 ls`
//! of a bucket shows what the namespace may list and nothing else. Staged
//! multipart parts are not objects and are never listed. A listed entry
//! carries no ETag: the store's listing names sizes and dates, and reading
//! every body to version it would make a listing cost the bytes it lists.

use std::collections::BTreeSet;

use base64::Engine;
use reqwest::StatusCode;

use crate::dashboard::listener::{http_status, Dashboard, Response};
use crate::remote::object_store::ObjectRef;
use crate::remote::sigv4::uri_encode;

use super::auth::Signer;
use super::objects::iso_time;
use super::wire::{document, escape, invalid, storage, Query, S3Error, XML};

/// The XML namespace of every S3 API document.
pub(crate) const S3_NAMESPACE: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// How many keys a listing answers when the request names no `max-keys`,
/// and the most it answers when it names more: the S3 API's own bound.
const MAX_KEYS: usize = 1000;

pub(super) fn list_buckets(signer: &Signer) -> Response {
    document(
        StatusCode::OK,
        &format!(
            "<ListAllMyBucketsResult xmlns=\"{S3_NAMESPACE}\"><Buckets><Bucket><Name>{}</Name></Bucket></Buckets></ListAllMyBucketsResult>",
            escape(&signer.namespace)
        ),
    )
}

pub(super) fn head_bucket(signer: &Signer) -> Response {
    Response::headers_only(
        http_status(StatusCode::OK),
        "OK",
        XML,
        0,
        &[("x-amz-bucket-region", signer.scope.region.clone())],
    )
}

/// The region a bucket answers in is the one its request was signed for:
/// the endpoint serves one store wherever it runs.
pub(super) fn location(signer: &Signer) -> Response {
    document(
        StatusCode::OK,
        &format!(
            "<LocationConstraint xmlns=\"{S3_NAMESPACE}\">{}</LocationConstraint>",
            escape(&signer.scope.region)
        ),
    )
}

/// One listed object.
struct Entry {
    key: String,
    size: u64,
    updated: Option<chrono::DateTime<chrono::Utc>>,
}

/// Where a listing resumes: after the continuation token's key, after
/// `start-after` or `marker`, or, with none of them, at the first key.
fn cursor(query: &Query, version_two: bool) -> Result<String, S3Error> {
    let named = |name: &str| match query.value(name) {
        Some(value) => value.to_string(),
        None => String::new(),
    };
    match (version_two, query.value("continuation-token")) {
        (true, Some(token)) => {
            let foreign = |error: String| {
                invalid(format!(
                    "the continuation token is not one this endpoint issued: {error}"
                ))
            };
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(token)
                .map_err(|error| foreign(error.to_string()))?;
            String::from_utf8(bytes).map_err(|error| foreign(error.to_string()))
        }
        (true, None) => Ok(named("start-after")),
        (false, _) => Ok(named("marker")),
    }
}

impl Dashboard {
    /// Every object the signer may list under `prefix`, ascending by key.
    async fn s3_listable(&self, signer: &Signer, prefix: &str) -> Result<Vec<Entry>, S3Error> {
        let scans: Vec<String> = match signer.policy.authorized_list_prefix(prefix, "list") {
            Some(authorized) => vec![authorized],
            None => signer
                .policy
                .prefix_policies()
                .iter()
                .filter(|policy| {
                    policy.actions().iter().any(|action| action == "list")
                        && policy.prefix().ends_with('/')
                        && policy.prefix().starts_with(prefix)
                })
                .map(|policy| policy.prefix().to_string())
                .collect(),
        };
        if scans.is_empty() {
            return Err(S3Error::new(
                StatusCode::FORBIDDEN,
                "AccessDenied",
                format!(
                    "namespace {}'s grant lets it list no prefix under {prefix:?}; \
                     object_api.namespaces.{}.prefix_policies declares what it may list",
                    signer.namespace, signer.namespace
                ),
            ));
        }
        let mut entries = Vec::new();
        let mut seen = BTreeSet::new();
        for scan in scans {
            let storage_prefix = ObjectRef::namespace_prefix(&signer.namespace, &scan)
                .map_err(|error| invalid(error.to_string()))?;
            for blob in self
                .store
                .list_blobs_with_meta(&storage_prefix)
                .await
                .map_err(storage)?
            {
                let object = ObjectRef::from_storage_path(&blob.name).map_err(storage)?;
                let key = object.key().to_string();
                // A blob removed between the listing and its stat has no
                // size: it is no longer an object to list.
                let Some(size) = blob.size else {
                    continue;
                };
                if key.contains(crate::queue::UPLOAD_PART_MARKER)
                    || !key.starts_with(prefix)
                    || !seen.insert(key.clone())
                {
                    continue;
                }
                entries.push(Entry {
                    key,
                    size,
                    updated: blob.updated,
                });
            }
        }
        entries.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(entries)
    }

    /// ListObjects (v1) and ListObjectsV2 (`list-type=2`).
    pub(super) async fn s3_list_objects(
        &self,
        signer: &Signer,
        query: &Query,
    ) -> Result<Response, S3Error> {
        let version_two = match query.value("list-type") {
            None => false,
            Some("2") => true,
            Some(other) => return Err(invalid(format!("list-type {other:?} is neither 1 nor 2"))),
        };
        let prefix = match query.value("prefix") {
            Some(prefix) => prefix,
            None => "",
        };
        let delimiter = query
            .value("delimiter")
            .filter(|delimiter| !delimiter.is_empty());
        let url_encoded = match query.value("encoding-type") {
            None => false,
            Some("url") => true,
            Some(other) => return Err(invalid(format!("encoding-type {other:?} is not url"))),
        };
        let max_keys = match query.value("max-keys") {
            None => MAX_KEYS,
            Some(raw) => raw
                .parse::<usize>()
                .map_err(|error| invalid(format!("max-keys {raw:?} is not a count: {error}")))?
                .min(MAX_KEYS),
        };
        let cursor = cursor(query, version_two)?;
        let entries = self.s3_listable(signer, prefix).await?;
        let shown = |text: &str| {
            if url_encoded {
                uri_encode(text, true)
            } else {
                escape(text)
            }
        };
        let mut contents = String::new();
        let mut prefixes = BTreeSet::new();
        let mut count = 0;
        let mut last = None;
        let mut truncated = false;
        for entry in entries
            .iter()
            .filter(|entry| entry.key.as_str() > cursor.as_str())
        {
            let common = delimiter.and_then(|delimiter| {
                let rest = &entry.key[prefix.len()..];
                rest.find(delimiter)
                    .map(|at| entry.key[..prefix.len() + at + delimiter.len()].to_string())
            });
            if let Some(common) = &common {
                if cursor.starts_with(common.as_str()) || prefixes.contains(common) {
                    continue;
                }
            }
            if count == max_keys {
                truncated = true;
                break;
            }
            count += 1;
            match common {
                Some(common) => {
                    last = Some(common.clone());
                    prefixes.insert(common);
                }
                None => {
                    last = Some(entry.key.clone());
                    let modified = match entry.updated {
                        Some(updated) => {
                            format!("<LastModified>{}</LastModified>", iso_time(updated))
                        }
                        None => String::new(),
                    };
                    contents.push_str(&format!(
                        "<Contents><Key>{}</Key>{modified}<Size>{}</Size></Contents>",
                        shown(&entry.key),
                        entry.size
                    ));
                }
            }
        }
        let common: String = prefixes
            .iter()
            .map(|common| {
                format!(
                    "<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>",
                    shown(common)
                )
            })
            .collect();
        let mut head = format!(
            "<Name>{}</Name><Prefix>{}</Prefix><MaxKeys>{max_keys}</MaxKeys><IsTruncated>{truncated}</IsTruncated>",
            escape(&signer.namespace),
            shown(prefix)
        );
        if let Some(delimiter) = delimiter {
            head.push_str(&format!("<Delimiter>{}</Delimiter>", shown(delimiter)));
        }
        if url_encoded {
            head.push_str("<EncodingType>url</EncodingType>");
        }
        let next = last.filter(|_| truncated);
        if version_two {
            head.push_str(&format!("<KeyCount>{count}</KeyCount>"));
            if let Some(token) = query.value("continuation-token") {
                head.push_str(&format!(
                    "<ContinuationToken>{}</ContinuationToken>",
                    escape(token)
                ));
            }
            if let Some(start) = query.value("start-after") {
                head.push_str(&format!("<StartAfter>{}</StartAfter>", shown(start)));
            }
            if let Some(next) = next {
                let token = base64::engine::general_purpose::STANDARD.encode(next.as_bytes());
                head.push_str(&format!(
                    "<NextContinuationToken>{token}</NextContinuationToken>"
                ));
            }
        } else {
            head.push_str(&format!("<Marker>{}</Marker>", shown(&cursor)));
            if let Some(next) = next {
                head.push_str(&format!("<NextMarker>{}</NextMarker>", shown(&next)));
            }
        }
        Ok(document(
            StatusCode::OK,
            &format!(
                "<ListBucketResult xmlns=\"{S3_NAMESPACE}\">{head}{contents}{common}</ListBucketResult>"
            ),
        ))
    }
}
