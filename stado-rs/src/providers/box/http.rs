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
    #[cfg(test)]
    pub(crate) fn new_for_test(api_key: &str, base_url: &str, timeout_seconds: f64) -> Self {
        Self::assemble(api_key, base_url.trim_end_matches('/'), timeout_seconds)
    }

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
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| BoxError::value(format!("invalid HTTP method {method:?}")))?;
        let mut request = self
            .client
            .request(method, self.url(path, query))
            .timeout(self.timeout)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.api_key),
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{http_response, mock_http};

    #[test]
    fn constructor_validation_matches_python() {
        let err = BoxHttpTransport::new("  ", DEFAULT_BOX_API_URL, 70.0).unwrap_err();
        assert!(err.to_string().contains("BOX_API_KEY is required"), "{err}");

        let err = BoxHttpTransport::new("k", "http://ascii.dev/api", 70.0).unwrap_err();
        assert!(
            err.to_string().contains("must be an HTTPS API base"),
            "{err}"
        );

        let err = BoxHttpTransport::new("k", "https://ascii.dev/api?x=1", 70.0).unwrap_err();
        assert!(
            err.to_string().contains("must be an HTTPS API base"),
            "{err}"
        );

        let err = BoxHttpTransport::new("k", "https://ascii.dev/api#frag", 70.0).unwrap_err();
        assert!(
            err.to_string().contains("must be an HTTPS API base"),
            "{err}"
        );

        let err = BoxHttpTransport::new("k", "not a url", 70.0).unwrap_err();
        assert!(
            err.to_string().contains("must be an HTTPS API base"),
            "{err}"
        );

        let err = BoxHttpTransport::new("k", DEFAULT_BOX_API_URL, 0.0).unwrap_err();
        assert!(
            err.to_string().contains("timeout must be positive"),
            "{err}"
        );

        // Trailing slashes normalize away.
        let t = BoxHttpTransport::new(" k ", "https://ascii.dev/api/box/v1/", 70.0).unwrap();
        assert_eq!(t.base_url(), "https://ascii.dev/api/box/v1");
        // Default base URL constant matches Python.
        assert_eq!(DEFAULT_BASE_URL, "https://ascii.dev/api/box/v1");
    }

    #[test]
    fn url_building_quotes_segments_and_query() {
        let t = BoxHttpTransport::new("k", "https://ascii.dev/api/box/v1", 70.0).unwrap();
        assert_eq!(t.url("/limits", &[]), "https://ascii.dev/api/box/v1/limits");
        // Each segment is quoted with an empty safe set; inner empty
        // segments are dropped (Python _url).
        assert_eq!(
            t.url(
                "/boxes/bx_2abcdefg/files",
                &[("path", "/tmp/a b.txt".to_string())]
            ),
            "https://ascii.dev/api/box/v1/boxes/bx_2abcdefg/files?path=%2Ftmp%2Fa+b.txt"
        );
        assert_eq!(
            t.url("//boxes//", &[]),
            "https://ascii.dev/api/box/v1/boxes"
        );
        // Empty query values are dropped.
        assert_eq!(
            t.url(
                "/boxes",
                &[("cursor", String::new()), ("sort", "asc".to_string())]
            ),
            "https://ascii.dev/api/box/v1/boxes?sort=asc"
        );
        // Root path returns the bare base.
        assert_eq!(t.url("/", &[]), "https://ascii.dev/api/box/v1");
    }

    fn transport(server: &crate::testutil::MockHttp) -> BoxHttpTransport {
        BoxHttpTransport::new_for_test("box_testkey", &server.base_url, 5.0)
    }

    #[tokio::test]
    async fn get_sends_bearer_and_parses_envelope() {
        let server = mock_http(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "limits.info", "canStart": true}"#,
        )])
        .await;
        let payload = transport(&server)
            .request_json("GET", "/limits", None, &[], &["limits.info"])
            .await
            .unwrap();
        assert_eq!(payload["canStart"], Value::Bool(true));
        let requests = server.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].starts_with("GET /limits HTTP/1.1\r\n"),
            "{}",
            requests[0]
        );
        assert!(
            requests[0].contains("authorization: Bearer box_testkey"),
            "{}",
            requests[0]
        );
        assert!(
            requests[0].contains("accept: application/json"),
            "{}",
            requests[0]
        );
        server.stop();
    }

    #[tokio::test]
    async fn post_body_is_compact_json() {
        let server = mock_http(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.created"}"#,
        )])
        .await;
        let body = serde_json::json!({"ttlSeconds": 7200, "noEnv": true});
        transport(&server)
            .request_json("POST", "/boxes", Some(&body), &[], &["box.created"])
            .await
            .unwrap();
        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests[0].contains("content-type: application/json"),
            "{}",
            requests[0]
        );
        assert!(
            requests[0].ends_with(r#"{"ttlSeconds":7200,"noEnv":true}"#),
            "compact separators: {}",
            requests[0]
        );
        server.stop();
    }

    #[tokio::test]
    async fn envelope_contract_is_enforced() {
        // ok missing.
        let server = mock_http(vec![http_response(200, "OK", r#"{"type": "limits.info"}"#)]).await;
        let err = transport(&server)
            .request_json("GET", "/limits", None, &[], &["limits.info"])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("lacks ok=true"), "{err}");
        server.stop();

        // Wrong type.
        let server = mock_http(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "other"}"#,
        )])
        .await;
        let err = transport(&server)
            .request_json("GET", "/limits", None, &[], &["limits.info"])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unexpected type"), "{err}");
        server.stop();

        // Invalid JSON.
        let server = mock_http(vec![http_response(200, "OK", "not json")]).await;
        let err = transport(&server)
            .request_json("GET", "/limits", None, &[], &[])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid JSON"), "{err}");
        server.stop();

        // Non-object JSON.
        let server = mock_http(vec![http_response(200, "OK", "[1,2]")]).await;
        let err = transport(&server)
            .request_json("GET", "/limits", None, &[], &[])
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("Box JSON response is not an object"),
            "{err}"
        );
        server.stop();
    }

    #[tokio::test]
    async fn http_error_maps_to_structured_redacted_api_error() {
        let server = mock_http(vec![http_response(
            429,
            "Too Many Requests",
            r#"{"code": "rate_limited", "message": "slow down box_secret9 ?token=abc", "requestId": "req-7"}"#,
        )])
        .await;
        let err = transport(&server)
            .request_json("GET", "/limits", None, &[], &[])
            .await
            .unwrap_err();
        let BoxError::Api(api) = err else {
            panic!("expected Api error: {err:?}")
        };
        assert_eq!(api.status, 429);
        assert!(api.retryable);
        assert_eq!(api.code, "rate_limited");
        assert_eq!(api.message, "slow down [REDACTED] ?token=[REDACTED]");
        assert_eq!(api.request_id, "req-7");
        server.stop();

        // Nested error object + non-retryable status + defaults.
        let server = mock_http(vec![http_response(
            404,
            "Not Found",
            r#"{"error": {"code": "box_not_found", "message": "gone"}}"#,
        )])
        .await;
        let err = transport(&server)
            .request_json("GET", "/boxes/bx_2abcdefg", None, &[], &[])
            .await
            .unwrap_err();
        let BoxError::Api(api) = err else {
            panic!("expected Api error: {err:?}")
        };
        assert_eq!(api.status, 404);
        assert!(!api.retryable);
        // "box_not_found" matches the box-key redaction pattern (Python
        // redacts it too) — it becomes "[REDACTED]" in the stored code.
        assert_eq!(api.code, "[REDACTED]");
        assert_eq!(api.message, "gone");
        server.stop();

        // Non-JSON error body -> defaults.
        let server = mock_http(vec![http_response(503, "Unavailable", "upstream broken")]).await;
        let err = transport(&server)
            .request_json("GET", "/limits", None, &[], &[])
            .await
            .unwrap_err();
        let BoxError::Api(api) = err else {
            panic!("expected Api error: {err:?}")
        };
        assert_eq!(api.code, "http_error");
        assert_eq!(api.message, "Box API request failed");
        assert!(api.retryable);
        server.stop();
    }

    #[tokio::test]
    async fn response_size_bound_is_enforced() {
        let big = "x".repeat(MAX_JSON_BYTES + 10);
        let server = mock_http(vec![http_response(200, "OK", &big)]).await;
        let err = transport(&server)
            .request_json("GET", "/limits", None, &[], &[])
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("exceeded configured size bound"),
            "{err}"
        );
        server.stop();
    }

    #[tokio::test]
    async fn binary_requests_skip_envelope_validation() {
        let server = mock_http(vec![http_response(200, "OK", "raw-bytes-not-json")]).await;
        let bytes = transport(&server)
            .request_binary(
                "GET",
                "/boxes/bx_2abcdefg/artifacts",
                &[("path", "/a".to_string())],
                1024,
            )
            .await
            .unwrap();
        assert_eq!(bytes, b"raw-bytes-not-json");
        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests[0].starts_with("GET /boxes/bx_2abcdefg/artifacts?path=%2Fa "),
            "{}",
            requests[0]
        );
        server.stop();
    }

    #[tokio::test]
    async fn connect_failure_is_a_redacted_transport_error() {
        // Bind then drop a listener so the port is closed.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let t = BoxHttpTransport::new_for_test("k", &format!("http://{addr}"), 2.0);
        let err = t
            .request_json("GET", "/limits", None, &[], &[])
            .await
            .unwrap_err();
        assert!(
            err.to_string().starts_with("Box transport failed: "),
            "{err}"
        );
    }
}
