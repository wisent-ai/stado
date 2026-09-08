//! What may be said about one resource a recorded run requested: host and
//! path without the signed query string, and the shape of the URL.

use serde_json::Value;

pub(super) fn public_resource_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut parsed) => {
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.to_string()
        }
        Err(_) => raw.split(['?', '#']).next().unwrap_or_default().to_string(),
    }
}

pub(super) fn resource_host(raw: &str) -> Option<String> {
    url::Url::parse(raw)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
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

pub(super) fn response_content_type(event: &Value) -> &str {
    event
        .get("headers")
        .and_then(Value::as_object)
        .and_then(|headers| {
            headers
                .get("content-type")
                .or_else(|| headers.get("Content-Type"))
        })
        .and_then(Value::as_str)
        .unwrap_or_default()
}
