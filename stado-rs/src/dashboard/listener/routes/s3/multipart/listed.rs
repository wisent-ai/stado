//! The uploads in progress and their parts: ListMultipartUploads over the
//! prefixes the namespace may list, ListParts of one upload its signer may
//! write. Both read what the store holds under the staging marker, so an
//! upload a client abandoned is found by the client, `aws s3api
//! list-multipart-uploads` or rclone's cleanup, and aborted by its id.
//! The object API's own chunked uploads, staged under the same marker with no
//! manifest, are not S3 uploads and are never listed.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use reqwest::StatusCode;

use crate::dashboard::listener::{Dashboard, Response};
use crate::queue::UPLOAD_PART_MARKER;
use crate::remote::object_store::ObjectRef;
use crate::remote::sigv4::uri_encode;

use super::super::auth::Signer;
use super::super::listing::S3_NAMESPACE;
use super::super::objects::iso_time;
use super::super::scans::{granted_scans, page_bound};
use super::super::wire::{allow, document, escape, invalid, not_implemented, object, storage};
use super::super::wire::{Query, S3Error};
use super::staged::{staged_part, upload_id, upload_prefix, MANIFEST, PART_SHA256};

/// One upload in progress: its object's key, its id and when it began.
struct Upload {
    key: String,
    id: String,
    initiated: Option<DateTime<Utc>>,
}

/// One staged part of an upload.
struct Part {
    number: u32,
    etag: String,
    size: u64,
    updated: Option<DateTime<Utc>>,
}

/// The key and upload id a staged manifest at `key` belongs to; none for a
/// staged object that is no S3 upload's manifest.
fn manifest_of(key: &str) -> Option<(String, String)> {
    let (target, staged) = key.split_once(UPLOAD_PART_MARKER)?;
    let (upload, name) = staged.split_once('/')?;
    (name == MANIFEST).then(|| (target.to_string(), upload.to_string()))
}

fn named(query: &Query, name: &str) -> String {
    match query.value(name) {
        Some(value) => value.to_string(),
        None => String::new(),
    }
}

impl Dashboard {
    /// ListMultipartUploads: every upload in progress under `prefix` the
    /// namespace may list, ascending by key and upload id.
    pub(in super::super) async fn s3_list_uploads(
        &self,
        signer: &Signer,
        query: &Query,
    ) -> Result<Response, S3Error> {
        if query
            .value("delimiter")
            .is_some_and(|delimiter| !delimiter.is_empty())
        {
            return Err(not_implemented(
                "ListMultipartUploads with a delimiter is not served; list uploads by prefix",
            ));
        }
        let url_encoded = match query.value("encoding-type") {
            None => false,
            Some("url") => true,
            Some(other) => return Err(invalid(format!("encoding-type {other:?} is not url"))),
        };
        let shown = |text: &str| match url_encoded {
            true => uri_encode(text, true),
            false => escape(text),
        };
        let prefix = named(query, "prefix");
        let key_marker = named(query, "key-marker");
        let id_marker = named(query, "upload-id-marker");
        let page = page_bound(query, "max-uploads")?;
        let mut seen = BTreeSet::new();
        let mut uploads = Vec::new();
        for storage_prefix in granted_scans(signer, &prefix)? {
            let staged = self
                .store
                .list_blobs_sized(&storage_prefix)
                .await
                .map_err(storage)?;
            for blob in staged {
                let staged = ObjectRef::from_storage_path(&blob.name).map_err(storage)?;
                let Some((key, id)) = manifest_of(staged.key()) else {
                    continue;
                };
                if !key.starts_with(&prefix) || !seen.insert((key.clone(), id.clone())) {
                    continue;
                }
                uploads.push(Upload {
                    key,
                    id,
                    initiated: blob.updated,
                });
            }
        }
        uploads.sort_by(|left, right| (&left.key, &left.id).cmp(&(&right.key, &right.id)));
        let after = |upload: &&Upload| {
            upload.key > key_marker
                || (upload.key == key_marker && !id_marker.is_empty() && upload.id > id_marker)
        };
        let remaining: Vec<&Upload> = uploads.iter().filter(after).collect();
        let truncated = remaining.len() > page;
        let listed = &remaining[..remaining.len().min(page)];
        let mut body = format!(
            "<Bucket>{}</Bucket><KeyMarker>{}</KeyMarker><UploadIdMarker>{}</UploadIdMarker><Prefix>{}</Prefix><MaxUploads>{page}</MaxUploads><IsTruncated>{truncated}</IsTruncated>",
            escape(&signer.namespace),
            shown(&key_marker),
            escape(&id_marker),
            shown(&prefix)
        );
        if url_encoded {
            body.push_str("<EncodingType>url</EncodingType>");
        }
        if let Some(last) = listed.last().filter(|_| truncated) {
            body.push_str(&format!(
                "<NextKeyMarker>{}</NextKeyMarker><NextUploadIdMarker>{}</NextUploadIdMarker>",
                shown(&last.key),
                last.id
            ));
        }
        for upload in listed {
            let initiated = match upload.initiated {
                Some(at) => format!("<Initiated>{}</Initiated>", iso_time(at)),
                None => String::new(),
            };
            body.push_str(&format!(
                "<Upload><Key>{}</Key><UploadId>{}</UploadId>{initiated}<StorageClass>STANDARD</StorageClass></Upload>",
                shown(&upload.key),
                upload.id
            ));
        }
        Ok(document(
            StatusCode::OK,
            &format!("<ListMultipartUploadsResult xmlns=\"{S3_NAMESPACE}\">{body}</ListMultipartUploadsResult>"),
        ))
    }

