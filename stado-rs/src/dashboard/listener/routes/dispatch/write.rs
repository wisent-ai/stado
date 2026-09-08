//! `PUT` and `DELETE`: the host beacon publication, the object write, and the
//! one deletion a release coordinate may ever answer.

use serde_json::json;

use crate::dashboard::listener::auth::{
    authorize_object, authorize_release, release_object_namespace, release_upload_target_key,
};
use crate::dashboard::listener::boundary::requires_object_boundary;
use crate::dashboard::listener::routes::object::object_from_query;
use crate::dashboard::listener::{
    dashboard_error_response, empty_response, http_status, parse_qs, query_value, send_json,
    storage_error_response, Boundary, Dashboard, Request, Response,
};

impl Dashboard {
    pub(crate) async fn do_put(&self, request: &Request) -> Response {
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        if path == "/api/host-health" {
            return self.put_host_health(request, query).await;
        }
        if path != "/api/object" {
            return empty_response(http_status("404"), "Not Found");
        }
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return response,
        };
        if requires_object_boundary(object.namespace(), object.key())
            && !self.boundaries_available(&[Boundary::Object]).await
        {
            return send_json(
                http_status("503"),
                &json!({"error": "object authorization unavailable"}),
            );
        }
        let authorized = if let Some(policy_key) =
            crate::object_store::release_policy_key(object.namespace(), object.key())
        {
            // A release object is only ever created, never replaced; the
            // client resolves its credential from the same routing function.
            let immutable = query_value(&parse_qs(query), "if_absent").as_deref() == Some("true");
            if object.namespace() == "releases" && !immutable {
                Ok(Some("release_write_must_be_create_only"))
            } else {
                authorize_release(self, request, &policy_key, false).await
            }
        } else {
            authorize_object(
                self,
                request,
                object.namespace(),
                object.key(),
                false,
                "put",
            )
            .await
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
        let _write_guard = match self.storage_write_guard() {
            Ok(guard) => guard,
            Err(error) => return storage_error_response(error),
        };
        match self.put_object(request, &object, query).await {
            Ok(response) => response,
            Err(error) => dashboard_error_response(error),
        }
    }

    pub(crate) async fn do_delete(&self, request: &Request) -> Response {
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return send_json(http_status("403"), &json!({"error": "forbidden"}));
        }
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        if path != "/api/object" {
            return empty_response(http_status("404"), "Not Found");
        }
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return response,
        };
        let authorized = if let Some(policy_key) =
            crate::object_store::release_policy_key(object.namespace(), object.key())
        {
            if release_object_namespace(object.namespace()) {
                let Some(target_key) = release_upload_target_key(object.key()) else {
                    return send_json(
                        http_status("403"),
                        &json!({"error": "release objects are immutable and cannot be deleted"}),
                    );
                };
                authorize_release(self, request, target_key, false).await
            } else {
                authorize_release(self, request, &policy_key, false).await
            }
        } else {
            if !self.boundaries_available(&[Boundary::Object]).await {
                return send_json(
                    http_status("503"),
                    &json!({"error": "object authorization unavailable"}),
                );
            }
            authorize_object(
                self,
                request,
                object.namespace(),
                object.key(),
                false,
                "delete",
            )
            .await
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
        let _write_guard = match self.storage_write_guard() {
            Ok(guard) => guard,
            Err(error) => return storage_error_response(error),
        };
        let result = self.store.delete_blob(&object.storage_path()).await;
        match result {
            Ok(()) => send_json(
                http_status("200"),
                &json!({"state": "absent", "uri": object.to_string()}),
            ),
            Err(error) => storage_error_response(error),
        }
    }
}
