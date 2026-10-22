//! The object writes through the S3 API: PutObject, unconditional, create
//! only (`If-None-Match: *`) or compare-and-swap (`If-Match`), DeleteObject
//! and DeleteObjects. Each takes the local store's write fence the object
//! API's own writes take, and lands the metadata the object API lands, so a
//! `stado storage get` of the object answers what the S3 write stored.

use std::collections::BTreeMap;

use reqwest::StatusCode;
use tokio::io::AsyncWriteExt;

use crate::dashboard::listener::routes::object::{merged_object_metadata, OCTET_STREAM};
use crate::dashboard::listener::{empty_response, http_status, Dashboard, Request, Response};
use crate::queue::StorageError;
use crate::remote::object_store::ObjectRef;

use super::super::auth::Signer;
use super::super::wire::{
    allow, body, document, element, elements, escape, invalid, not_implemented, object, storage,
    S3Error, XML,
};
use super::stored::user_metadata;

/// The metadata one write lands: the object API's own fields for the
/// object and its media type, and the request's user metadata.
pub(crate) fn write_metadata(
    object: &ObjectRef,
    content_type: &str,
    user: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, S3Error> {
    merged_object_metadata(object, content_type, user).map_err(|refusal| {
        invalid(format!(
            "{refusal}: every x-amz-meta-* header needs a non-empty value without control \
             characters"
        ))
    })
}

impl Dashboard {
    /// PutObject.
    pub(crate) async fn s3_put_object(
        &self,
        request: &Request,
        signer: &Signer,
        key: &str,
    ) -> Result<Response, S3Error> {
        allow(signer, key, "put")?;
        let object = object(signer, key)?;
        let data = body(request, signer)?;
        let content_type = match request.header("content-type") {
            Some(declared) => declared.to_string(),
            None => OCTET_STREAM.to_string(),
        };
        let metadata = write_metadata(&object, &content_type, &user_metadata(request))?;
        let path = object.storage_path();
        let _fence = self.storage_write_guard().map_err(storage)?;
        let etag = match (request.header("if-none-match"), request.header("if-match")) {
            (Some(_), Some(_)) => {
                return Err(invalid(
                    "If-None-Match and If-Match name two different writes; send one",
                ))
            }
            (Some("*"), None) => {
                let staged = tempfile::NamedTempFile::new()
                    .map_err(|error| storage(StorageError::Io(error)))?;
                let mut file = tokio::fs::File::create(staged.path())
                    .await
                    .map_err(|error| storage(StorageError::Io(error)))?;
                file.write_all(&data)
                    .await
                    .map_err(|error| storage(StorageError::Io(error)))?;
                file.flush()
                    .await
                    .map_err(|error| storage(StorageError::Io(error)))?;
                let created = self
                    .store
                    .upload_file_if_absent(&path, staged.path())
                    .await
                    .map_err(storage)?;
                if !created {
                    return Err(S3Error::new(
                        StatusCode::PRECONDITION_FAILED,
                        "PreconditionFailed",
                        format!(
                            "{object} exists, and the write was create-only (If-None-Match: *)"
                        ),
                    ));
                }
                None
            }
            (Some(other), None) => {
                return Err(not_implemented(format!(
                    "If-None-Match {other:?} on a write is not served; only * (create only) is"
                )))
            }
            (None, Some(expected)) => {
                let expected = expected.trim().trim_matches('"');
                let text = std::str::from_utf8(&data).map_err(|_| {
                    not_implemented(
                        "a conditional replace of a binary object is not served: the store \
                         versions text objects only",
                    )
                })?;
                match self
                    .store
                    .compare_and_swap_text(&path, expected, text)
                    .await
                {
                    Ok(version) => Some(version),
                    Err(StorageError::StorageConflict(_)) => {
                        return Err(S3Error::new(
                            StatusCode::PRECONDITION_FAILED,
                            "PreconditionFailed",
                            format!("{object} is no longer at ETag {expected}"),
                        ))
                    }
                    Err(StorageError::NotFound(_)) => {
                        return Err(S3Error::new(
                            StatusCode::NOT_FOUND,
                            "NoSuchKey",
                            format!("{object} is absent, so it is at no ETag"),
                        ))
                    }
                    Err(error) => return Err(storage(error)),
                }
            }
            (None, None) => {
                self.store
                    .upload_bytes(&path, &data)
                    .await
                    .map_err(storage)?;
                None
            }
        };
        self.store
            .backend()
            .set_metadata(&path, &metadata)
            .await
            .map_err(storage)?;
        let etag = match etag {
            Some(version) => version,
            None => self.s3_etag(&path, &data).await?,
        };
        Ok(Response::new_with_headers(
            http_status(StatusCode::OK),
            "OK",
            XML,
            b"",
            &[("ETag", format!("\"{etag}\""))],
        ))
    }

    /// DeleteObject. Deleting an absent object succeeds, as S3's does.
    pub(crate) async fn s3_delete_object(
        &self,
        signer: &Signer,
        key: &str,
    ) -> Result<Response, S3Error> {
        allow(signer, key, "delete")?;
        let object = object(signer, key)?;
        let _fence = self.storage_write_guard().map_err(storage)?;
        self.store
            .delete_blob(&object.storage_path())
            .await
            .map_err(storage)?;
        Ok(empty_response(
            http_status(StatusCode::NO_CONTENT),
            "No Content",
        ))
    }

    /// DeleteObjects: every key of the `Delete` document deleted on its own
    /// authority, each refusal reported beside the deletions that held.
    pub(crate) async fn s3_delete_objects(
        &self,
        request: &Request,
        signer: &Signer,
    ) -> Result<Response, S3Error> {
        let data = body(request, signer)?;
        let document_text = std::str::from_utf8(&data)
            .map_err(|error| invalid(format!("the Delete document is not UTF-8: {error}")))?;
        let quiet = element(document_text, "Quiet")
            .map_err(invalid)?
            .is_some_and(|value| value == "true");
        let mut deleted = String::new();
        let mut failed = String::new();
        let _fence = self.storage_write_guard().map_err(storage)?;
        for entry in elements(document_text, "Object") {
            let key = element(entry, "Key")
                .map_err(invalid)?
                .ok_or_else(|| invalid("an Object of the Delete document names no Key"))?;
            let outcome = match allow(signer, &key, "delete").and_then(|()| object(signer, &key)) {
                Ok(object) => self
                    .store
                    .delete_blob(&object.storage_path())
                    .await
                    .map_err(storage),
                Err(error) => Err(error),
            };
            match outcome {
                Ok(()) if quiet => {}
                Ok(()) => {
                    deleted.push_str(&format!("<Deleted><Key>{}</Key></Deleted>", escape(&key)))
                }
                Err(error) => failed.push_str(&format!(
                    "<Error><Key>{}</Key><Code>{}</Code><Message>{}</Message></Error>",
                    escape(&key),
                    error.code,
                    escape(&error.message)
                )),
            }
        }
        Ok(document(
            StatusCode::OK,
            &format!(
                "<DeleteResult xmlns=\"{}\">{deleted}{failed}</DeleteResult>",
                super::super::listing::S3_NAMESPACE
            ),
        ))
    }
}
