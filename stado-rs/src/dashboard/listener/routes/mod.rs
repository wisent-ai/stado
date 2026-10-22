//! The routing table: the integration group and the method gates
//! ([`dispatch`]), the object data plane ([`object`]), the S3 API over the
//! same objects ([`s3`]), the control plane ([`control`]), and the desktop
//! update channel ([`sparkle`]).

mod control;
mod dispatch;
mod object;
mod s3;
mod sparkle;

pub(crate) use s3::is_s3_route;

use serde_json::json;

use crate::dashboard::integration;

use super::{empty_response, http_status, send_json, Boundary, Dashboard, Request, Response};

impl Dashboard {
    pub(crate) async fn route(&self, request: &Request) -> Response {
        let (path, _) = request.target();
        // The S3 API answers in its own vocabulary, HEAD included, and
        // authorizes by signature rather than by bearer, so it is routed
        // before the method gates every other route answers behind.
        if is_s3_route(path) {
            return self.s3(request).await;
        }
        if path.starts_with("/api/integration/") {
            if !self
                .trusted_request_host(request.header("host"), request.header("x-forwarded-proto"))
            {
                return send_json(
                    http_status(reqwest::StatusCode::FORBIDDEN),
                    &json!({"error": "forbidden"}),
                );
            }
            let available = self.boundaries_available(&[Boundary::Integration]).await;
            return integration::handle(request, available, &self.store).await;
        }
        // The reader refuses a request line without a method, so every
        // method reaching here is a token; one no route serves is 501
        // (RFC 9110 §15.6.2).
        match request.method.as_str() {
            "GET" => self.do_get(request).await,
            "POST" => self.do_post(request).await,
            "PUT" => self.do_put(request).await,
            "DELETE" => self.do_delete(request).await,
            _ => empty_response(
                http_status(reqwest::StatusCode::NOT_IMPLEMENTED),
                "Not Implemented",
            ),
        }
    }
}
