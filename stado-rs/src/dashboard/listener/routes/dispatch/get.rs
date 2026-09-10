//! `GET`: the Host guard, liveness and operator state, the enrollment reads,
//! and the boundary and bearer gate every other read answers behind.

use serde_json::json;

use crate::machine::MachineError;

use crate::dashboard::listener::auth::{authorize_object, authorize_release, authorize_service};
use crate::dashboard::listener::boundary::boundary_plan;
use crate::dashboard::listener::routes::control::{machine_result_response, service_name};
use crate::dashboard::listener::routes::object::{object_from_query, object_list_from_query};
use crate::dashboard::listener::{
    dashboard_error_response, http_status, send_json, Boundary, Dashboard, Request, Response,
};
use crate::dashboard::{fleet_join, registry_policy};

impl Dashboard {
    pub(crate) async fn do_get(&self, request: &Request) -> Response {
        let path_no_query = request.path.split('?').next().unwrap_or("");
        // The immutable release channel is the recovery root for every other
        // boundary, so its exact read-only route must not consult host trust or
        // Skarbiec-backed authorization. Namespace validation remains in
        // `get_routes`; no list, mutation, or other object route enters here.
        if path_no_query == "/api/release/object" {
            return match self.get_routes(request).await {
                Ok(response) => response,
                Err(error) => dashboard_error_response(error),
            };
        }
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            if path_no_query == "/api/machine/status" {
                return machine_result_response(Err(MachineError::new("FORBIDDEN", "forbidden")));
            }
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        if path_no_query == crate::dashboard::operator_console::stream::PATH {
            return crate::dashboard::operator_console::stream::upgrade(request).await;
        }
        if path_no_query == "/healthz" || path_no_query == "/livez" {
            // Liveness answers before authorization, so it publishes the flat
            // readiness booleans only: a boundary's reason names vault items,
            // grants and endpoints, and an unauthenticated probe has no
            // business reading those. The startup log carries the reason.
            let (degraded, boundaries) = {
                let boundaries = self
                    .boundaries
                    .read()
                    .expect("dashboard boundary state lock");
                (!boundaries.all_ready(), boundaries.ready_json())
            };
            return send_json(
                http_status("200"),
                &json!({
                    "ok": true,
                    "degraded": degraded,
                    "boundaries": boundaries,
                }),
            );
        }
        // The operator's read of the same state, with each boundary's reason.
        //
        // `/healthz` cannot carry it and should not, and until this route
        // existed the sentence was reachable nowhere on a live process: the
        // verdict is held in memory, the holder logs no boundary line unless
        // it revalidates, and the standing remedy points at a unit log that on
        // one host does not exist. So a closed boundary was one bit, and one
        // bit cannot distinguish `validation did not settle within N seconds`
        // — arithmetic, answered by the item budget — from `item set mismatch`
        // or `missing or empty`, which is a credential answer and is not fixed
        // by restarting anything.
        //
        // Loopback-only, like every route on this listener, and it publishes
        // the verifier's own sentence rather than any material: what refused
        // and about which subject.
        if path_no_query == "/api/state.json" {
            let (degraded, boundaries) = {
                let boundaries = self
                    .boundaries
                    .read()
                    .expect("dashboard boundary state lock");
                (!boundaries.all_ready(), boundaries.state_json())
            };
            let write_fence = self
                .store
                .local_storage_path()
                .filter(|_| self.store.backend_name() == "local")
                .map(|root| {
                    crate::queue::LocalBackend::write_fence_state(std::path::Path::new(root))
                        .unwrap_or_else(|error| json!({"error": error.to_string()}))
                });
            return send_json(
                http_status("200"),
                &json!({
                    "degraded": degraded,
                    "boundaries": boundaries,
                    "storage": {
                        "pid": std::process::id(),
                        "version": env!("CARGO_PKG_VERSION"),
                        "backend": self.store.backend_name(),
                        "local_path": self.store.local_storage_path(),
                        "write_fence": write_fence,
                        "backup": self.store.backup_endpoint().map(|endpoint| json!({
                            "backend": endpoint.kind,
                            "local_path": (endpoint.adapter()
                                == Some(crate::capabilities::StorageAdapter::Local))
                                .then_some(endpoint.path.as_str()),
                        })),
                    },
                }),
            );
        }
        // Enrollment by invite. Both answer before any operator
        // authorization is consulted, and neither ever consults it: the
        // machine holds an invite code and nothing else, and a loopback
        // caller's implicit operator trust must not become a way in.
        if path_no_query == "/join.sh" {
            return fleet_join::join_script();
        }
        if path_no_query == "/api/fleet/invite/key" {
            return fleet_join::invite_key(&self.store, request).await;
        }
        let object_route = path_no_query == "/api/object"
            || path_no_query == "/api/object/list"
            || path_no_query == "/api/object/stat";
        if object_route {
            let query = request
                .path
                .split_once('?')
                .map(|(_, query)| query)
                .unwrap_or("");
            let scope = if path_no_query == "/api/object/list" {
                object_list_from_query(query).map(|(namespace, prefix)| (namespace, prefix, true))
            } else {
                object_from_query(query).map(|object| {
                    (
                        object.namespace().to_string(),
                        object.key().to_string(),
                        false,
                    )
                })
            };
            let (namespace, key_or_prefix, list) = match scope {
                Ok(scope) => scope,
                Err(response) => return response,
            };
            // Same rule as the writer above, and the same split: a release
            // coordinate revalidates the release boundary, which is what
            // gives it a way to reopen, without being gated by it.
            let plan = boundary_plan(path_no_query, Some((&namespace, &key_or_prefix)));
            if !self.satisfy_boundaries(&plan).await {
                return send_json(
                    http_status("503"),
                    &json!({"error": "object authorization unavailable"}),
                );
            }
            let action = if list {
                "list"
            } else if path_no_query == "/api/object/stat" {
                "stat"
            } else {
                "get"
            };
            let authorized = if let Some(policy_key) =
                crate::object_store::release_policy_key(&namespace, &key_or_prefix)
            {
                // A catalog object is addressed exactly, never listed as a prefix.
                let listing = list && namespace != "system";
                authorize_release(self, request, &policy_key, listing).await
            } else {
                authorize_object(self, request, &namespace, &key_or_prefix, list, action).await
            };
            match authorized {
                Ok(None) => {}
                Ok(Some(reason)) => {
                    return send_json(
                        http_status("401"),
                        &json!({"error": "unauthorized", "reason": reason}),
                    )
                }
                Err(()) => {
                    return send_json(
                        http_status("503"),
                        &json!({"error": "object authorization unavailable"}),
                    )
                }
            }
        } else {
            // At most one of these paths matches, so at most one boundary is
            // ever consulted here.
            if path_no_query == "/api/service/status"
                && !self.boundaries_available(&[Boundary::Service]).await
            {
                return send_json(
                    http_status("503"),
                    &json!({"error": "service authorization unavailable"}),
                );
            }
            if path_no_query == "/api/machine/status"
                && !self.boundaries_available(&[Boundary::Machine]).await
            {
                return machine_result_response(Err(MachineError::retryable(
                    "AUTH_UNAVAILABLE",
                    "machine authorization unavailable",
                )));
            }
            if matches!(
                path_no_query,
                "/api/host/inventory"
                    | "/api/service/converge"
                    | "/api/host/storage-root-reconcile"
            ) {
                if !self.boundaries_available(&[Boundary::Registry]).await {
                    return send_json(
                        http_status("503"),
                        &json!({"error": "registry authorization unavailable"}),
                    );
                }
                let action = match path_no_query {
                    "/api/service/converge" => "converge-read",
                    "/api/host/storage-root-reconcile" => "storage-reconcile-read",
                    _ => "policy-read",
                };
                if let Err(response) = registry_policy::authorized(request, action).await {
                    return response;
                }
            }
            if path_no_query == "/api/service/status" {
                let query = request
                    .path
                    .split_once('?')
                    .map(|(_, query)| query)
                    .unwrap_or("");
                let service = match service_name(query) {
                    Ok(service) => service,
                    Err(response) => return response,
                };
                match authorize_service(request, service, "status").await {
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
            }
        }
        match self.get_routes(request).await {
            Ok(response) => response,
            Err(error) => dashboard_error_response(error),
        }
    }
}
