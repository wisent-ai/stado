//! The request path: bounded binary bodies and JSON envelopes.
//!
//! Python `request(..., binary=True)` and `request(..., binary=False)`
//! over the shared `send` that resolves the bearer token, applies the
//! per-request timeout, and bounds the response.

use serde_json::{Map, Value};

use super::super::types::{BoxError, MAX_JSON_BYTES};
use super::response::{api_error, parse_json, read_bounded, transport_error};
use super::BoxHttpTransport;

impl BoxHttpTransport {
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
    /// error statuses to [`BoxApiError`](super::super::BoxApiError) and
    /// network failures to [`BoxError::Transport`].
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
}
