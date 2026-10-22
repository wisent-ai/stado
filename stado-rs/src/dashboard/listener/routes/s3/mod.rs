//! Stado as its own S3 provider: the S3 API served at `/s3` on the object
//! API listener, over the same store `/api/object` serves.
//!
//! A bucket is a namespace `object_api.namespaces` declares, and its access
//! key id is that namespace; the secret is the namespace's bearer
//! ([`auth`]). Objects are the namespace's `stado://<namespace>/<key>`
//! objects, so an object written through either door is read through the
//! other, and every request is authorized against the same per-prefix
//! action policy (`get`, `stat`, `list`, `put`, `delete`). Addressing is by
//! path: `/s3/<bucket>/<key>`.
//!
//! Served: ListBuckets, HeadBucket, GetBucketLocation, GetBucketVersioning
//! (never enabled: objects keep no versions), ListObjects and
//! ListObjectsV2 ([`listing`]); GetObject, HeadObject, PutObject with
//! `If-None-Match: *` and `If-Match`, CopyObject, DeleteObject and
//! DeleteObjects ([`objects`]); CreateMultipartUpload, UploadPart,
//! CompleteMultipartUpload and AbortMultipartUpload ([`multipart`]). Any
//! other operation is answered `NotImplemented` naming what was asked.

mod auth;
mod listing;
mod multipart;
mod objects;
mod wire;

use reqwest::StatusCode;

use crate::dashboard::listener::{Boundary, Dashboard, Request, Response};
use crate::remote::sigv4::S3_ROOT;

use auth::Signer;
use wire::{document, method, not_implemented, path_decode, Query, S3Error};

/// Whether `path` is the S3 endpoint's: its root, or a path below it.
pub(crate) fn is_s3_route(path: &str) -> bool {
    match path.strip_prefix(S3_ROOT) {
        Some(rest) => rest.is_empty() || rest.starts_with('/'),
        None => false,
    }
}

/// GetBucketVersioning: Stado keeps no object versions, so every bucket
/// answers as one whose versioning was never enabled, the empty
/// configuration S3 answers for such a bucket. Turning it on is refused as
/// not implemented.
fn versioning() -> Response {
    document(
        StatusCode::OK,
        &format!(
            "<VersioningConfiguration xmlns=\"{}\"/>",
            listing::S3_NAMESPACE
        ),
    )
}

impl Dashboard {
    /// Answer one S3 request. Errors are S3 error documents for the
    /// request's own path; a `HEAD` answer carries none.
    pub(crate) async fn s3(&self, request: &Request) -> Response {
        let (raw_path, raw_query) = request.target();
        let head = request.method == "HEAD";
        match self.s3_route(request, raw_path, raw_query).await {
            Ok(response) => response,
            Err(error) => {
                eprintln!(
                    "[dashboard] s3 {} {raw_path} refused: {} {}",
                    request.method, error.code, error.message
                );
                error.response(raw_path, head)
            }
        }
    }