    /// ListParts: the parts of one upload in progress, by part number, each
    /// with the ETag its UploadPart answered.
    pub(in super::super) async fn s3_list_parts(
        &self,
        signer: &Signer,
        query: &Query,
        key: &str,
        upload: &str,
    ) -> Result<Response, S3Error> {
        allow(signer, key, "put")?;
        let object = object(signer, key)?;
        let upload = upload_id(upload)?;
        self.s3_manifest(&object, upload).await?;
        let marker = match query.value("part-number-marker") {
            None => None,
            Some(raw) => Some(raw.trim().parse::<u32>().map_err(|error| {
                invalid(format!(
                    "part-number-marker {raw:?} is not a part number: {error}"
                ))
            })?),
        };
        let page = page_bound(query, "max-parts")?;
        let mut parts = Vec::new();
        let staged = upload_prefix(&object, upload);
        for blob in self
            .store
            .list_blobs_with_meta(&staged)
            .await
            .map_err(storage)?
        {
            let Some(number) = staged_part(&blob.name)? else {
                continue;
            };
            // A part removed between the listing and its stat has no size.
            let Some(size) = blob.size else { continue };
            let etag = blob.metadata.get(PART_SHA256).cloned().ok_or_else(|| {
                S3Error::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "InternalError",
                    format!(
                        "part {number} of upload {upload} of {object} records no {PART_SHA256}"
                    ),
                )
            })?;
            parts.push(Part {
                number,
                etag,
                size,
                updated: blob.updated,
            });
        }
        parts.sort_by_key(|part| part.number);
        let remaining: Vec<&Part> = parts
            .iter()
            .filter(|part| marker.is_none_or(|marker| part.number > marker))
            .collect();
        let truncated = remaining.len() > page;
        let listed = &remaining[..remaining.len().min(page)];
        let mut body = format!(
            "<Bucket>{}</Bucket><Key>{}</Key><UploadId>{upload}</UploadId><MaxParts>{page}</MaxParts><IsTruncated>{truncated}</IsTruncated>",
            escape(&signer.namespace),
            escape(key)
        );
        if let Some(marker) = marker {
            body.push_str(&format!("<PartNumberMarker>{marker}</PartNumberMarker>"));
        }
        if let Some(last) = listed.last().filter(|_| truncated) {
            body.push_str(&format!(
                "<NextPartNumberMarker>{}</NextPartNumberMarker>",
                last.number
            ));
        }
        for part in listed {
            let modified = match part.updated {
                Some(at) => format!("<LastModified>{}</LastModified>", iso_time(at)),
                None => String::new(),
            };
            body.push_str(&format!(
                "<Part><PartNumber>{}</PartNumber>{modified}<ETag>&quot;{}&quot;</ETag><Size>{}</Size></Part>",
                part.number, part.etag, part.size
            ));
        }
        Ok(document(
            StatusCode::OK,
            &format!("<ListPartsResult xmlns=\"{S3_NAMESPACE}\">{body}</ListPartsResult>"),
        ))
    }
}
