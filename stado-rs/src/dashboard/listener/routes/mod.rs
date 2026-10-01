//! The routing table: the integration group and the method gates
//! ([`dispatch`]), the object data plane ([`object`]), the control plane
//! ([`control`]), and the desktop update channel ([`sparkle`]).

mod control;
mod dispatch;
mod object;
mod sparkle;

use serde_json::json;

use crate::dashboard::integration;

use super::{empty_response, http_status, send_json, Boundary, Dashboard, Request, Response};

impl Dashboard {
    pub(crate) async fn route(&self, request: &Request) -> Response {
        let path = request.path.split('?').next().unwrap_or("");
        if path.starts_with("/api/integration/") {
            if !self
                .trusted_request_host(request.header("host"), request.header("x-forwarded-proto"))
            {
                return send_json(http_status(reqwest::StatusCode::FORBIDDEN), &json!({"error": "forbidden"}));
            }
            let available = self.boundaries_available(&[Boundary::Integration]).await;
            return integration::handle(request, available, &self.store).await;
        }
        match request.method.as_str() {
            "" => empty_response(400, "Bad Request"),
            "GET" => self.do_get(request).await,
            "POST" => self.do_post(request).await,
            "PUT" => self.do_put(request).await,
            "DELETE" => self.do_delete(request).await,
            // Python BaseHTTPRequestHandler: 501 Unsupported method.
            _ => empty_response(501, "Not Implemented"),
        }
    }
}
