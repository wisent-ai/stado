//! Endpoint assembly: Python `_url` and `urllib.parse.quote_plus`.
//!
//! Path segments are percent-encoded with an empty safe set, empty query
//! values are dropped, and the rest is quote_plus-encoded.

use crate::queue::gcs::percent_encode;

use super::BoxHttpTransport;

impl BoxHttpTransport {
    /// Python `_url`: quote every path segment with an empty safe set,
    /// drop empty query values, quote_plus the rest.
    pub(crate) fn url(&self, path: &str, query: &[(&str, String)]) -> String {
        let clean: Vec<String> = path
            .trim_matches('/')
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(percent_encode)
            .collect();
        let mut url = if clean.is_empty() {
            self.base_url.clone()
        } else {
            format!("{}/{}", self.base_url, clean.join("/"))
        };
        let pairs: Vec<&(&str, String)> = query
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .collect();
        if !pairs.is_empty() {
            let encoded: Vec<String> = pairs
                .iter()
                .map(|(key, value)| format!("{}={}", quote_plus(key), quote_plus(value)))
                .collect();
            url.push('?');
            url.push_str(&encoded.join("&"));
        }
        url
    }
}

/// urllib.parse.quote_plus: unreserved stays, space becomes '+', everything
/// else percent-encodes per UTF-8 byte (uppercase hex).
fn quote_plus(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
