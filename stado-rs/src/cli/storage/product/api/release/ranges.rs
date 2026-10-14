//! The ranged fallback for a release body no single response carried.

use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) async fn get_release_in_ranges(
        &self,
        origin: url::Url,
    ) -> Result<Vec<u8>, CmdError> {
        // Every error the release channel answers with ends the download at
        // once with that cause: a range sent again after a refused
        // connection, a cut body or a failed read only waited on the same
        // answer again.
        let mut body = Vec::new();

        'download: loop {
            let start = body.len();
            let end = start.saturating_add(OBJECT_API_CHUNK_BYTES.saturating_sub(1));
            let mut endpoint = origin.clone();
            let mut selected = None;
            for hop in 0..=3 {
                let request = if hop == 0 {
                    self.request(reqwest::Method::GET, endpoint.clone())
                } else {
                    self.http.get(endpoint.clone())
                }
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"));
                let response = match sent(request).await {
                    Ok(response) => response,
                    Err(error) => {
                        let cause = match crate::wait::refusal(&error) {
                            Some(cause) => {
                                format!("the release channel refused the connection: {cause}")
                            }
                            None => crate::cli::http_failure(&error),
                        };
                        return Err(CmdError::unreachable(format!(
                            "public release GET of the range beginning at {start} failed: {cause}"
                        )));
                    }
                };
                if response.status().is_redirection() {
                    let location = response
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| {
                            CmdError::click("release redirect carries no Location")
                                .stating(crate::primitives::failure::FailureCode::InfraDown)
                        })?;
                    endpoint = response.url().join(location).map_err(|error| {
                        CmdError::click(format!("invalid release redirect: {error}"))
                            .stating(crate::primitives::failure::FailureCode::InfraDown)
                    })?;
                    continue;
                }
                selected = Some(response);
                break;
            }
            let Some(response) = selected else {
                return Err(CmdError::click("too many release download redirects")
                    .stating(crate::primitives::failure::FailureCode::InfraDown));
            };
            if !response.status().is_success() {
                return Err(self.response_error(response, None).await);
            }
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(CmdError::click(format!(
                    "Stado object API release GET refused the byte range beginning at {start}"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown));
            }
            let (end_exclusive, total) = partial_content_bounds(&response, start, "release GET")?;
            body.reserve(total.saturating_sub(body.capacity()));

            let mut response = response;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if chunk.len() > end_exclusive.saturating_sub(body.len()) {
                            return Err(CmdError::click(
                                "Stado object API release GET sent bytes outside the requested \
                             range",
                            )
                            .stating(crate::primitives::failure::FailureCode::InfraDown));
                        }
                        body.extend_from_slice(&chunk);
                    }
                    Ok(None) if body.len() != end_exclusive => {
                        return Err(CmdError::click(format!(
                            "public release GET range ended after {} of {end_exclusive} bytes",
                            body.len()
                        ))
                        .stating(crate::primitives::failure::FailureCode::InfraDown));
                    }
                    Ok(None) if body.len() == total => return Ok(body),
                    Ok(None) => continue 'download,
                    Err(error) => {
                        return Err(CmdError::click(format!(
                            "public release GET range connection closed after {} bytes: {error}",
                            body.len()
                        ))
                        .stating(crate::primitives::failure::FailureCode::InfraDown));
                    }
                }
            }
        }
    }
}
