//! Multipart uploads through the S3 API, staged exactly where the object
//! API's own chunked uploads stage: `<key>.__stado_upload/<upload-id>/
//! <index>`, with the part number as the eight-digit index and index zero
//! holding the upload's media type and user metadata. The parts stay
//! invisible to listings, `stado storage abort-upload` discards an upload
//! a client abandoned, and completion writes the one object and removes
//! every part.

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::dashboard::listener::routes::object::OCTET_STREAM;
use crate::dashboard::listener::{empty_response, http_status, Dashboard, Request, Response};
use crate::queue::{Listing, UPLOAD_PART_MARKER};
use crate::remote::object_store::ObjectRef;
use crate::remote::sigv4::hex_sha256;

use super::auth::Signer;
use super::listing::S3_NAMESPACE;
use super::objects::{user_metadata, write_metadata};
use super::wire::{allow, body, document, element, elements, escape, invalid, object, storage};
use super::wire::{Query, S3Error, XML};

/// The part numbers the S3 API allows: 1 through 10,000.
const LAST_PART: u32 = 10_000;

/// What CreateMultipartUpload records for CompleteMultipartUpload to land.
#[derive(Serialize, Deserialize)]
struct Manifest {
    content_type: String,
    metadata: std::collections::BTreeMap<String, String>,
}

fn upload_prefix(object: &ObjectRef, upload: &str) -> String {
    format!("{}{UPLOAD_PART_MARKER}{upload}/", object.storage_path())
}

fn part_path(object: &ObjectRef, upload: &str, index: u32) -> String {
    format!("{}{index:08}", upload_prefix(object, upload))
}

fn no_upload(object: &ObjectRef, upload: &str) -> S3Error {
    S3Error::new(
        StatusCode::NOT_FOUND,
        "NoSuchUpload",
        format!("no multipart upload {upload} of {object} is in progress"),
    )
}

fn invalid_part(message: String) -> S3Error {
    S3Error::new(StatusCode::BAD_REQUEST, "InvalidPart", message)
}

/// A part number the S3 API allows.
fn part_number(raw: &str) -> Result<u32, S3Error> {
    let refused = || invalid(format!("partNumber {raw:?} is not 1 through {LAST_PART}"));
    let number = raw.trim().parse::<u32>().map_err(|_| refused())?;
    if (1..=LAST_PART).contains(&number) {
        Ok(number)
    } else {
        Err(refused())
    }
}

/// An upload id this endpoint issued: 64 lowercase hex digits.
fn upload_id(upload: &str) -> Result<&str, S3Error> {
    if upload.len() == 64
        && upload
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(upload)
    } else {
        Err(S3Error::new(
            StatusCode::NOT_FOUND,
            "NoSuchUpload",
            format!("{upload:?} is not an upload id this endpoint issued"),
        ))
    }
}

impl Dashboard {
    async fn s3_manifest(&self, object: &ObjectRef, upload: &str) -> Result<Manifest, S3Error> {
        let bytes = self
            .store
            .read_bytes(&part_path(object, upload, 0))
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
            .upload_bytes(&part_path(&object, &upload, 0), &encoded)
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

    /// UploadPart.
    pub(super) async fn s3_upload_part(
        &self,
        request: &Request,
        signer: &Signer,
        query: &Query,
        key: &str,
        upload: &str,
    ) -> Result<Response, S3Error> {
        allow(signer, key, "put")?;
        let object = object(signer, key)?;
        let upload = upload_id(upload)?;
        let part = match query.value("partNumber") {
            Some(raw) => part_number(raw)?,
            None => return Err(invalid("UploadPart needs partNumber")),
        };
        self.s3_manifest(&object, upload).await?;
        let data = body(request, signer)?;
        let _fence = self.storage_write_guard().map_err(storage)?;
        self.store
            .upload_bytes(&part_path(&object, upload, part), &data)
            .await
            .map_err(storage)?;
        Ok(Response::new_with_headers(
            http_status(StatusCode::OK),
            "OK",
            XML,
            b"",
            &[("ETag", format!("\"{}\"", hex_sha256(&data)))],
        ))
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
        let mut previous = 0;
        for part in elements(text, "Part") {
            let number = match element(part, "PartNumber").map_err(invalid)? {
                Some(raw) => part_number(&raw)?,
                None => return Err(invalid_part("a Part names no PartNumber".to_string())),
            };
            if number <= previous {
                return Err(S3Error::new(
                    StatusCode::BAD_REQUEST,
                    "InvalidPartOrder",
                    format!("part {number} follows part {previous}; list parts in ascending order"),
                ));
            }
            previous = number;
            let stated = element(part, "ETag")
                .map_err(invalid)?
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
        if previous == 0 {
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
        if self.s3_discard_parts(&object, upload).await? == 0 {
            return Err(no_upload(&object, upload));
        }
        Ok(empty_response(
            http_status(StatusCode::NO_CONTENT),
            "No Content",
        ))
    }

    /// Delete every staged part of one upload; how many there were.
    async fn s3_discard_parts(&self, object: &ObjectRef, upload: &str) -> Result<usize, S3Error> {
        let staged = self
            .store
            .list_paths(&upload_prefix(object, upload), Listing::Every)
            .await
            .map_err(storage)?;
        for path in &staged {
            self.store.delete_blob(path).await.map_err(storage)?;
        }
        Ok(staged.len())
    }
}
