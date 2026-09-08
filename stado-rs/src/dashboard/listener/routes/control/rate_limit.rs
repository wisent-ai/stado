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
                http_status("415"),
                &json!({"error": "content-type must be application/json"}),
            );
        }
        let supplied = request
            .header("authorization")
            .and_then(|value| value.trim().strip_prefix("Bearer "))
            .unwrap_or_default();
        let client = match rate_limit::authenticate(supplied).await {
            Ok(Some(client)) => client,
            Ok(None) => return send_json(http_status("401"), &json!({"error": "unauthorized"})),
            Err(_) => {
                return send_json(
                    http_status("503"),
                    &json!({"error": "rate limiting unavailable"}),
                )
            }
        };
        let payload = match serde_json::from_slice::<ConsumeRequest>(&request.body) {
            Ok(payload) => payload,
            Err(_) => {
                return send_json(
                    http_status("400"),
                    &json!({"error": "invalid rate-limit request"}),
                )
            }
        };
        match self.rate_limiter.consume(client, &payload).await {
            Ok(response) => send_json(http_status("200"), &json!(response)),
            Err(RateLimitError::InvalidRequest(message)) => {
                send_json(http_status("400"), &json!({"error": message}))
            }
            Err(_) => send_json(
                http_status("503"),
                &json!({"error": "rate limiting unavailable"}),
            ),
        }
    }
}
