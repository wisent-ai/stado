//! One object body, read whole or, when too large, in ranges. Every error
//! the object API answers with ends the read at once with that cause.

use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) async fn get(&self, uri: &str) -> Result<Vec<u8>, CmdError> {
        self.get_object_whole(uri, false).await?.ok_or_else(|| {
            CmdError::click(format!("object disappeared during GET: {uri}"))
                .stating(crate::primitives::failure::FailureCode::NotFound)
        })
    }

    pub(in crate::cli::storage) async fn get_optional(
        &self,
        uri: &str,
    ) -> Result<Option<Vec<u8>>, CmdError> {
        self.get_object_whole(uri, true).await
    }

    async fn get_object_in_ranges(
        &self,
        endpoint: url::Url,
        bearer: Option<&str>,
        optional: bool,
    ) -> Result<Option<Vec<u8>>, CmdError> {
        // Every error the object API answers with ends the read at once with
        // that cause: a range sent again after a refused connection, a cut
        // body or a failed read only waited on the same answer again.
        let mut body = Vec::new();

        'download: loop {
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
                    "authenticated object GET of the range beginning at {start} failed"
                ))
            })?;
            if response.status() == reqwest::StatusCode::NOT_FOUND && body.is_empty() && optional {
                return Ok(None);
            }
            if !response.status().is_success() {
                return Err(self.response_error(response, bearer).await);
            }
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(CmdError::click(format!(
                    "Stado object API object GET refused the byte range beginning at {start}"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown));
            }
            let (end_exclusive, total) = partial_content_bounds(&response, start, "object GET")?;
            body.reserve(total.saturating_sub(body.capacity()));

            let mut response = response;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if chunk.len() > end_exclusive.saturating_sub(body.len()) {
                            return Err(CmdError::click(
                                "Stado object API object GET sent bytes outside the requested \
                             range",
                            )
                            .stating(crate::primitives::failure::FailureCode::InfraDown));
                        }
                        body.extend_from_slice(&chunk);
                    }
                    Ok(None) if body.len() != end_exclusive => {
                        return Err(CmdError::click(format!(
                            "authenticated object GET range ended after {} of {end_exclusive} bytes",
                            body.len()
                        ))
                        .stating(crate::primitives::failure::FailureCode::InfraDown));
                    }
                    Ok(None) if body.len() == total => return Ok(Some(body)),
                    Ok(None) => continue 'download,
                    Err(error) => {
                        return Err(CmdError::click(format!(
                            "authenticated object GET range connection closed after {} bytes: {error}",
                            body.len()
                        ))
                        .stating(crate::primitives::failure::FailureCode::InfraDown));
                    }
                }
            }
        }
    }

    /// One GET of the whole object. Every error the object API answers with
    /// ends the read at once with that cause, as the ranged read does: a GET
    /// sent again after a refused connection, a failed send or a body cut
    /// short only waits on the same dependency again.
    async fn get_object_whole(
        &self,
        uri: &str,
        optional: bool,
    ) -> Result<Option<Vec<u8>>, CmdError> {
        let endpoint = self.endpoint("/api/object", &[("uri", uri)])?;
        let bearer = self.release_bearer(uri).await?;
        let request = self.request_as(reqwest::Method::GET, endpoint.clone(), bearer.as_deref());
        let response = sent(request)
            .await
            .map_err(|error| error.within("authenticated object GET failed"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND && optional {
            return Ok(None);
        }
        if response.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
            drop(response);
            return self
                .get_object_in_ranges(endpoint, bearer.as_deref(), optional)
                .await;
        }
        if !response.status().is_success() {
            return Err(self.response_error(response, bearer.as_deref()).await);
        }

        let total = declared_size(&response, "object GET")?;
        let mut body = Vec::new();
        if let Some(total) = total {
            body.reserve(total);
        }
        let mut response = response;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                Ok(None) => match total {
                    Some(total) if body.len() != total => {
                        return Err(CmdError::unreachable(format!(
                            "Stado object API object GET response ended after {} of {total} bytes",
                            body.len()
                        )));
                    }
                    _ => return Ok(Some(body)),
                },
                Err(error) => {
                    return Err(CmdError::unreachable(format!(
                        "Stado object API object GET response body connection closed before \
                         completion after {} bytes: {error}",
                        body.len()
                    )));
                }
            }
        }
    }
}
