//! The `GET` route table: which read each authorized path resolves to.

use serde_json::json;

use crate::dashboard::listener::routes::control::host_inventory_target;
use crate::dashboard::listener::routes::object::public_release_object_from_query;
use crate::dashboard::listener::{
    empty_response, http_status, send_json, Boundary, Dashboard, Request, Response,
};
use crate::dashboard::registry_policy;
use crate::dashboard::DashboardError;

impl Dashboard {
    pub(crate) async fn get_routes(&self, request: &Request) -> Result<Response, DashboardError> {
        let (path, query) = match request.path.split_once('?') {
            Some((path, query)) => (path, query),
            None => (request.path.as_str(), ""),
        };
        if path == "/api/host/storage-root-reconcile" {
            return Ok(self.storage_root_reconcile(request, query, false).await);
        }
        if path == "/api/service/converge" {
            return Ok(self.service_converge(request, query, false).await);
        }
        if path == "/api/host/inventory" {
            let target = match host_inventory_target(query) {
                Ok(target) => target,
                Err(response) => return Ok(response),
            };
            let runner = crate::deploy::production_runner();
            return Ok(
                match crate::deploy::host_inventory::inventory_host(&target, &runner).await {
                    Ok(report) => send_json(http_status("200"), &report),
                    Err(error) => send_json(
                        http_status("503"),
                        &json!({
                            "target": target,
                            "status": crate::deploy::host_channel::FAILED_STATUS,
                            "error": error.to_string(),
                        }),
                    ),
                },
            );
        }
        if path == "/api/release/object" {
            // Public read-only release channel. This dashboard route is the
            // store's delivery endpoint; an operator-owned TLS reverse proxy
            // may expose it off-host without changing its response contract:
            //   200: the object bytes, Content-Type from object metadata
            //        (default application/octet-stream), Accept-Ranges: bytes
            //   206/416: byte-range answers from get_object
            //   400: {"error": ...} — query is not exactly one valid uri
            //   403: {"error": ...} — a namespace that is not `releases`
            //   404: {"state": "absent", "uri": ...} — the object is missing
            let object = match public_release_object_from_query(query) {
                Ok(object) => object,
                Err(response) => return Ok(response),
            };
            if object.namespace() != "releases" {
                return Ok(send_json(
                    http_status("403"),
                    &json!({"error": "only stado://releases software artifacts are publicly readable"}),
                ));
            }
            return self.get_object(request, query).await;
        }
        if path == "/api/object" {
            return self.get_object(request, query).await;
        }
        if path == "/api/object/list" {
            return self.list_objects(query).await;
        }
        if path == "/api/object/stat" {
            return self.stat_object(query).await;
        }
        if path == "/api/machine/status" {
            return Ok(self.get_machine_status(request, query).await);
        }
        if path == "/api/service/status" {
            return Ok(self.get_service_status(request, query).await);
        }
        // The registry-policy boundary gates both of these. Its verifier is
        // ready even when nothing is declared, so an undeclared deployment
        // refuses here with 401 rather than reporting an outage.
        if path == "/api/registry.json" {
            if !self.boundaries_available(&[Boundary::Registry]).await {
                return Ok(send_json(
                    http_status("503"),
                    &json!({"error": "registry authorization unavailable"}),
                ));
            }
            if let Err(response) = registry_policy::authorized(request, "policy-read").await {
                return Ok(response);
            }
            return Ok(registry_policy::get_policy().await);
        }
        if path == "/api/cleanup.json" {
            if !self.boundaries_available(&[Boundary::Registry]).await {
                return Ok(send_json(
                    http_status("503"),
                    &json!({"error": "registry authorization unavailable"}),
                ));
            }
            if let Err(response) = registry_policy::authorized(request, "cleanup-read").await {
                return Ok(response);
            }
            return Ok(registry_policy::get_cleanup());
        }
        if path == "/api/memory-policies.json" {
            if !self.boundaries_available(&[Boundary::Registry]).await {
                return Ok(send_json(
                    http_status("503"),
                    &json!({"error": "registry authorization unavailable"}),
                ));
            }
            if let Err(response) = registry_policy::authorized(request, "policy-read").await {
                return Ok(response);
            }
            return Ok(registry_policy::get_memory_policies());
        }

        Ok(empty_response(404, "Not Found"))
    }
}
