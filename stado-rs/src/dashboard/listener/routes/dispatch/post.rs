//! `POST`: the Host guard, the enrollment write, and the boundary and
//! operator-action gate every control write answers behind.

use serde_json::json;

use crate::machine::MachineError;

use crate::dashboard::listener::auth::authorize_service;
use crate::dashboard::listener::routes::control::{machine_result_response, service_name};
use crate::dashboard::listener::{
    empty_response, http_status, send_json, Boundary, Dashboard, Request, Response,
};
use crate::dashboard::{fleet_join, operator_console, registry_policy};

impl Dashboard {
    pub(crate) async fn do_post(&self, request: &Request) -> Response {
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        let control_route = path == "/api/machine/submit"
            || path == "/api/machine/cancel"
            || path == "/api/service/restart"
            || path == "/api/rate-limit/consume";
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            if matches!(path, "/api/machine/submit" | "/api/machine/cancel") {
                return machine_result_response(Err(MachineError::new("FORBIDDEN", "forbidden")));
            }
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        // Enrollment by invite: authorized by the invite token alone, before
        // any operator authorization is reached, and never by it.
        if path == "/api/fleet/join" {
            return fleet_join::join(&self.store, request).await;
        }
        if path == "/api/object/compose" {
            return self.post_object_compose(request).await;
        }
        if path == "/api/operator/run" {
            return operator_console::handle(request).await;
        }
        let required: &[Boundary] = match path {
            "/api/rate-limit/consume" => &[Boundary::RateLimitVerifier, Boundary::RateLimitState],
            "/api/machine/submit" | "/api/machine/cancel" => &[Boundary::Machine],
            "/api/service/restart" => &[Boundary::Service],
            _ => &[],
        };
        let unavailable = !self.boundaries_available(required).await;
        if unavailable {
            if matches!(path, "/api/machine/submit" | "/api/machine/cancel") {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )));
            }
            return send_json(
                http_status("503"),
                &json!({"error": "authorization boundary unavailable"}),
            );
        }
        if path == "/api/rate-limit/consume" {
            return self.post_rate_limit_consume(request).await;
        }
        // Registry adoption, policy writes, cleanup and convergence each require
        // their own operator action. Convergence can replace every declared
        // binary on a host; a read or one service's deployer grant is insufficient.
        if matches!(
            path,
            "/api/registry/import"
                | "/api/registry/policy"
                | "/api/cleanup/run"
                | "/api/service/converge"
                | "/api/host/storage-root-reconcile"
        ) {
            if !self.boundaries_available(&[Boundary::Registry]).await {
                return send_json(
                    http_status("503"),
                    &json!({"error": "registry authorization unavailable"}),
                );
            }
            let action = match path {
                "/api/registry/import" => "registry-import",
                "/api/registry/policy" => "policy-write",
                "/api/service/converge" => "converge-apply",
                "/api/host/storage-root-reconcile" => "storage-reconcile-apply",
                _ => "cleanup-run",
            };
            if let Err(response) = registry_policy::authorized(request, action).await {
                return response;
            }
            return match path {
                "/api/registry/import" => registry_policy::import_registry(request).await,
                "/api/registry/policy" => registry_policy::set_policy(request).await,
                "/api/service/converge" => self.service_converge(request, query, true).await,
                "/api/host/storage-root-reconcile" => {
                    self.storage_root_reconcile(request, query, true).await
                }
                _ => registry_policy::run_cleanup().await,
            };
        }
        if control_route {
            if path == "/api/service/restart" {
                let service = match service_name(query) {
                    Ok(service) => service,
                    Err(response) => return response,
                };
                match authorize_service(request, service, "restart").await {
                    Ok(true) => {}
                    Ok(false) => {
                        return send_json(http_status("401"), &json!({"error": "unauthorized"}))
                    }
                    Err(()) => {
                        return send_json(
                            http_status("503"),
                            &json!({"error": "service authorization unavailable"}),
                        )
                    }
                }
                return self.post_service_restart(request, query).await;
            }
            return if path == "/api/machine/submit" {
                self.post_machine_submit(request).await
            } else {
                self.post_machine_cancel(request, query).await
            };
        }
        empty_response(http_status("404"), "Not Found")
    }
}
