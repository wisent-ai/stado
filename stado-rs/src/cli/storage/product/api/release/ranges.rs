//! The ranged fallback for a release body no single response carried.

use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) async fn get_release_in_ranges(
        &self,
        origin: url::Url,
    ) -> Result<Vec<u8>, CmdError> {
        let limit = max_object_api_download_body();
        let mut body = Vec::new();
        let mut failures = 0usize;

        'download: loop {
            let start = body.len();
            let end = start
                .saturating_add(OBJECT_API_CHUNK_BYTES.saturating_sub(1))
                .min(limit.saturating_sub(1));
            let mut endpoint = origin.clone();
            let mut selected = None;
            for hop in 0..=3 {
                let request = if hop == 0 {
                    self.request(reqwest::Method::GET, endpoint.clone())
                } else {
                    self.http.get(endpoint.clone())
                }
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"));
                let response = match request.send().await {
                    Ok(response) => response,
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        if failures > 3 {
                            return Err(CmdError::click(format!(
                                "public release GET exhausted its byte-range retries after \
                                 {start} bytes: {error}"
                            )));
                        }
                        continue 'download;
                    }
                };
                if response.status().is_redirection() {
                    let location = response
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| CmdError::click("release redirect carries no Location"))?;
                    endpoint = response.url().join(location).map_err(|error| {
                        CmdError::click(format!("invalid release redirect: {error}"))
                    })?;
                    continue;
                }
                selected = Some(response);
                break;
            }
            let Some(response) = selected else {
                return Err(CmdError::click("too many release download redirects"));
            };
            if !response.status().is_success() {
                return Err(self.response_error(response, None).await);
            }
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(CmdError::click(format!(
                    "Stado object API release GET refused the byte range beginning at {start}"
                )));
            }
            let (end_exclusive, total) = partial_content_bounds(&response, start, "release GET")?;
            if total > limit {
                return Err(CmdError::click(format!(
                    "Stado object API release GET response exceeds the {limit}-byte limit"
                )));
            }
            body.reserve(total.saturating_sub(body.capacity()));

            let mut response = response;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if chunk.len() > end_exclusive.saturating_sub(body.len()) {
                            return Err(CmdError::click(
                                "Stado object API release GET sent bytes outside the requested \
                                 range",
                            ));
                        }
                        body.extend_from_slice(&chunk);
                    }
                    Ok(None) if body.len() != end_exclusive => {
                        failures = failures.saturating_add(1);
                        if failures > 3 {
                            return Err(CmdError::click(format!(
                                "public release GET exhausted its byte-range retries after {} of \
                                 {end_exclusive} bytes",
                                body.len()
                            )));
                        }
                        continue 'download;
                    }
                    Ok(None) if body.len() == total => return Ok(body),
                    Ok(None) => {
                        failures = 0;
                        continue 'download;
                    }
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        if failures > 3 {
                            return Err(CmdError::click(format!(
                                "public release GET exhausted its byte-range retries after {} \
                                 bytes: {error}",
                                body.len()
                            )));
                        }
                        continue 'download;
                    }
                }
            }
        }
    }
}
