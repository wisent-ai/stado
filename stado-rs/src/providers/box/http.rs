//! Bounded HTTP transport for Box Public API v1.
//!
//! Port of `stado/providers/box/_http.py`. The Python class builds on
//! urllib with an injectable opener; here the transport wraps reqwest with
//! the same contract: HTTPS-only base URL, Bearer auth, per-request
//! timeout, bounded response reads, `ok=true` + response-type validation,
//! and structured redacted errors for non-success statuses.
//!
//! Divergence note: reqwest reads the body via `chunk()` accumulation with
//! the same `limit + 1` cut as Python's `response.read(limit + 1)`, so the
//! "response exceeded configured size bound" failure is identical while
//! memory stays bounded. Transport-error kinds are reqwest categories
//! ("timeout" / "connect_error" / "transport_error") rather than Python
//! exception class names ("TimeoutError" / "URLError" / ...).

use std::time::Duration;

use serde_json::{Map, Value};

use crate::queue::gcs::percent_encode;

use super::types::{
    first_truthy_str, required_dict, safe_text, BoxApiError, BoxError, DEFAULT_BOX_API_URL,
    DEFAULT_TIMEOUT_SECONDS, MAX_JSON_BYTES, TRANSIENT_HTTP,
};

/// Validated transport with no construction-time requests (Python
/// `BoxHTTPTransport`).
#[derive(Debug, Clone)]
pub struct BoxHttpTransport {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    timeout: Duration,
}

impl BoxHttpTransport {
    /// Python `BoxHTTPTransport.__init__`: strip the key, require it, and
    /// pin the base URL to an HTTPS API base without query or fragment.
    pub fn new(api_key: &str, base_url: &str, timeout_seconds: f64) -> Result<Self, BoxError> {
        let key = api_key.trim();
        if key.is_empty() {
            return Err(BoxError::configuration(
                "BOX_API_KEY is required for Box provider",
            ));
        }
        let base_url = base_url.trim_end_matches('/');
        let parsed = url::Url::parse(base_url).map_err(|_| {
            BoxError::configuration(
                "BOX_API_URL must be an HTTPS API base without query or fragment",
            )
        })?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none_or(|h| h.is_empty())
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(BoxError::configuration(
                "BOX_API_URL must be an HTTPS API base without query or fragment",
            ));
        }
        if timeout_seconds <= 0.0 {
            return Err(BoxError::configuration(
                "Box request timeout must be positive",
            ));
        }
        // Rebuild scheme://netloc + path without trailing slash (Python
        // urlunsplit((scheme, netloc, path.rstrip("/"), "", ""))).
        let mut normalized = format!(
            "{}://{}",
            parsed.scheme(),
            parsed.host_str().unwrap_or_default()
        );
        if let Some(port) = parsed.port() {
            normalized.push_str(&format!(":{port}"));
        }
        normalized.push_str(parsed.path().trim_end_matches('/'));
        Ok(Self::assemble(key, &normalized, timeout_seconds))
    }

    /// Test-only constructor: same wiring, without the HTTPS scheme check,
    /// so a loopback mock can stand in for ascii.dev.
    fn assemble(api_key: &str, base_url: &str, timeout_seconds: f64) -> Self {
        BoxHttpTransport {
            client: reqwest::Client::new(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            timeout: Duration::from_secs_f64(timeout_seconds),
        }
    }

    /// The validated base URL (`https://host[/path]` without trailing slash).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

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

    /// Python `request(..., binary=True)`: bounded raw body, no ok/type
    /// validation (artifacts are not JSON envelopes).
    pub async fn request_binary(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, String)],
        max_bytes: usize,
    ) -> Result<Vec<u8>, BoxError> {
        self.send(method, path, None, query, max_bytes).await
    }

    /// Python `request(..., binary=False)`: bounded JSON envelope with the
    /// `ok=true` and expected-`type` contract enforced.
    pub async fn request_json(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        query: &[(&str, String)],
        expected_types: &[&str],
    ) -> Result<Map<String, Value>, BoxError> {
        let raw = self.send(method, path, body, query, MAX_JSON_BYTES).await?;
        let payload = parse_json(&raw)?;
        if payload.get("ok") != Some(&Value::Bool(true)) {
            return Err(BoxError::transport("Box success response lacks ok=true"));
        }
        if !expected_types.is_empty()
            && !expected_types.contains(&payload.get("type").and_then(Value::as_str).unwrap_or(""))
        {
            return Err(BoxError::transport("Box response has an unexpected type"));
        }
        Ok(payload)
    }

    /// Execute the request and return the bounded raw body, mapping HTTP
    /// error statuses to [`BoxApiError`] and network failures to
    /// [`BoxError::Transport`].
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        query: &[(&str, String)],
        max_bytes: usize,
    ) -> Result<Vec<u8>, BoxError> {
        let api_key = if self.api_key.is_empty() {
            crate::skarbiec::read_string("stado-box", "api_key")
                .await
                .map_err(|err| BoxError::configuration(err.to_string()))?
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    BoxError::configuration(
                        "Skarbiec item stado-box field api_key is required for Box provider",
                    )
                })?
        } else {
            self.api_key.clone()
        };
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| BoxError::value(format!("invalid HTTP method {method:?}")))?;
        let mut request = self
            .client
            .request(method, self.url(path, query))
            .timeout(self.timeout)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = body {
            // Python json.dumps(body, separators=(",", ":")) — compact.
            let data = serde_json::to_vec(body).map_err(|err| BoxError::value(err.to_string()))?;
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(data);
        }
        let response = request.send().await.map_err(transport_error)?;
        let status = response.status().as_u16();
        let raw = read_bounded(response, max_bytes).await?;
        if !(200..300).contains(&status) {
            return Err(api_error(status, &raw));
        }
        Ok(raw)
    }
    /// Build a transport whose bearer token is resolved from
    /// `stado-box/api_key` in Skarbiec on each request.
    pub fn from_skarbiec(base_url: &str, timeout_seconds: f64) -> Result<Self, BoxError> {
        let mut transport = Self::new("skarbiec", base_url, timeout_seconds)?;
        transport.api_key.clear();
        Ok(transport)
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

/// Python `_read_bounded`: fail once the body exceeds the limit.
async fn read_bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, BoxError> {
    let mut raw: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        raw.extend_from_slice(&chunk);
        if raw.len() > limit {
            return Err(BoxError::transport(
                "Box response exceeded configured size bound",
            ));
        }
    }
    Ok(raw)
}

