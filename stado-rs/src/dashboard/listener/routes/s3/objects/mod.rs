//! One object through the S3 API: the read ([`Dashboard::s3_get_object`],
//! GetObject and HeadObject), the writes (`write`), the copy (`copy`), and
//! what the store holds about it in S3 terms (`stored`).

mod copy;
mod stored;
mod write;

pub(crate) use stored::{iso_time, user_metadata};
pub(crate) use write::write_metadata;

use reqwest::StatusCode;

use crate::dashboard::listener::http::parse_byte_range;
use crate::dashboard::listener::routes::object::OCTET_STREAM;
use crate::dashboard::listener::{http_status, Dashboard, Request, Response};

use super::auth::Signer;
use super::wire::{allow, invalid, not_implemented, object, storage, Query, S3Error};
use stored::{http_date, metadata_headers, Preconditions};

/// The `response-*` query parameters of a GetObject and the header each one
/// overrides in the answer.
const RESPONSE_OVERRIDES: [(&str, &str); 5] = [
    ("response-content-disposition", "Content-Disposition"),
    ("response-cache-control", "Cache-Control"),
    ("response-content-encoding", "Content-Encoding"),
    ("response-content-language", "Content-Language"),
    ("response-expires", "Expires"),
];

impl Dashboard {
    /// GetObject and HeadObject: the bytes, or only their headers, with the
    /// ETag, the last modification, the user metadata, a byte range when one
    /// is asked, and the preconditions the request sets.
    pub(super) async fn s3_get_object(
        &self,
        request: &Request,
        signer: &Signer,
        query: &Query,
        key: &str,
    ) -> Result<Response, S3Error> {
        let head = request.method == "HEAD";
        allow(signer, key, if head { "stat" } else { "get" })?;
        if query.has("partNumber") {
            return Err(not_implemented(
                "reading one part of a completed upload is not served; Stado keeps the \
                 composed object only",
            ));
        }
        let object = object(signer, key)?;
        let path = object.storage_path();
        let missing = || {
            S3Error::new(
                StatusCode::NOT_FOUND,
                "NoSuchKey",
                format!("{object} is absent from namespace {}", signer.namespace),
            )
        };
        let bytes = self
            .store
            .read_bytes(&path)
            .await
            .map_err(storage)?
            .ok_or_else(missing)?;
        let blob = self.s3_stat(&path).await?.ok_or_else(missing)?;
        let etag = self.s3_etag(&path, &bytes).await?;
        Preconditions::read(request, "if-")?.check(&etag, blob.updated, true)?;
        let content_type = match query.value("response-content-type") {
            Some(overridden) => overridden.to_string(),
            None => match blob.metadata.get("content-type") {
                Some(stored) => stored.clone(),
                None => OCTET_STREAM.to_string(),
            },
        };
        let mut headers: Vec<(String, String)> = vec![
            ("ETag".to_string(), format!("\"{etag}\"")),
            ("Accept-Ranges".to_string(), "bytes".to_string()),
        ];
        if let Some(updated) = blob.updated {
            headers.push(("Last-Modified".to_string(), http_date(updated)));
        }
        headers.extend(metadata_headers(&blob));
        for (parameter, header) in RESPONSE_OVERRIDES {
            if let Some(value) = query.value(parameter) {
                if value.chars().any(char::is_control) {
                    return Err(invalid(format!(
                        "{parameter} holds a control character a header cannot carry"
                    )));
                }
                headers.push((header.to_string(), value.to_string()));
            }
        }
        let (status, reason, body) = match request.header("range") {
            None => (StatusCode::OK, "OK", &bytes[..]),
            Some(range) => {
                let Some((start, end)) = parse_byte_range(range, bytes.len()) else {
                    return Err(S3Error::new(
                        StatusCode::RANGE_NOT_SATISFIABLE,
                        "InvalidRange",
                        format!(
                            "the range {range:?} is not one satisfiable range of the {}-byte \
                             object",
                            bytes.len()
                        ),
                    ));
                };
                headers.push((
                    "Content-Range".to_string(),
                    format!("bytes {start}-{end}/{}", bytes.len()),
                ));
                (
                    StatusCode::PARTIAL_CONTENT,
                    "Partial Content",
                    &bytes[start..=end],
                )
            }
        };
        let headers: Vec<(&str, String)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.clone()))
            .collect();
        if head {
            return Ok(Response::headers_only(
                http_status(status),
                reason,
                &content_type,
                body.len(),
                &headers,
            ));
        }
        Ok(Response::new_with_headers(
            http_status(status),
            reason,
            &content_type,
            body,
            &headers,
        ))
    }
}
