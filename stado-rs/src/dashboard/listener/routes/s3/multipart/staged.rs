//! Where one multipart upload is staged and what names it: its manifest and
//! each part under `<key>.__stado_upload/<upload-id>/`, the metadata each
//! carries, and the upload ids and part numbers the S3 API admits.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::queue::UPLOAD_PART_MARKER;
use crate::remote::object_store::ObjectRef;

use super::super::wire::{invalid, S3Error};

// The highest part number UploadPart admits, as the S3 API defines it:
// https://docs.aws.amazon.com/AmazonS3/latest/userguide/qfacts.html
pub(super) const LAST_PART: u32 = 10_000;

/// The name the upload's manifest is staged under, beside its parts, which
/// are named by their numbers.
pub(super) const MANIFEST: &str = "manifest";

/// The metadata field each staged part carries: the SHA-256 of its bytes,
/// the ETag UploadPart answered. The object API's own chunks carry the same
/// field, so a part is described alike whichever door staged it.
pub(super) const PART_SHA256: &str = "stado-upload-sha256";

/// What CreateMultipartUpload records for CompleteMultipartUpload to land.
#[derive(Serialize, Deserialize)]
pub(super) struct Manifest {
    pub(super) content_type: String,
    pub(super) metadata: BTreeMap<String, String>,
}

/// The staged parts' common prefix in the store.
pub(super) fn upload_prefix(object: &ObjectRef, upload: &str) -> String {
    format!("{}{UPLOAD_PART_MARKER}{upload}/", object.storage_path())
}

/// Where the upload's manifest is staged.
pub(super) fn manifest_path(object: &ObjectRef, upload: &str) -> String {
    format!("{}{MANIFEST}", upload_prefix(object, upload))
}

/// Where part `number` is staged, in the form the object API's chunked
/// uploads use too.
pub(super) fn part_path(object: &ObjectRef, upload: &str, number: u32) -> String {
    format!(
        "{}{}",
        upload_prefix(object, upload),
        crate::queue::upload_part_name(number)
    )
}

pub(super) fn no_upload(object: &ObjectRef, upload: &str) -> S3Error {
    S3Error::new(
        StatusCode::NOT_FOUND,
        "NoSuchUpload",
        format!("no multipart upload {upload} of {object} is in progress"),
    )
}

pub(super) fn invalid_part(message: String) -> S3Error {
    S3Error::new(StatusCode::BAD_REQUEST, "InvalidPart", message)
}

/// A part number the S3 API allows: one through [`LAST_PART`].
pub(super) fn part_number(raw: &str) -> Result<u32, S3Error> {
    let refused = |cause: String| {
        invalid(format!(
            "partNumber {raw:?} is not 1 through {LAST_PART}: {cause}"
        ))
    };
    let number = raw
        .trim()
        .parse::<NonZeroU32>()
        .map_err(|error| refused(error.to_string()))?;
    if number.get() <= LAST_PART {
        Ok(number.get())
    } else {
        Err(refused(format!("it is above {LAST_PART}")))
    }
}

/// An upload id this endpoint issued: two UUIDs' lowercase hex digits, as
/// CreateMultipartUpload writes it.
pub(super) fn upload_id(upload: &str) -> Result<&str, S3Error> {
    let nil = uuid::Uuid::nil().simple();
    let issued = format!("{nil}{nil}").len();
    if upload.len() == issued
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

/// The part number a staged path ends with; `None` for the manifest. A path
/// under the upload that ends in neither is refused by name.
pub(super) fn staged_part(path: &str) -> Result<Option<u32>, S3Error> {
    let unnamed = |cause: String| {
        S3Error::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            format!("{path} is staged under a multipart upload as neither its manifest nor a part: {cause}"),
        )
    };
    let (_, name) = path
        .rsplit_once('/')
        .ok_or_else(|| unnamed("it names no folder".to_string()))?;
    if name == MANIFEST {
        return Ok(None);
    }
    name.parse::<u32>()
        .map(Some)
        .map_err(|error| unnamed(error.to_string()))
}