/// Python `_parse_json`: empty body -> {}; non-object or invalid JSON is a
/// transport failure.
fn parse_json(raw: &[u8]) -> Result<Map<String, Value>, BoxError> {
    if raw.is_empty() {
        return Ok(Map::new());
    }
    let text =
        std::str::from_utf8(raw).map_err(|_| BoxError::transport("Box returned invalid JSON"))?;
    let value: Value =
        serde_json::from_str(text).map_err(|_| BoxError::transport("Box returned invalid JSON"))?;
    required_dict(value, "JSON")
}

/// Python `_raise_http_error`: over-limit error bodies are discarded, then
/// code/message fall back payload -> nested error object -> defaults.
fn api_error(status: u16, raw: &[u8]) -> BoxError {
    let payload: Map<String, Value> = if raw.is_empty() {
        Map::new()
    } else {
        std::str::from_utf8(raw)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
    };
    let nested = payload.get("error").and_then(Value::as_object);
    let code = first_truthy_str(
        &[payload.get("code"), nested.and_then(|e| e.get("code"))],
        "http_error",
    );
    let message = first_truthy_str(
        &[
            payload.get("message"),
            nested.and_then(|e| e.get("message")),
        ],
        "Box API request failed",
    );
    let request_id = first_truthy_str(&[payload.get("requestId")], "");
    BoxApiError::new(
        status,
        &code,
        &message,
        &request_id,
        TRANSIENT_HTTP.contains(&status),
    )
    .into()
}

/// Python `except (URLError, TimeoutError, socket.timeout, OSError)`:
/// redacted, class-name-only transport failure.
fn transport_error(err: reqwest::Error) -> BoxError {
    let kind = if err.is_timeout() {
        "timeout"
    } else if err.is_connect() {
        "connect_error"
    } else {
        "transport_error"
    };
    BoxError::transport(format!(
        "Box transport failed: {}",
        safe_text(kind, "transport_error")
    ))
}

/// Default timeout used by [`super::BoxProvider::from_env`] — re-exported
/// for callers that don't go through env resolution.
pub const DEFAULT_TIMEOUT: f64 = DEFAULT_TIMEOUT_SECONDS;
/// Default base URL (re-exported so `super::BoxProvider` mirrors the
/// Python constructor defaults).
pub const DEFAULT_BASE_URL: &str = DEFAULT_BOX_API_URL;
