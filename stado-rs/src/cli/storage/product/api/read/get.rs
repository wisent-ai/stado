//! One object body, resumed after a truncated read and then ranged.

use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) async fn get(&self, uri: &str) -> Result<Vec<u8>, CmdError> {
        self.get_object_resumable(uri, false)
            .await?
            .ok_or_else(|| CmdError::click(format!("object disappeared during GET: {uri}")))
    }

    pub(in crate::cli::storage) async fn get_optional(
        &self,
        uri: &str,
    ) -> Result<Option<Vec<u8>>, CmdError> {
        self.get_object_resumable(uri, true).await
    }

    async fn get_object_in_ranges(
        &self,
        endpoint: url::Url,
        bearer: Option<&str>,
        optional: bool,
    ) -> Result<Option<Vec<u8>>, CmdError> {
        let limit = max_object_api_download_body();
        let mut body = Vec::new();
        let mut failures = 0usize;

        'download: loop {
            let start = body.len();
            let end = start
                .saturating_add(OBJECT_API_CHUNK_BYTES.saturating_sub(1))
                .min(limit.saturating_sub(1));
            let response = match self
                .request_as(reqwest::Method::GET, endpoint.clone(), bearer)
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                .send()
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    failures = failures.saturating_add(1);
                    if failures > 3 {
                        return Err(CmdError::click(format!(
                            "authenticated object GET exhausted its byte-range retries after \
                             {start} bytes: {error}"
                        )));
                    }
                    continue;
                }
            };
            if response.status() == reqwest::StatusCode::NOT_FOUND && body.is_empty() && optional {
                return Ok(None);
            }
            if !response.status().is_success() {
                return Err(self.response_error(response, bearer).await);
            }
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(CmdError::click(format!(
                    "Stado object API object GET refused the byte range beginning at {start}"
                )));
            }
            let (end_exclusive, total) = partial_content_bounds(&response, start, "object GET")?;
            if total > limit {
                return Err(CmdError::click(format!(
                    "Stado object API object GET response exceeds the {limit}-byte limit"
                )));
            }
            body.reserve(total.saturating_sub(body.capacity()));

            let mut response = response;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if chunk.len() > end_exclusive.saturating_sub(body.len()) {
                            return Err(CmdError::click(
                                "Stado object API object GET sent bytes outside the requested \
                                 range",
                            ));
                        }
                        body.extend_from_slice(&chunk);
                    }
                    Ok(None) if body.len() != end_exclusive => {
                        failures = failures.saturating_add(1);
                        if failures > 3 {
                            return Err(CmdError::click(format!(
                                "authenticated object GET exhausted its byte-range retries after \
                                 {} of {end_exclusive} bytes",
                                body.len()
                            )));
                        }
                        continue 'download;
                    }
                    Ok(None) if body.len() == total => return Ok(Some(body)),
                    Ok(None) => {
                        failures = 0;
                        continue 'download;
                    }
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        if failures > 3 {
                            return Err(CmdError::click(format!(
                                "authenticated object GET exhausted its byte-range retries after \
                                 {} bytes: {error}",
                                body.len()
                            )));
                        }
                        continue 'download;
                    }
                }
            }
        }
    }

    async fn get_object_resumable(
        &self,
        uri: &str,
        optional: bool,
    ) -> Result<Option<Vec<u8>>, CmdError> {
        let endpoint = self.endpoint("/api/object", &[("uri", uri)])?;
        let bearer = self.release_bearer(uri).await?;
        let limit = max_object_api_download_body();
        let mut body = Vec::new();
        let mut last_read_error = None;

        for recovery in 0..=3 {
            let mut request =
                self.request_as(reqwest::Method::GET, endpoint.clone(), bearer.as_deref());
            if !body.is_empty() {
                request = request.header(reqwest::header::RANGE, format!("bytes={}-", body.len()));
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => {
                    last_read_error = Some(format!(
                        "error sending authenticated object GET after {} bytes: {}",
                        body.len(),
                        crate::cli::http_failure(&error)
                    ));
                    if recovery == 3 {
                        break;
                    }
                    continue;
                }
            };
            if response.status() == reqwest::StatusCode::NOT_FOUND && body.is_empty() && optional {
                return Ok(None);
            }
            if response.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE && body.is_empty() {
                drop(response);
                return self
                    .get_object_in_ranges(endpoint, bearer.as_deref(), optional)
                    .await;
            }
            if !response.status().is_success() {
                return Err(self.response_error(response, bearer.as_deref()).await);
            }
            if !body.is_empty() && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(CmdError::click(format!(
                    "Stado object API object GET refused byte resume at offset {}",
                    body.len()
                )));
            }

            let expected_start = body.len();
            let total = if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
                let content_range = response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| {
                        CmdError::click(
                            "Stado object API object GET partial response carries no Content-Range",
                        )
                    })?;
                let (range, total) = content_range
                    .strip_prefix("bytes ")
                    .and_then(|value| value.split_once('/'))
                    .ok_or_else(|| {
                        CmdError::click(format!(
                            "Stado object API object GET returned invalid Content-Range \
                             {content_range:?}"
                        ))
                    })?;
                let (start, _) = range.split_once('-').ok_or_else(|| {
                    CmdError::click(format!(
                        "Stado object API object GET returned invalid Content-Range \
                         {content_range:?}"
                    ))
                })?;
                let start = start.parse::<usize>().map_err(|_| {
                    CmdError::click(format!(
                        "Stado object API object GET returned invalid Content-Range \
                         {content_range:?}"
                    ))
                })?;
                let total = total.parse::<usize>().map_err(|_| {
                    CmdError::click(format!(
                        "Stado object API object GET returned invalid Content-Range \
                         {content_range:?}"
                    ))
                })?;
                if start != expected_start {
                    return Err(CmdError::click(format!(
                        "Stado object API object GET resumed at byte {start}, expected \
                         {expected_start}"
                    )));
                }
                Some(total)
            } else {
                response
                    .content_length()
                    .and_then(|length| usize::try_from(length).ok())
            };
            if total.is_some_and(|total| total > limit) {
                return Err(CmdError::click(format!(
                    "Stado object API object GET response exceeds the {limit}-byte limit"
                )));
            }
            if let Some(total) = total {
                body.reserve(total.saturating_sub(body.capacity()));
            }

            let mut response = response;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if chunk.len() > limit.saturating_sub(body.len()) {
                            return Err(CmdError::click(format!(
                                "Stado object API object GET response exceeds the \
                                 {limit}-byte limit"
                            )));
                        }
                        body.extend_from_slice(&chunk);
                    }
                    Ok(None) if total.is_some_and(|total| body.len() != total) => {
                        last_read_error = Some(format!(
                            "Stado object API object GET response ended after {} of {} bytes",
                            body.len(),
                            total.unwrap_or_default()
                        ));
                        break;
                    }
                    Ok(None) => return Ok(Some(body)),
                    Err(error) => {
                        last_read_error = Some(format!(
                            "Stado object API object GET response body connection closed before \
                             completion after {} bytes: {error}",
                            body.len()
                        ));
                        break;
                    }
                }
            }
            if recovery == 3 {
                break;
            }
        }

        Err(CmdError::click(last_read_error.unwrap_or_else(|| {
            "authenticated object GET exhausted its byte-resume attempts".to_string()
        })))
    }
}
