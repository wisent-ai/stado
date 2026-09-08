//! The object reads and writes themselves: one buffered body in, one
//! declared-length body out.

use reqwest::{Method, Response, StatusCode};

use crate::queue::StorageError;

use super::super::StadoObjectBackend;

impl StadoObjectBackend {
    /// The whole body of one successful object response.
    ///
    /// `bytes()` yields what arrived, which is not the same claim as what the
    /// object is. The route answers every read with a `Content-Length` — 41,041
    /// bytes for the canonical registry — and `Accept-Ranges: bytes`, and
    /// nothing here compared the two. That matters because of where the body
    /// goes next: `providers::local::disk_cleanup::fetch_canonical_registry`
    /// hands it to `serde_json::from_str`, so a body that is not the whole
    /// object is journalled as a document that does not parse — `ValueError`,
    /// a finding about the registry's content — when what happened was a
    /// transfer. #317 is open on exactly that shape for the sibling
    /// `/api/release/object` route.
    ///
    /// What this reader owns is narrower than "truncation", and saying so is
    /// the point. An HTTP/1.1 body that stops early under a declared length is
    /// already refused by the client's own framing check and arrives as
    /// [`StorageError::Http`]. What reaches here instead is framing that ends
    /// cleanly at a size the response's own declaration contradicts — a body
    /// chunked to its end, or re-framed by something between the gateway and
    /// this process, while the declared length still says how long the object
    /// was. `tests/truncation` pins both halves.
    ///
    /// A declared length is the only thing that can be checked, so it is the
    /// only thing that is: a chunked or otherwise unlengthed response has
    /// nothing to disagree with and passes through exactly as before.
    ///
    /// `limit` is the second thing a declared length is good for. An object
    /// read is buffered whole, in this process, and `to_vec` holds a second
    /// copy while it is built, so an unbounded read is unbounded MEMORY and
    /// not merely unbounded time. A timeout cannot help with that: it fires
    /// after the bytes are already resident. Callers that read a document
    /// whose size is part of its contract pass a ceiling, and a response
    /// declaring more than the ceiling is refused BEFORE the body is
    /// requested, so the bytes never arrive. Callers that read software
    /// artifacts pass `None`: those are legitimately large and are written
    /// straight to a file.
    pub(super) async fn whole_body(
        response: Response,
        path: &str,
        limit: Option<usize>,
    ) -> Result<Vec<u8>, StorageError> {
        let declared = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<usize>().ok());
        if let (Some(limit), Some(declared)) = (limit, declared) {
            if declared > limit {
                return Err(StorageError::Other(format!(
                    "Stado object API declared {declared} bytes for {path}, over the {limit}-byte \
                     document ceiling; refused without reading the body"
                )));
            }
        }
        let bytes = response.bytes().await?.to_vec();
        if let Some(declared) = declared {
            if bytes.len() != declared {
                return Err(StorageError::Other(format!(
                    "Stado object API returned {} of {declared} declared bytes for {path}",
                    bytes.len()
                )));
            }
        }
        // An unlengthed response had nothing to refuse in advance. Name what
        // arrived rather than handing a document of unknown size to a parser:
        // the memory is already spent, but the read stops being a silent way
        // to grow this process without bound.
        if let Some(limit) = limit {
            if bytes.len() > limit {
                return Err(StorageError::Other(format!(
                    "Stado object API returned {} bytes for {path} with no declared length, over \
                     the {limit}-byte document ceiling",
                    bytes.len()
                )));
            }
        }
        Ok(bytes)
    }

    /// One object read, optionally under a byte ceiling. See
    /// [`Self::whole_body`] for what the ceiling buys and why a timeout does
    /// not buy it.
    pub(super) async fn download_bytes_limited(
        &self,
        path: &str,
        limit: Option<usize>,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        let response =
            Self::send_through_boundary(self.request(Method::GET, self.object_url(path, &[])?))
                .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        Ok(Some(Self::whole_body(response, path, limit).await?))
    }

    pub(super) async fn upload(
        &self,
        path: &str,
        content: Vec<u8>,
        if_absent: bool,
    ) -> Result<bool, StorageError> {
        let options = if if_absent {
            vec![("if_absent", "true")]
        } else {
            Vec::new()
        };
        let response = Self::send_through_boundary(
            self.request(Method::PUT, self.object_url(path, &options)?)
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                // Reqwest omits Content-Length for an empty Vec body. The object
                // endpoint requires the header even when the payload is zero bytes,
                // so empty logs and artifacts must declare their length explicitly.
                .header(reqwest::header::CONTENT_LENGTH, content.len())
                .body(content),
        )
        .await?;
        if matches!(
            response.status(),
            StatusCode::CONFLICT | StatusCode::PRECONDITION_FAILED
        ) {
            return Ok(false);
        }
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        Ok(true)
    }
}