    async fn s3_route(
        &self,
        request: &Request,
        raw_path: &str,
        raw_query: &str,
    ) -> Result<Response, S3Error> {
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return Err(S3Error::new(
                StatusCode::FORBIDDEN,
                "AccessDenied",
                "the Host header names no origin this listener answers for",
            ));
        }
        let query = Query::parse(raw_query)?;
        if !self.boundaries_available(&[Boundary::Object]).await {
            return Err(S3Error::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailable",
                format!(
                    "object authorization is unavailable: {}",
                    self.closed_boundary(&[Boundary::Object]).cause
                ),
            ));
        }
        let signer = self.s3_signer(request, raw_path, &query.pairs).await?;
        let rest = raw_path[S3_ROOT.len()..].trim_start_matches('/');
        let (bucket, key) = match rest.split_once('/') {
            Some((bucket, key)) => (path_decode(bucket)?, path_decode(key)?),
            None => (path_decode(rest)?, String::new()),
        };
        if bucket.is_empty() {
            query.only_subresource(None)?;
            return match request.method.as_str() {
                "GET" => Ok(listing::list_buckets(&signer)),
                other => Err(method(other, "the service")),
            };
        }
        if bucket != signer.namespace {
            return Err(S3Error::new(
                StatusCode::FORBIDDEN,
                "AccessDenied",
                format!(
                    "the access key id names namespace {}, and bucket {bucket:?} is another; each \
                     namespace is signed for with its own secret",
                    signer.namespace
                ),
            ));
        }
        if key.is_empty() {
            return self.s3_bucket(request, &signer, &query).await;
        }
        self.s3_object(request, &signer, &query, &key).await
    }

    async fn s3_bucket(
        &self,
        request: &Request,
        signer: &Signer,
        query: &Query,
    ) -> Result<Response, S3Error> {
        match request.method.as_str() {
            "HEAD" => {
                query.only_subresource(None)?;
                Ok(listing::head_bucket(signer))
            }
            "GET" if query.has("location") => {
                query.only_subresource(Some("location"))?;
                Ok(listing::location(signer))
            }
            "GET" if query.has("versioning") => {
                query.only_subresource(Some("versioning"))?;
                Ok(versioning())
            }
            "GET" => {
                query.only_subresource(None)?;
                self.s3_list_objects(signer, query).await
            }
            "POST" if query.has("delete") => {
                query.only_subresource(Some("delete"))?;
                self.s3_delete_objects(request, signer).await
            }
            // A PUT with a sub-resource (?versioning, ?acl, ?policy, …) sets
            // that configuration, which is not served; only a bare PUT is a
            // CreateBucket.
            "PUT" => {
                query.only_subresource(None)?;
                Err(S3Error::new(
                    StatusCode::CONFLICT,
                    "BucketAlreadyOwnedByYou",
                    format!(
                        "bucket {} is namespace {} and exists; buckets are the namespaces \
                         object_api.namespaces declares, never created through the S3 API",
                        signer.namespace, signer.namespace
                    ),
                ))
            }
            other => Err(method(other, "a bucket")),
        }
    }

    async fn s3_object(
        &self,
        request: &Request,
        signer: &Signer,
        query: &Query,
        key: &str,
    ) -> Result<Response, S3Error> {
        if query.has("versionId") {
            return Err(not_implemented(
                "Stado objects keep no versions; address the object without versionId",
            ));
        }
        let upload = query.value("uploadId");
        match (request.method.as_str(), upload) {
            ("GET" | "HEAD", Some(_)) => Err(not_implemented(
                "ListParts is not served: a multipart upload is completed or aborted by its id",
            )),
            ("GET" | "HEAD", None) => {
                query.only_subresource(None)?;
                self.s3_get_object(request, signer, query, key).await
            }
            ("PUT", Some(upload)) => {
                query.only_subresource(None)?;
                self.s3_upload_part(request, signer, query, key, upload)
                    .await
            }
            ("PUT", None) if request.header("x-amz-copy-source").is_some() => {
                query.only_subresource(None)?;
                self.s3_copy_object(request, signer, key).await
            }
            ("PUT", None) => {
                query.only_subresource(None)?;
                self.s3_put_object(request, signer, key).await
            }
            ("POST", None) if query.has("uploads") => {
                query.only_subresource(Some("uploads"))?;
                self.s3_create_upload(request, signer, key).await
            }
            ("POST", Some(upload)) => {
                query.only_subresource(None)?;
                self.s3_complete_upload(request, signer, key, upload).await
            }
            ("DELETE", Some(upload)) => {
                query.only_subresource(None)?;
                self.s3_abort_upload(signer, key, upload).await
            }
            ("DELETE", None) => {
                query.only_subresource(None)?;
                self.s3_delete_object(signer, key).await
            }
            (other, _) => Err(method(other, "an object")),
        }
    }
}
