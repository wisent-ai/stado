//! One byte range of one object, read from the store as that range alone.
//!
//! A ranged GET used to read the whole object into memory and answer a
//! slice of it, so a client reading a large object one chunk at a time made
//! the store read the whole object once per chunk: a 1.6 GB input in 3 MiB
//! ranges was read more than five hundred times over. The range is now
//! asked of the store by its size ([`crate::queue::BlobBackend::stat`]) and
//! read with [`crate::queue::BlobBackend::read_range`], so each range costs
//! its own bytes.

use serde_json::json;

use crate::dashboard::listener::http::parse_byte_range;
use crate::dashboard::listener::{http_status, send_json, Dashboard, Response};
use crate::dashboard::DashboardError;

use super::write::OCTET_STREAM;

impl Dashboard {
    /// The answer to a GET of `range` (the request's `Range` value) of the
    /// object `uri` stored at `path`.
    pub(super) async fn object_range(
        &self,
        uri: &str,
        path: &str,
        range: &str,
    ) -> Result<Response, DashboardError> {
        let absent = || {
            send_json(
                http_status(reqwest::StatusCode::NOT_FOUND),
                &json!({"state": "absent", "uri": uri}),
            )
        };
        let Some(blob) = self.store.backend().stat(path).await? else {
            return Ok(absent());
        };
        let content_type = match blob.metadata.get("content-type") {
            Some(stored) => stored.clone(),
            None => OCTET_STREAM.to_string(),
        };
        let Some(size) = blob.size.and_then(|size| usize::try_from(size).ok()) else {
            return Ok(send_json(
                http_status(reqwest::StatusCode::SERVICE_UNAVAILABLE),
                &json!({
                    "error": "size_unknown",
                    "detail": format!(
                        "the store states no size for {uri} that this host can address, so \
                         the range {range} cannot be read alone"
                    ),
                }),
            ));
        };
        let Some((start, end)) = parse_byte_range(range, size) else {
            return Ok(Response::new_with_headers(
                http_status(reqwest::StatusCode::RANGE_NOT_SATISFIABLE),
                "Range Not Satisfiable",
                &content_type,
                b"",
                &[("Content-Range", format!("bytes */{size}"))],
            ));
        };
        let Some(bytes) = self
            .store
            .backend()
            .read_range(path, start as u64, end as u64)
            .await?
        else {
            return Ok(absent());
        };
        if bytes.len() != end - start + 1 {
            return Ok(send_json(
                http_status(reqwest::StatusCode::CONFLICT),
                &json!({
                    "error": "changed",
                    "detail": format!(
                        "{uri} changed while bytes {start}-{end} of its {size} were read: the \
                         store answered {} bytes; read it again",
                        bytes.len()
                    ),
                }),
            ));
        }
        Ok(Response::new_with_headers(
            http_status(reqwest::StatusCode::PARTIAL_CONTENT),
            "Partial Content",
            &content_type,
            &bytes,
            &[
                ("Accept-Ranges", "bytes".to_string()),
                ("Content-Range", format!("bytes {start}-{end}/{size}")),
            ],
        ))
    }
}
