//! One object route under an origin, and the response detail a failure
//! reports with every bearer redacted.

use crate::cli::storage::*;

pub(crate) fn object_api_endpoint(
    base_url: &url::Url,
    route: &str,
    query: &[(&str, &str)],
) -> Result<url::Url, CmdError> {
    let mut endpoint = base_url.clone();
    {
        let mut segments = endpoint.path_segments_mut().map_err(|()| {
            CmdError::click("configured object base URL cannot be used as an HTTP API base URL")
        })?;
        segments.pop_if_empty();
        for segment in route.trim_start_matches('/').split('/') {
            segments.push(segment);
        }
    }
    if !query.is_empty() {
        let mut pairs = endpoint.query_pairs_mut();
        for &(name, value) in query {
            pairs.append_pair(name, value);
        }
    }
    Ok(endpoint)
}

pub(in crate::cli::storage) fn response_body_detail(
    body: &[u8],
    generic_bearer: Option<&str>,
    request_bearer: Option<&str>,
) -> String {
    let detail = String::from_utf8_lossy(body);
    let detail = detail.trim();
    if detail.is_empty() {
        return "<empty response body>".to_string();
    }
    let mut redacted = detail.to_string();
    for secret in [generic_bearer, request_bearer].into_iter().flatten() {
        if !secret.is_empty() {
            redacted = redacted.replace(secret, "[REDACTED]");
        }
    }
    redacted
}
