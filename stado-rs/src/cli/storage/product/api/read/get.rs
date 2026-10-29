//! One object body: one GET of the whole object, resumed in ranges of the
//! route's chunk (`OBJECT_API_CHUNK_BYTES`) from the first byte it lacks
//! when the body stops after some of its bytes arrived. Every body is read
//! as a wait with the route's progress window ([`super::body`]), so a body
//! that stops arriving is named as a stall. A read that made progress is
//! asked again from where it stopped, since that is not the same answer
//! awaited again; one that brought no byte, and every error the object API
//! answers with, ends the read at once with its cause. A store that refuses
//! to send the object whole (413) is read in ranges from the start.

use super::body::{self, Ended};
use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) async fn get(&self, uri: &str) -> Result<Vec<u8>, CmdError> {
        self.get_object(uri, false).await?.ok_or_else(|| {
            CmdError::click(format!("object disappeared during GET: {uri}"))
                .stating(crate::primitives::failure::FailureCode::NotFound)
        })
    }

    pub(in crate::cli::storage) async fn get_optional(
        &self,
        uri: &str,
    ) -> Result<Option<Vec<u8>>, CmdError> {
        self.get_object(uri, true).await
    }

    async fn get_object(&self, uri: &str, optional: bool) -> Result<Option<Vec<u8>>, CmdError> {
        let endpoint = self.endpoint("/api/object", &[("uri", uri)])?;
        let bearer = self.release_bearer(uri).await?;
        let bearer = bearer.as_deref();
        let response = sent(self.request_as(reqwest::Method::GET, endpoint.clone(), bearer))
            .await
            .map_err(|error| error.within(format!("authenticated object GET of {uri} failed")))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND && optional {
            return Ok(None);
        }
        let mut body = Vec::new();
        if response.status() != reqwest::StatusCode::PAYLOAD_TOO_LARGE {
            if !response.status().is_success() {
                return Err(self.response_error(response, bearer).await);
            }
            let total = declared_size(&response, "object GET")?;
            if let Some(total) = total {
                body.reserve(total);
            }
            let what = format!("object GET of {uri}");
            match body::read(response, &what, &mut body, total).await? {
                Ended::Closed if total.is_none_or(|total| body.len() == total) => {
                    return Ok(Some(body))
                }
                Ended::Closed | Ended::Broken(_) if !body.is_empty() => {
                    eprintln!(
                        "stado: {what}: the body stopped after {} bytes; asking for the rest \
                         from byte {} in ranges",
                        body.len(),
                        body.len()
                    );
                }
                Ended::Closed => {
                    return Err(CmdError::unreachable(format!(
                        "{what}: the response ended with no byte of its declared body"
                    )))
                }
                Ended::Broken(said) => return Err(CmdError::unreachable(said)),
            }
        }
        self.get_object_in_ranges(uri, endpoint, bearer, body)
            .await
            .map(Some)
    }

    /// The rest of the object from `body.len()`, one range at a time.
    async fn get_object_in_ranges(
        &self,
        uri: &str,
        endpoint: url::Url,
        bearer: Option<&str>,
        mut body: Vec<u8>,
    ) -> Result<Vec<u8>, CmdError> {
        loop {
            let start = body.len();
            // The last byte of this chunk: a Range header names it inclusively.
            let Some(end) = (start..start.saturating_add(OBJECT_API_CHUNK_BYTES)).next_back()
            else {
                return Err(CmdError::click(
                    "OBJECT_API_CHUNK_BYTES declares no bytes per ranged object GET",
                )
                .stating(crate::primitives::failure::FailureCode::Config));
            };
            let response = sent(
                self.request_as(reqwest::Method::GET, endpoint.clone(), bearer)
                    .header(reqwest::header::RANGE, format!("bytes={start}-{end}")),
            )
            .await
            .map_err(|error| {
                error.within(format!(
                    "authenticated object GET of {uri} from byte {start} failed"
                ))
            })?;
            if !response.status().is_success() {
                return Err(self.response_error(response, bearer).await);
            }
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(CmdError::click(format!(
                    "Stado object API object GET of {uri} refused the byte range beginning at \
                     {start}"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown));
            }
            let (end_exclusive, total) = partial_content_bounds(&response, start, "object GET")?;
            body.reserve(total.saturating_sub(body.len()));
            let what = format!("object GET of {uri} ({total} bytes)");
            let ended = body::read(response, &what, &mut body, Some(end_exclusive - start)).await?;
            match ended {
                Ended::Closed if body.len() == total => return Ok(body),
                Ended::Closed if body.len() == end_exclusive => {}
                Ended::Closed | Ended::Broken(_) if body.len() > start => {
                    eprintln!(
                        "stado: {what}: the range from byte {start} stopped after {} bytes; \
                         asking for the rest from byte {}",
                        body.len() - start,
                        body.len()
                    );
                }
                Ended::Closed => {
                    return Err(CmdError::unreachable(format!(
                        "{what}: the range from byte {start} ended with no byte of it"
                    )))
                }
                Ended::Broken(said) => return Err(CmdError::unreachable(said)),
            }
        }
    }
}
