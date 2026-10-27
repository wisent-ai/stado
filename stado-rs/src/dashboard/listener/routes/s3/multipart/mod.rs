//! Multipart uploads through the S3 API, staged exactly where the object
//! API's own chunked uploads stage ([`staged`]): under
//! `<key>.__stado_upload/<upload-id>/`, the manifest holding the upload's
//! media type and user metadata beside each part at its eight-digit number,
//! each part carrying the SHA-256 its ETag is. A part comes from the
//! request's body (UploadPart) or from an object the signer may read
//! (UploadPartCopy, [`copied`]). The parts stay invisible to object
//! listings; ListMultipartUploads and ListParts read them ([`listed`]),
//! `stado storage abort` discards an upload a client abandoned, and
//! completion writes the one object and removes every part.

mod copied;
mod listed;
mod staged;

use std::collections::BTreeMap;

use reqwest::StatusCode;

use crate::dashboard::listener::routes::object::OCTET_STREAM;
use crate::dashboard::listener::{empty_response, http_status, Dashboard, Request, Response};
use crate::queue::Listing;
use crate::remote::object_store::ObjectRef;
use crate::remote::sigv4::hex_sha256;

use super::auth::Signer;
use super::listing::S3_NAMESPACE;
use super::objects::{user_metadata, write_metadata};
use super::wire::{
    allow, body, document, escape, invalid, object, read, storage, CompleteDocument,
};
use super::wire::{Query, S3Error, XML};
use staged::{invalid_part, manifest_path, no_upload, part_number, part_path, upload_id};
use staged::{upload_prefix, Manifest, PART_SHA256};

