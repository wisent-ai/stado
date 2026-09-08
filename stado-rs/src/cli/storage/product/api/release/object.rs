//! One release object read, and one release presence probe.

use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) async fn get_release(
        &self,
        uri: &str,
    ) -> Result<Vec<u8>, CmdError> {
        let origin = self.endpoint("/api/release/object", &[("uri", uri)])?;
        let limit = max_object_api_download_body();
        let mut body = Vec::new();
        let mut last_read_error = None;

        for recovery in 0..=3 {
            let mut endpoint = origin.clone();
            for hop in 0..=3 {
                let mut request = if hop == 0 {
                    self.request(reqwest::Method::GET, endpoint.clone())
                } else {
                    self.http.get(endpoint.clone())
                };
                if !body.is_empty() {
                    request =
                        request.header(reqwest::header::RANGE, format!("bytes={}-", body.len()));
                }
                let response = request.send().await?;
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
                if response.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE && body.is_empty() {
                    drop(response);
                    return self.get_release_in_ranges(origin).await;
                }
                if !response.status().is_success() {
                    return Err(self.response_error(response, None).await);
                }
                if !body.is_empty() && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                    return Err(CmdError::click(format!(
                        "Stado object API release GET refused byte resume at offset {}",
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
                                "Stado object API release GET partial response carries no \
                                 Content-Range",
                            )
                        })?;
                    let (range, total) = content_range
                        .strip_prefix("bytes ")
                        .and_then(|value| value.split_once('/'))
                        .ok_or_else(|| {
                            CmdError::click(format!(
                                "Stado object API release GET returned invalid Content-Range \
                                 {content_range:?}"
                            ))
                        })?;
                    let (start, _) = range.split_once('-').ok_or_else(|| {
                        CmdError::click(format!(
                            "Stado object API release GET returned invalid Content-Range \
                             {content_range:?}"
                        ))
                    })?;
                    let start = start.parse::<usize>().map_err(|_| {
                        CmdError::click(format!(
                            "Stado object API release GET returned invalid Content-Range \
                             {content_range:?}"
                        ))
                    })?;
                    let total = total.parse::<usize>().map_err(|_| {
                        CmdError::click(format!(
                            "Stado object API release GET returned invalid Content-Range \
                             {content_range:?}"
                        ))
                    })?;
                    if start != expected_start {
                        return Err(CmdError::click(format!(
                            "Stado object API release GET resumed at byte {start}, expected \
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
                        "Stado object API release GET response exceeds the {limit}-byte limit"
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
                                    "Stado object API release GET response exceeds the \
                                     {limit}-byte limit"
                                )));
                            }
                            body.extend_from_slice(&chunk);
                        }
                        // The stream ending is not the object ending. The
                        // release route streams an unranged GET until its own
                        // window closes and then closes the body cleanly, with
                        // no length declared and no error to read: the darwin
                        // archive of 0.13.46 is 73,864,632 bytes and two reads
                        // of it here returned 22,925,186 and 15,318,446, both
                        // as `Ok`. A short object with a successful exit is
                        // worse than a failed download, because every caller
                        // downstream -- digest verification, archive extract,
                        // a host staging a release -- reports its own true
                        // finding about bytes that were never the object.
                        //
                        // Whole is provable only against a declared total, so
                        // that is the only thing accepted here. Anything else
                        // goes to the bounded byte-range reader below, which
                        // asks for one chunk at a time and knows the total
                        // from every `Content-Range` it gets back.
                        Ok(None) if total == Some(body.len()) => return Ok(body),
                        Ok(None) => return self.get_release_in_ranges(origin).await,
                        Err(error) => {
                            last_read_error = Some(format!(
                                "Stado object API release GET response body connection closed \
                                 before completion after {} bytes: {error}",
                                body.len()
                            ));
                            break;
                        }
                    }
                }
                break;
            }
            if recovery == 3 {
                break;
            }
        }
        Err(CmdError::click(last_read_error.unwrap_or_else(|| {
            "release GET exceeded three redirects".to_string()
        })))
    }

    /// Ask the release channel itself whether it serves one object.
    ///
    /// `stat` otherwise answers from the configured job store, and for a
    /// `stado://releases/...` URI that is the wrong witness entirely: the channel
    /// publishes through this route, and the local store has never held those bytes.
    /// Reading its silence as `absent` is how a baseline naming a published artifact
    /// gets certified against a store that could not have served it either way.
    ///
    /// Five states, because two would let silence pass for absence and three let
    /// every kind of silence pass for one kind. A redirect counts as present: this
    /// route answers a served object by redirecting to where the bytes live, and the
    /// client does not follow it, so the redirect IS the testimony. An explicit 404
    /// is absence. Every other status is a way of not answering, and
    /// [`unanswered_for_status`] says which way, because `401` (this reader may not
    /// ask), `503` (the boundary is down, ask again) and `502` (the resolver's SSH
    /// forward carried nothing) have three different remedies and used to arrive as
    /// one word. A transport error that never produced a status answered nothing at
    /// all, so it is unreachable outright.
    pub(in crate::cli::storage) async fn stat_release(
        &self,
        uri: &str,
    ) -> Result<Presence, CmdError> {
        let endpoint = self.endpoint("/api/release/object", &[("uri", uri)])?;
        match self.request(reqwest::Method::GET, endpoint).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() || status.is_redirection() {
                    let size = response
                        .content_length()
                        .and_then(|value| usize::try_from(value).ok())
                        .unwrap_or_default();
                    Ok(Presence::Present {
                        size,
                        version: None,
                        detail: None,
                    })
                } else if status == reqwest::StatusCode::NOT_FOUND {
                    Ok(Presence::Absent)
                } else {
                    Ok(unanswered_for_status(
                        status.as_u16(),
                        self.response_error(response, None).await.to_string(),
                    ))
                }
            }
            Err(error) => Ok(Presence::Unreachable(error.to_string())),
        }
    }
}
