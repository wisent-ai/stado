//! The shared atomic rate-limit consume route.

use serde_json::json;

use crate::rate_limit::{self, ConsumeRequest, RateLimitError};

use crate::dashboard::listener::{http_status, send_json, Dashboard, Request, Response};

impl Dashboard {
    pub(crate) async fn post_rate_limit_consume(&self, request: &Request) -> Response {
        let content_type = request
            .header("content-type")
            .and_then(|value| value.split(';').next())
            .map(str::trim);
        if content_type != Some("application/json") {
            return send_json(
                http_status(reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE),
                &json!({"error": "content-type must be application/json"}),
            );
        }
        let Some(supplied) = crate::dashboard::listener::auth::presented_bearer(request) else {
            return send_json(
                http_status(reqwest::StatusCode::UNAUTHORIZED),
                &json!({"error": "unauthorized"}),
            );
        };
        let client = match rate_limit::authenticate(supplied).await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return send_json(
                    http_status(reqwest::StatusCode::UNAUTHORIZED),
                    &json!({"error": "unauthorized"}),
                )
            }
            // The cause is said: "rate limiting unavailable" alone left the
            // caller nothing to repair.
            Err(error) => {
                return send_json(
                    http_status(reqwest::StatusCode::SERVICE_UNAVAILABLE),
                    &json!({"error": format!("rate limiting unavailable: {error}")}),
                )
            }
        };
        let payload = match serde_json::from_slice::<ConsumeRequest>(&request.body) {
            Ok(payload) => payload,
            Err(error) => {
                return send_json(
                    http_status(reqwest::StatusCode::BAD_REQUEST),
                    &json!({"error": format!("invalid rate-limit request: {error}")}),
                )
            }
        };
        match self.rate_limiter.consume(client, &payload).await {
            Ok(response) => send_json(http_status(reqwest::StatusCode::OK), &json!(response)),
            Err(RateLimitError::InvalidRequest(message)) => send_json(
                http_status(reqwest::StatusCode::BAD_REQUEST),
                &json!({"error": message}),
            ),
            // Configuration, storage, Skarbiec and persisted-state failures
            // are the limiter's, not the caller's; the answer names which.
            Err(unavailable) => send_json(
                http_status(reqwest::StatusCode::SERVICE_UNAVAILABLE),
                &json!({"error": format!("rate limiting unavailable: {unavailable}")}),
            ),
        }
    }
}
