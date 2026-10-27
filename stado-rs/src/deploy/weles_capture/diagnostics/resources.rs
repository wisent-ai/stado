//! What may be said about one resource a recorded run requested: host and
//! path without the signed query string, and the shape of the URL.

use serde_json::Value;

/// The URL without its query and fragment, which may carry a signature. A
/// recorded URL that does not parse is cut at its first `?` or `#` the same
/// way, so no signed part is said either.
pub(super) fn public_resource_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut parsed) => {
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.to_string()
        }
        Err(_unparsed) => match raw.find(['?', '#']) {
            Some(end) => raw[..end].to_string(),
            None => raw.to_string(),
        },
    }
}

/// The host a recorded URL names; a URL that does not parse, or names no
/// host, has none to count.
pub(super) fn resource_host(raw: &str) -> Option<String> {
    match url::Url::parse(raw) {
        Ok(parsed) => parsed.host_str().map(str::to_string),
        Err(_unparsed) => None,
    }
}

pub(super) fn is_stado_object_url(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|parsed| parsed.path() == "/api/stado/object")
}

pub(super) fn is_legacy_cloud_image_url(raw: &str) -> bool {
    let Some(host) = resource_host(raw) else {
        return false;
    };
    [
        "amazonaws.com",
        "blob.core.windows.net",
        "cloudfront.net",
        "googleapis.com",
        "storage.cloud.google.com",
    ]
    .iter()
    .any(|suffix| {
        host == *suffix
            || host
                .strip_suffix(suffix)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

/// The content type a recorded response declares, or the empty text when it
/// declares none, which no image type matches.
pub(super) fn response_content_type(event: &Value) -> &str {
    let declared = event
        .get("headers")
        .and_then(Value::as_object)
        .and_then(|headers| {
            headers
                .get("content-type")
                .or_else(|| headers.get("Content-Type"))
        })
        .and_then(Value::as_str);
    match declared {
        Some(declared) => declared,
        None => "",
    }
}
