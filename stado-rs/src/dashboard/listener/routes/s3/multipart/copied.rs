//! UploadPartCopy: a part of an upload in progress taken from an object the
//! signer may read, whole or the one byte range `x-amz-copy-source-range`
//! names, server-side. The AWS CLI copies an object above its multipart
//! threshold this way (`aws s3 cp s3://bucket/a s3://bucket/b`), one ranged
//! request per part, so each part reads only its range from the store
//! (`BlobBackend::read_range`): an N-part copy reads the source once in
//! total, and no request holds more than its part.

use reqwest::StatusCode;

use crate::dashboard::listener::http::parse_byte_range;
use crate::dashboard::listener::{Dashboard, Request, Response};
use crate::remote::object_store::ObjectRef;

use super::super::auth::Signer;
use super::super::listing::S3_NAMESPACE;
use super::super::objects::iso_time;
use super::super::wire::{document, storage, Query, S3Error};
use super::staged::part_path;

/// The first and last byte `range` (`bytes=first-last`) names of a source
/// of `size` bytes, refused as `InvalidRange` unless both ends are stated
/// and lie within it.
fn ranged(range: &str, size: u64) -> Result<(u64, u64), S3Error> {
    let unsatisfiable = |cause: String| {
        S3Error::new(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "InvalidRange",
            format!(
                "x-amz-copy-source-range {range:?} is not bytes=<first>-<last> within the \
                 {size}-byte source: {cause}"
            ),
        )
    };
    let length = usize::try_from(size).map_err(|error| unsatisfiable(error.to_string()))?;
    let (first, last) = parse_byte_range(range, length)
        .ok_or_else(|| unsatisfiable("it is no one satisfiable range".to_string()))?;
    // parse_byte_range shortens a range that runs past the end, as a read
    // may; a copy takes only the bytes it was told to, so the stated last
    // byte must be the one found.
    let stated = range.rsplit_once('-').map(|(_, last)| last.trim());
    if stated != Some(last.to_string().as_str()) {
        return Err(unsatisfiable(
            "its last byte is past the source's end".to_string(),
        ));
    }
    let wide =
        |index: usize| u64::try_from(index).map_err(|error| unsatisfiable(error.to_string()));
    Ok((wide(first)?, wide(last)?))
}

/// The source was removed between its stat and the read of its bytes.
fn vanished(source: &ObjectRef) -> S3Error {
    S3Error::new(
        StatusCode::NOT_FOUND,
        "NoSuchKey",
        format!("the copy source {source} was removed while it was copied"),
    )
}

/// The source changed between its stat and the read of its bytes.
fn changed(source: &ObjectRef, cause: String) -> S3Error {
    S3Error::new(
        StatusCode::CONFLICT,
        "OperationAborted",
        format!("the copy source {source} changed while it was read: {cause}; repeat the request"),
    )
}

impl Dashboard {
    /// Bytes `first` through `last` of the copy source at `path`, read as
    /// that range alone, refused when the store answers another length.
    async fn s3_source_range(
        &self,
        source: &ObjectRef,
        path: &str,
        (first, last): (u64, u64),
    ) -> Result<Vec<u8>, S3Error> {
        let bytes = self
            .store
            .backend()
            .read_range(path, first, last)
            .await
            .map_err(storage)?
            .ok_or_else(|| vanished(source))?;
        // The range holds its first and last bytes and every one between:
        // https://www.rfc-editor.org/rfc/rfc9110#section-14.1.2
        let wanted = last - first + 1;
        let read =
            u64::try_from(bytes.len()).map_err(|error| changed(source, error.to_string()))?;
        if read != wanted {
            return Err(changed(
                source,
                format!("{read} of the {wanted} bytes came back"),
            ));
        }
        Ok(bytes)
    }

    /// UploadPartCopy.
    pub(in super::super) async fn s3_upload_part_copy(
        &self,
        request: &Request,
        signer: &Signer,
        query: &Query,
        key: &str,
        upload: &str,
    ) -> Result<Response, S3Error> {
        let (object, upload, part) = self.s3_part_of(signer, query, key, upload).await?;
        let source = self
            .s3_copy_source(request, signer, "UploadPartCopy")
            .await?;
        let from = source.object.storage_path();
        let data = match (request.header("x-amz-copy-source-range"), source.read) {
            (Some(range), _) => {
                let bounds = ranged(range, source.size)?;
                self.s3_source_range(&source.object, &from, bounds).await?
            }
            (None, Some(bytes)) => bytes,
            (None, None) => self
                .store
                .read_bytes(&from)
                .await
                .map_err(storage)?
                .ok_or_else(|| vanished(&source.object))?,
        };
        let etag = self.s3_stage_part(&object, upload, part, &data).await?;
        let path = part_path(&object, upload, part);
        let modified = match self.s3_stat(&path).await?.and_then(|staged| staged.updated) {
            Some(updated) => format!("<LastModified>{}</LastModified>", iso_time(updated)),
            None => String::new(),
        };
        Ok(document(
            StatusCode::OK,
            &format!(
                "<CopyPartResult xmlns=\"{S3_NAMESPACE}\"><ETag>&quot;{etag}&quot;</ETag>{modified}</CopyPartResult>"
            ),
        ))
    }
}
