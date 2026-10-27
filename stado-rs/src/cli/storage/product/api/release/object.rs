//! One release object read, and one release presence probe.

use crate::cli::storage::*;

impl RemoteObjectApi {
    /// The release channel's answer for `origin`, its redirects followed to
    /// where the bytes live. Only the channel itself is sent the reader's
    /// credentials; where it redirects to is asked without them. A failed
    /// send ends at once with its cause, named `what`; a redirect back to an
    /// address already asked is a cycle, refused with the addresses followed.
    pub(in crate::cli::storage) async fn release_response(
        &self,
        origin: &url::Url,
        range: Option<&str>,
        what: &str,
    ) -> Result<reqwest::Response, CmdError> {
        let mut endpoint = origin.clone();
        let mut followed: Vec<url::Url> = Vec::new();
        loop {
            let mut request = if followed.is_empty() {
                self.request(reqwest::Method::GET, endpoint.clone())
            } else {
                self.http.get(endpoint.clone())
            };
            if let Some(range) = range {
                request = request.header(reqwest::header::RANGE, range);
            }
            let response = sent(request)
                .await
                .map_err(|error| error.within(format!("{what} failed")))?;
            if !response.status().is_redirection() {
                return Ok(response);
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or_else(|| {
                    CmdError::click("release redirect carries no Location")
                        .stating(crate::primitives::failure::FailureCode::InfraDown)
                })?
                .to_str()
                .map_err(|error| {
                    CmdError::click(format!("release redirect Location is not text: {error}"))
                        .stating(crate::primitives::failure::FailureCode::InfraDown)
                })?;
            let next = response.url().join(location).map_err(|error| {
                CmdError::click(format!("invalid release redirect: {error}"))
                    .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
            followed.push(endpoint);
            if followed.contains(&next) {
                let chain: Vec<&str> = followed.iter().map(url::Url::as_str).collect();
                return Err(CmdError::unreachable(format!(
                    "release redirects return to {next}, already asked: {}",
                    chain.join(" -> ")
                )));
            }
            endpoint = next;
        }
    }

    /// One release object. Every error the channel answers with ends the read
    /// at once with that cause: a GET sent again after a failed send or a
    /// body cut short only waits on the same dependency again.
    pub(in crate::cli::storage) async fn get_release(
        &self,
        uri: &str,
    ) -> Result<Vec<u8>, CmdError> {
        let origin = self.endpoint("/api/release/object", &[("uri", uri)])?;
        let response = self.release_response(&origin, None, "release GET").await?;
        if response.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
            drop(response);
            return self.get_release_in_ranges(origin).await;
        }
        if !response.status().is_success() {
            return Err(self.response_error(response, None).await);
        }

        let total = declared_size(&response, "release GET")?;
        let mut body = Vec::new();
        if let Some(total) = total {
            body.reserve(total);
        }
        let mut response = response;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                // The stream ending is not the object ending. The release
                // route streams an unranged GET until its own window closes
                // and then closes the body cleanly, with no length declared
                // and no error to read: the darwin archive of 0.13.46 is
                // 73,864,632 bytes and two reads of it here returned
                // 22,925,186 and 15,318,446, both as `Ok`. A short object with
                // a successful exit is worse than a failed download, because
                // every caller downstream -- digest verification, archive
                // extract, a host staging a release -- reports its own true
                // finding about bytes that were never the object.
                //
                // Whole is provable only against a declared total, so that
                // is the only thing accepted here. Anything else goes to the
                // bounded byte-range reader, which asks for one chunk at a
                // time and knows the total from every `Content-Range` it gets
                // back.
                Ok(None) if total == Some(body.len()) => return Ok(body),
                Ok(None) => return self.get_release_in_ranges(origin).await,
                Err(error) => {
                    return Err(CmdError::unreachable(format!(
                        "Stado object API release GET response body connection closed before \
                         completion after {} bytes: {error}",
                        body.len()
                    )));
                }
            }
        }
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
        match sent(self.request(reqwest::Method::GET, endpoint)).await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() || status.is_redirection() {
                    // A redirect names where the bytes live and need not say
                    // how many there are; read as zero, a published archive
                    // was handed to its target as a zero-byte download.
                    let size = declared_size(&response, "release presence probe")?;
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
