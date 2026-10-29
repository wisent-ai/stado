//! One object body, read one range of the route's chunk
//! (`OBJECT_API_CHUNK_BYTES`) at a time, each range a request of its own,
//! so a large object is never held whole by the store to answer one request
//! and a body that stops arriving loses one range rather than the object. A
//! store that answers the first range with the whole object (it serves no
//! ranges) is read whole. Every body is read as a wait with the route's
//! progress window ([`super::body`]). A range that stops after some of its
//! bytes arrived is asked again from the first byte it lacks: it made
//! progress, so it is not the same answer awaited again. A range that brings
//! no byte, and every error the object API answers with, ends the read at
//! once with its cause.

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
        let mut body = Vec::new();
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
            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND && body.is_empty() && optional {
                return Ok(None);
            }
            if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && body.is_empty() {
                // The first range of an empty object starts past its end.
                return match partial_total(&response) {
                    Some(0) => Ok(Some(body)),
                    _ => Err(self.response_error(response, bearer).await),
                };
            }
            if status == reqwest::StatusCode::OK && body.is_empty() {
                return self.whole(uri, response).await.map(Some);
            }
            if status != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(self.response_error(response, bearer).await);
            }
            let (end_exclusive, total) = partial_content_bounds(&response, start, "object GET")?;
            body.reserve(total.saturating_sub(body.len()));
            let what = format!("object GET of {uri} ({total} bytes)");
            let ended = body::read(response, &what, &mut body, Some(end_exclusive - start)).await?;
            match ended {
                Ended::Closed if body.len() == total => return Ok(Some(body)),
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

    /// The whole object a store that serves no ranges answered the first
    /// range with.
    async fn whole(&self, uri: &str, response: reqwest::Response) -> Result<Vec<u8>, CmdError> {
        let total = declared_size(&response, "object GET")?;
        let mut body = match total {
            Some(total) => Vec::with_capacity(total),
            None => Vec::new(),
        };
        let what = format!("object GET of {uri}");
        match body::read(response, &what, &mut body, total).await? {
            Ended::Closed => match total {
                Some(total) if body.len() != total => Err(CmdError::unreachable(format!(
                    "{what}: the response ended after {} of {total} bytes",
                    body.len()
                ))),
                _ => Ok(body),
            },
            Ended::Broken(said) => Err(CmdError::unreachable(said)),
        }
    }
}

/// The total a `Content-Range: bytes */<total>` refusal states.
fn partial_total(response: &reqwest::Response) -> Option<usize> {
    response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)?
        .to_str()
        .ok()?
        .strip_prefix("bytes */")?
        .parse()
        .ok()
}
