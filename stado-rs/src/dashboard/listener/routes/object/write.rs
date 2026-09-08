//! The object write plane: the preflight that refuses a PUT before its body
//! is read, and the write itself.

use std::collections::BTreeMap;
use std::io::Write;

use serde_json::json;

use crate::queue::StorageError;

use crate::dashboard::listener::auth::{authorize_object, authorize_release};
use crate::dashboard::listener::boundary::boundary_plan;
use crate::dashboard::listener::{
    empty_response, http_status, parse_qs, query_value, send_json, Dashboard, Request, Response,
};
use crate::dashboard::DashboardError;

use super::{merged_object_metadata, object_from_query};

impl Dashboard {
    pub(crate) async fn object_put_preflight(&self, request: &Request) -> Option<Response> {
        if !self.trusted_request_host(request.header("host"), request.header("x-forwarded-proto")) {
            return Some(send_json(
                http_status("403"),
                &json!({"error": "forbidden"}),
            ));
        }
        let (path, query) = request
            .path
            .split_once('?')
            .unwrap_or((request.path.as_str(), ""));
        if path != "/api/object" {
            return Some(empty_response(http_status("404"), "Not Found"));
        }
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return Some(response),
        };
        // A release coordinate authorizes against the RELEASE verifier's
        // material (`authorize_release` reads `release_token` for the mapped
        // item), so that boundary is this request's precondition just as much
        // as the object one. Requiring it here is what makes
        // `Boundary::Release` mean something: it was enumerated, labelled,
        // described as "release publication", validated once at startup and
        // required by NO route, so it read `false` until someone restarted the
        // unit and no request could ever reopen it — `boundaries_available`
        // revalidates only what a request requires. On 2026-08-31 an operator
        // read that field, believed its description, and held the quietest
        // publication window of the night waiting for a value with no
        // mechanism to change.
        //
        // Ordinary object traffic is deliberately unaffected: only a
        // release-policy coordinate adds the requirement, because only it
        // reads that material.
        // Ordinary object traffic is deliberately unaffected, and a release
        // coordinate now REVALIDATES the release boundary without being gated
        // by it -- the split `boundary_plan` exists for.
        let plan = boundary_plan(
            request.path.split('?').next().unwrap_or(""),
            Some((object.namespace(), object.key())),
        );
        if !self.satisfy_boundaries(&plan).await {
            return Some(send_json(
                http_status("503"),
                &json!({"error": "object authorization unavailable"}),
            ));
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
                return Some(send_json(
                    http_status("401"),
                    &json!({"error": "unauthorized", "reason": reason}),
                ))
            }
            Err(()) => {
                return Some(send_json(
                    http_status("503"),
                    &json!({"error": "object authorization unavailable"}),
                ))
            }
        }
        None
    }

    pub(crate) async fn put_object(
        &self,
        request: &Request,
        object: &crate::object_store::ObjectRef,
        query: &str,
    ) -> Result<Response, DashboardError> {
        let values = parse_qs(query);
        let if_absent = query_value(&values, "if_absent").as_deref() == Some("true");
        let if_version = query_value(&values, "if_version").filter(|value| !value.is_empty());
        let metadata_only = query_value(&values, "metadata_only").as_deref() == Some("true");
        let selected =
            usize::from(if_absent) + usize::from(if_version.is_some()) + usize::from(metadata_only);
        if selected > 1 {
            return Ok(send_json(
                http_status("400"),
                &json!({"error": "if_absent, if_version, and metadata_only are mutually exclusive"}),
            ));
        }
        let path = object.storage_path();
        if metadata_only {
            if !self.store.backend().exists(&path).await? {
                return Ok(send_json(
                    http_status("404"),
                    &json!({"state": "absent", "uri": object.to_string()}),
                ));
            }
            let metadata: std::collections::BTreeMap<String, String> =
                match serde_json::from_slice(&request.body) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        return Ok(send_json(
                            http_status("400"),
                            &json!({"error": format!("invalid metadata: {error}")}),
                        ))
                    }
                };
            self.store.backend().set_metadata(&path, &metadata).await?;
            return Ok(send_json(
                http_status("200"),
                &json!({"state": "metadata-updated", "uri": object.to_string()}),
            ));
        }
        if let Some(expected_version) = if_version {
            let content = match std::str::from_utf8(&request.body) {
                Ok(content) => content,
                Err(error) => {
                    return Ok(send_json(
                        http_status("400"),
                        &json!({"error": format!("conditional object writes require UTF-8: {error}")}),
                    ))
                }
            };
            let version = match self
                .store
                .compare_and_swap_text(&path, &expected_version, content)
                .await
            {
                Ok(version) => version,
                Err(StorageError::StorageConflict(_)) => {
                    return Ok(send_json(
                        http_status("409"),
                        &json!({"error": "object version changed", "uri": object.to_string()}),
                    ))
                }
                Err(StorageError::NotFound(_)) => {
                    return Ok(send_json(
                        http_status("404"),
                        &json!({"state": "absent", "uri": object.to_string()}),
                    ))
                }
                Err(error) => return Err(error.into()),
            };
            return Ok(send_json(
                http_status("200"),
                &json!({
                    "state": "stored",
                    "uri": object.to_string(),
                    "version": version,
                }),
            ));
        }
        let content_type = request
            .header("content-type")
            .unwrap_or("application/octet-stream")
            .to_string();
        let extra = match request.header("x-stado-object-metadata") {
            Some(raw) => match serde_json::from_str(raw) {
                Ok(value) => value,
                Err(error) => {
                    return Ok(send_json(
                        http_status("400"),
                        &json!({"error": format!("invalid object metadata: {error}")}),
                    ))
                }
            },
            None => BTreeMap::new(),
        };
        let metadata = match merged_object_metadata(object, &content_type, &extra) {
            Ok(metadata) => metadata,
            Err(error) => return Ok(send_json(http_status("400"), &json!({"error": error}))),
        };
        if if_absent {
            let mut source = tempfile::NamedTempFile::new()?;
            source.write_all(&request.body)?;
            if !self
                .store
                .upload_file_if_absent(&path, source.path())
                .await?
            {
                return Ok(send_json(
                    http_status("409"),
                    &json!({"error": "object exists", "uri": object.to_string()}),
                ));
            }
        } else {
            self.store.upload_bytes(&path, &request.body).await?;
        }
        self.store.backend().set_metadata(&path, &metadata).await?;
        let landed = self.store.backend().list_blobs_with_meta(&path).await?;
        let Some(blob) = landed.into_iter().find(|blob| blob.name == path) else {
            return Err(DashboardError::Other(format!(
                "object metadata verification could not find {object}"
            )));
        };
        if metadata
            .iter()
            .filter(|(_, value)| !value.is_empty())
            .any(|(key, value)| blob.metadata.get(key) != Some(value))
        {
            return Err(DashboardError::Other(format!(
                "object metadata verification failed for {object}"
            )));
        }
        Ok(send_json(
            http_status("200"),
            &json!({
                "state": "stored",
                "uri": object.to_string(),
                "content_type": content_type,
            }),
        ))
    }
}
