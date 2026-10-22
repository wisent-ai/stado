//! CopyObject: one object of the signer's namespace written at another key
//! of it, server-side, with the source's metadata (`COPY`) or the
//! request's (`REPLACE`), and the source preconditions the request sets.
//! A copy across namespaces would need two secrets in one signature, so the
//! source is always in the bucket the request is signed for.

use reqwest::StatusCode;

use crate::dashboard::listener::routes::object::OCTET_STREAM;
use crate::dashboard::listener::{Dashboard, Request, Response};

use super::super::auth::Signer;
use super::super::listing::S3_NAMESPACE;
use super::super::wire::{
    allow, document, invalid, not_implemented, object, path_decode, storage, S3Error,
};
use super::stored::{iso_time, stored_user_metadata, user_metadata, Preconditions};
use super::write::write_metadata;

impl Dashboard {
    pub(crate) async fn s3_copy_object(
        &self,
        request: &Request,
        signer: &Signer,
        key: &str,
    ) -> Result<Response, S3Error> {
        let source = match request.header("x-amz-copy-source") {
            Some(source) => source,
            None => return Err(invalid("CopyObject needs x-amz-copy-source")),
        };
        if source.contains('?') {
            return Err(not_implemented(
                "a copy source with a query (versionId) is not served: Stado objects keep no \
                 versions",
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
        allow(signer, key, "put")?;
        let from = object(signer, source_key)?;
        let to = object(signer, key)?;
        let from_path = from.storage_path();
        let missing = || {
            S3Error::new(
                StatusCode::NOT_FOUND,
                "NoSuchKey",
                format!("the copy source {from} is absent"),
            )
        };
        let bytes = self
            .store
            .read_bytes(&from_path)
            .await
            .map_err(storage)?
            .ok_or_else(missing)?;
        let blob = self.s3_stat(&from_path).await?.ok_or_else(missing)?;
        let source_etag = self.s3_etag(&from_path, &bytes).await?;
        Preconditions::read(request, "x-amz-copy-source-if-")?.check(
            &source_etag,
            blob.updated,
            false,
        )?;
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
