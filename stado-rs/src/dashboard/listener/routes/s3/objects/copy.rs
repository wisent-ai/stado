//! CopyObject: one object of the signer's namespace written at another key
//! of it, server-side, with the source's metadata (`COPY`) or the
//! request's (`REPLACE`), and the source preconditions the request sets.
//! A copy across namespaces would need two secrets in one signature, so the
//! source is always in the bucket the request is signed for. The source is
//! found and checked by [`Dashboard::s3_copy_source`], which UploadPartCopy
//! asks too: it stats the source and reads its bytes only when a stated
//! ETag precondition needs them, so a part copied from one range of a large
//! object never reads the rest.

use reqwest::StatusCode;

use crate::dashboard::listener::routes::object::OCTET_STREAM;
use crate::dashboard::listener::{Dashboard, Request, Response};
use crate::queue::BlobInfo;
use crate::remote::object_store::ObjectRef;

use super::super::auth::Signer;
use super::super::listing::S3_NAMESPACE;
use super::super::wire::{
    allow, document, invalid, not_implemented, object, path_decode, storage, S3Error,
};
use super::stored::{iso_time, stored_user_metadata, user_metadata, Preconditions};
use super::write::write_metadata;

/// A copy source whose preconditions held: the object, its stat and size,
/// and its bytes when checking an ETag precondition read them.
pub(crate) struct CopySource {
    pub(crate) object: ObjectRef,
    pub(crate) blob: BlobInfo,
    pub(crate) size: u64,
    pub(crate) read: Option<Vec<u8>>,
}

fn absent(object: &ObjectRef) -> S3Error {
    S3Error::new(
        StatusCode::NOT_FOUND,
        "NoSuchKey",
        format!("the copy source {object} is absent"),
    )
}

/// The object `x-amz-copy-source` names, in the signer's namespace and
/// readable by it. `what` names the operation in a refusal.
fn named_source(request: &Request, signer: &Signer, what: &str) -> Result<ObjectRef, S3Error> {
    let source = match request.header("x-amz-copy-source") {
        Some(source) => source,
        None => return Err(invalid(format!("{what} needs x-amz-copy-source"))),
    };
    if source.contains('?') {
        return Err(not_implemented(
            "a copy source with a query (versionId) is not served: Stado objects keep no versions",
        ));
    }
    let source = path_decode(source.trim_start_matches('/'))?;
    let (bucket, source_key) = source.split_once('/').ok_or_else(|| {
        invalid(format!(
            "x-amz-copy-source {source:?} is not <bucket>/<key>"
        ))
    })?;
    if bucket != signer.namespace {
        return Err(S3Error::new(
            StatusCode::FORBIDDEN,
            "AccessDenied",
            format!(
                "the copy source is in bucket {bucket:?} and the request is signed for \
                 namespace {}; copy within one namespace",
                signer.namespace
            ),
        ));
    }
    allow(signer, source_key, "get")?;
    object(signer, source_key)
}

impl Dashboard {
    /// The copy source after the request's `x-amz-copy-source-if-*`
    /// preconditions held. Its bytes are read only to version it for an
    /// `if-match` or `if-none-match`; the dates are checked on its stat.
    pub(crate) async fn s3_copy_source(
        &self,
        request: &Request,
        signer: &Signer,
        what: &str,
    ) -> Result<CopySource, S3Error> {
        let from = named_source(request, signer, what)?;
        let path = from.storage_path();
        let blob = self.s3_stat(&path).await?.ok_or_else(|| absent(&from))?;
        // A source removed between its listing and its stat has no size.
        let size = blob.size.ok_or_else(|| absent(&from))?;
        let preconditions = Preconditions::read(request, "x-amz-copy-source-if-")?;
        let (etag, read) = if preconditions.names_etag() {
            let bytes = self
                .store
                .read_bytes(&path)
                .await
                .map_err(storage)?
                .ok_or_else(|| absent(&from))?;
            (self.s3_etag(&path, &bytes).await?, Some(bytes))
        } else {
            // No ETag is compared, so none is computed.
            (String::new(), None)
        };
        preconditions.check(&etag, blob.updated, false)?;
        Ok(CopySource {
            object: from,
            blob,
            size,
            read,
        })
    }

    pub(crate) async fn s3_copy_object(
        &self,
        request: &Request,
        signer: &Signer,
        key: &str,
    ) -> Result<Response, S3Error> {
        allow(signer, key, "put")?;
        let to = object(signer, key)?;
        let CopySource {
            object: from,
            blob,
            read,
            ..
        } = self.s3_copy_source(request, signer, "CopyObject").await?;
        let replace = match request.header("x-amz-metadata-directive") {
            None | Some("COPY") => false,
            Some("REPLACE") => true,
            Some(other) => {
                return Err(invalid(format!(
                    "x-amz-metadata-directive {other:?} is neither COPY nor REPLACE"
                )))
            }
        };
        if from == to && !replace {
            return Err(S3Error::new(
                StatusCode::BAD_REQUEST,
                "InvalidRequest",
                "this copy request is illegal because it copies an object to itself without \
                 changing its metadata; send x-amz-metadata-directive: REPLACE",
            ));
        }
        let bytes = match read {
            Some(bytes) => bytes,
            None => self
                .store
                .read_bytes(&from.storage_path())
                .await
                .map_err(storage)?
                .ok_or_else(|| absent(&from))?,
        };
        let (content_type, user) = if replace {
            let content_type = match request.header("content-type") {
                Some(declared) => declared.to_string(),
                None => OCTET_STREAM.to_string(),
            };
            (content_type, user_metadata(request))
        } else {
            let content_type = match blob.metadata.get("content-type") {
                Some(stored) => stored.clone(),
                None => OCTET_STREAM.to_string(),
            };
            (content_type, stored_user_metadata(&blob))
        };
        let metadata = write_metadata(&to, &content_type, &user)?;
        let to_path = to.storage_path();
        let _fence = self.storage_write_guard().map_err(storage)?;
        self.store
            .upload_bytes(&to_path, &bytes)
            .await
            .map_err(storage)?;
        self.store
            .backend()
            .set_metadata(&to_path, &metadata)
            .await
            .map_err(storage)?;
        let etag = self.s3_etag(&to_path, &bytes).await?;
        let modified = match self
            .s3_stat(&to_path)
            .await?
            .and_then(|landed| landed.updated)
        {
            Some(updated) => format!("<LastModified>{}</LastModified>", iso_time(updated)),
            None => String::new(),
        };
        Ok(document(
            StatusCode::OK,
            &format!(
                "<CopyObjectResult xmlns=\"{S3_NAMESPACE}\"><ETag>&quot;{etag}&quot;</ETag>{modified}</CopyObjectResult>"
            ),
        ))
    }
}
