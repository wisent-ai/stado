//! The URLs every request is addressed through, and the encoder they share.
//!
//! One upload endpoint, one object resource endpoint, one media endpoint and
//! two listing endpoints, so the percent-encoding the JSON API needs (an
//! object name is a single path segment) and the pagination query are written
//! once here instead of at each route.

use super::API_BASE;

/// Percent-encode per RFC 3986: keep the unreserved set, encode everything
/// else as uppercase %XX of the UTF-8 bytes. Used both for the object path
/// segment of the JSON API (slash must become %2F) and for query params.
pub(crate) fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// POST endpoint for media upload; `name` travels as a query param.
pub(super) fn upload_url(bucket: &str, name: &str, if_generation_match: Option<&str>) -> String {
    let mut url = format!(
        "{API_BASE}/upload/storage/v1/b/{bucket}/o?uploadType=media&name={}",
        percent_encode(name)
    );
    if let Some(generation) = if_generation_match {
        url.push_str(&format!("&ifGenerationMatch={generation}"));
    }
    url
}

/// Object resource endpoint; the object name is a single path segment, so
/// slashes inside the name must be encoded as %2F.
pub(super) fn object_url(bucket: &str, name: &str) -> String {
    format!(
        "{API_BASE}/storage/v1/b/{bucket}/o/{}",
        percent_encode(name)
    )
}

/// Media download endpoint (`?alt=media`).
pub(super) fn media_url(bucket: &str, name: &str) -> String {
    format!("{}?alt=media", object_url(bucket, name))
}

/// Object listing endpoint with `fields` projection and pagination.
pub(super) fn list_url(
    bucket: &str,
    prefix: &str,
    page_token: Option<&str>,
    fields: &str,
) -> String {
    let mut url = format!(
        "{API_BASE}/storage/v1/b/{bucket}/o?prefix={}&fields={}",
        percent_encode(prefix),
        percent_encode(fields)
    );
    if let Some(token) = page_token {
        url.push_str(&format!("&pageToken={}", percent_encode(token)));
    }
    url
}

/// Object listing endpoint for one bounded, ordered page: `startOffset`
/// makes the server begin the name-ordered scan at the resume point and
/// `maxResults` caps the page. `startOffset` is INCLUSIVE, so the caller
/// still has to drop a name equal to its cursor.
pub(super) fn list_page_url(
    bucket: &str,
    prefix: &str,
    page_token: Option<&str>,
    fields: &str,
    start_offset: &str,
    max_results: usize,
) -> String {
    let mut url = list_url(bucket, prefix, page_token, fields);
    if !start_offset.is_empty() {
        url.push_str(&format!("&startOffset={}", percent_encode(start_offset)));
    }
    if max_results > 0 {
        url.push_str(&format!("&maxResults={max_results}"));
    }
    url
}