impl Dashboard {
    async fn s3_manifest(&self, object: &ObjectRef, upload: &str) -> Result<Manifest, S3Error> {
        let bytes = self
            .store
            .read_bytes(&manifest_path(object, upload))
            .await
            .map_err(storage)?
            .ok_or_else(|| no_upload(object, upload))?;
        serde_json::from_slice(&bytes).map_err(|error| {
            S3Error::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                format!("the manifest of upload {upload} of {object} does not parse: {error}"),
            )
        })
    }

    /// CreateMultipartUpload.
    pub(super) async fn s3_create_upload(
        &self,
        request: &Request,
        signer: &Signer,
        key: &str,
    ) -> Result<Response, S3Error> {
        allow(signer, key, "put")?;
        let object = object(signer, key)?;
        let manifest = Manifest {
            content_type: match request.header("content-type") {
                Some(declared) => declared.to_string(),
                None => OCTET_STREAM.to_string(),
            },
            metadata: user_metadata(request),
        };
        write_metadata(&object, &manifest.content_type, &manifest.metadata)?;
        let upload = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let encoded = serde_json::to_vec(&manifest).map_err(|error| {
            S3Error::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalError",
                error.to_string(),
            )
        })?;
        let _fence = self.storage_write_guard().map_err(storage)?;
        self.store
            .upload_bytes(&manifest_path(&object, &upload), &encoded)
            .await
            .map_err(storage)?;
        Ok(document(
            StatusCode::OK,
            &format!(
                "<InitiateMultipartUploadResult xmlns=\"{S3_NAMESPACE}\"><Bucket>{}</Bucket><Key>{}</Key><UploadId>{upload}</UploadId></InitiateMultipartUploadResult>",
                escape(&signer.namespace),
                escape(key)
            ),
        ))
    }

    /// UploadPart: the part's bytes from the request's body.
    pub(super) async fn s3_upload_part(
        &self,
        request: &Request,
        signer: &Signer,
        query: &Query,
        key: &str,
        upload: &str,
    ) -> Result<Response, S3Error> {
        let (object, upload, part) = self.s3_part_of(signer, query, key, upload).await?;
        let data = body(request, signer)?;
        let etag = self.s3_stage_part(&object, upload, part, &data).await?;
        Ok(Response::new_with_headers(
            http_status(StatusCode::OK),
            "OK",
            XML,
            b"",
            &[("ETag", format!("\"{etag}\""))],
        ))
    }

    /// The upload in progress and the part number an UploadPart or
    /// UploadPartCopy writes, once the signer may `put` the key.
    async fn s3_part_of<'a>(
        &self,
        signer: &Signer,
        query: &Query,
        key: &str,
        upload: &'a str,
    ) -> Result<(ObjectRef, &'a str, u32), S3Error> {
        allow(signer, key, "put")?;
        let object = object(signer, key)?;
        let upload = upload_id(upload)?;
        let part = match query.value("partNumber") {
            Some(raw) => part_number(raw)?,
            None => return Err(invalid("UploadPart needs partNumber")),
        };
        self.s3_manifest(&object, upload).await?;
        Ok((object, upload, part))
    }

    /// Stage `data` as `part` of `upload`, then the SHA-256 its ETag is,
    /// which ListParts reads without reading the part; answers the ETag.
    async fn s3_stage_part(
        &self,
        object: &ObjectRef,
        upload: &str,
        part: u32,
        data: &[u8],
    ) -> Result<String, S3Error> {
        let etag = hex_sha256(data);
        let path = part_path(object, upload, part);
        let _fence = self.storage_write_guard().map_err(storage)?;
        self.store
            .upload_bytes(&path, data)
            .await
            .map_err(storage)?;
        let described = BTreeMap::from([(PART_SHA256.to_string(), etag.clone())]);
        self.store
            .backend()
            .set_metadata(&path, &described)
            .await
            .map_err(storage)?;
        Ok(etag)
    }

    /// CompleteMultipartUpload: the named parts, in ascending order and each
    /// at the ETag its UploadPart answered, composed into the object.
    pub(super) async fn s3_complete_upload(
        &self,
        request: &Request,
        signer: &Signer,
        key: &str,
        upload: &str,
    ) -> Result<Response, S3Error> {
        allow(signer, key, "put")?;
        let object = object(signer, key)?;
        let upload = upload_id(upload)?;
        let manifest = self.s3_manifest(&object, upload).await?;
        let data = body(request, signer)?;
        let text = std::str::from_utf8(&data).map_err(|error| {
            invalid(format!(
                "the CompleteMultipartUpload document is not UTF-8: {error}"
            ))
        })?;
        let mut composed = Vec::new();
        let mut previous: Option<u32> = None;
        let asked: CompleteDocument = read(text, "CompleteMultipartUpload").map_err(invalid)?;
        for part in asked.parts {
            let number = match &part.number {
                Some(raw) => part_number(raw)?,
                None => return Err(invalid_part("a Part names no PartNumber".to_string())),
            };
            if let Some(previous) = previous.filter(|previous| number <= *previous) {
                return Err(S3Error::new(
                    StatusCode::BAD_REQUEST,
                    "InvalidPartOrder",
                    format!("part {number} follows part {previous}; list parts in ascending order"),
                ));
            }
            previous = Some(number);
            let stated = part
                .etag
                .ok_or_else(|| invalid_part(format!("part {number} names no ETag")))?;
            let bytes = self
                .store
                .read_bytes(&part_path(&object, upload, number))
                .await
                .map_err(storage)?
                .ok_or_else(|| {
                    invalid_part(format!(
                        "part {number} of upload {upload} was never uploaded"
                    ))
                })?;
            let held = hex_sha256(&bytes);
            if stated.trim().trim_matches('"') != held {
                return Err(invalid_part(format!(
                    "part {number} is held at ETag {held}, the request names {stated}"
                )));
            }
            composed.extend_from_slice(&bytes);
        }
        if previous.is_none() {
            return Err(invalid_part("the request names no part".to_string()));
        }
        let metadata = write_metadata(&object, &manifest.content_type, &manifest.metadata)?;
        let path = object.storage_path();
        let _fence = self.storage_write_guard().map_err(storage)?;
        self.store
            .upload_bytes(&path, &composed)
            .await
            .map_err(storage)?;
        self.store
            .backend()
            .set_metadata(&path, &metadata)
            .await
            .map_err(storage)?;
        self.s3_discard_parts(&object, upload).await?;
        let etag = self.s3_etag(&path, &composed).await?;
        Ok(document(
            StatusCode::OK,
            &format!(
                "<CompleteMultipartUploadResult xmlns=\"{S3_NAMESPACE}\"><Bucket>{}</Bucket><Key>{}</Key><ETag>&quot;{etag}&quot;</ETag></CompleteMultipartUploadResult>",
                escape(&signer.namespace),
                escape(key)
            ),
        ))
    }

    /// AbortMultipartUpload.
    pub(super) async fn s3_abort_upload(
        &self,
        signer: &Signer,
        key: &str,
        upload: &str,
    ) -> Result<Response, S3Error> {
        allow(signer, key, "delete")?;
        let object = object(signer, key)?;
        let upload = upload_id(upload)?;
        let _fence = self.storage_write_guard().map_err(storage)?;
        if self.s3_discard_parts(&object, upload).await?.is_empty() {
            return Err(no_upload(&object, upload));
        }
        Ok(empty_response(
            http_status(StatusCode::NO_CONTENT),
            "No Content",
        ))
    }

    /// Delete everything staged for one upload; the paths that were there.
    async fn s3_discard_parts(
        &self,
        object: &ObjectRef,
        upload: &str,
    ) -> Result<Vec<String>, S3Error> {
        let staged = self
            .store
            .list_paths(&upload_prefix(object, upload), Listing::Every)
            .await
            .map_err(storage)?;
        for path in &staged {
            self.store.delete_blob(path).await.map_err(storage)?;
        }
        Ok(staged)
    }
}
