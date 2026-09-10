//! The object read plane: one object's bytes and byte ranges, one authorized
//! prefix listing, and one object's metadata.

use serde_json::json;

use crate::config;
use crate::models::isoformat_utc;

use crate::dashboard::listener::auth::release_object_namespace;
use crate::dashboard::listener::http::parse_byte_range;
use crate::dashboard::listener::{
    http_status, parse_qs, query_value, send_json, Dashboard, Request, Response,
};
use crate::dashboard::DashboardError;

use super::{object_from_query, object_list_from_query};

impl Dashboard {
    pub(crate) async fn get_object(
        &self,
        request: &Request,
        query: &str,
    ) -> Result<Response, DashboardError> {
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return Ok(response),
        };
        let path = object.storage_path();
        let versioned = query_value(&parse_qs(query), "versioned").as_deref() == Some("true");
        let public_release_route = request
            .path
            .split_once('?')
            .map_or(request.path.as_str(), |(path, _)| path)
            == "/api/release/object";
        let (bytes, version) = if versioned {
            let Some(value) = self.store.read_text_versioned(&path).await? else {
                return Ok(send_json(
                    http_status("404"),
                    &json!({"state": "absent", "uri": object.to_string()}),
                ));
            };
            (value.content.into_bytes(), Some(value.version))
        } else {
            let bytes = if object.namespace() == "releases" && public_release_route {
                // Public delivery may traverse a namespaced Stado-object backend,
                // so it uses that backend's cross-namespace release route.
                self.store
                    .backend()
                    .download_release(&object.to_string())
                    .await?
            } else {
                // Authenticated /api/object reads the same local storage path as
                // PUT. Sending this path through the public route made a successful
                // write immediately unreadable and broke publisher preflight.
                self.store.read_bytes(&path).await?
            };
            let Some(bytes) = bytes else {
                return Ok(send_json(
                    http_status("404"),
                    &json!({"state": "absent", "uri": object.to_string()}),
                ));
            };
            (bytes, None)
        };
        let metadata = self
            .store
            .backend()
            .list_blobs_with_meta(&path)
            .await?
            .into_iter()
            .find(|blob| blob.name == path)
            .map(|blob| blob.metadata)
            .unwrap_or_default();
        let content_type = metadata
            .get("content-type")
            .map(String::as_str)
            .unwrap_or("application/octet-stream");
        if let Some(value) = request.header("range") {
            let Some((start, end)) = parse_byte_range(value, bytes.len()) else {
                return Ok(Response::new_with_headers(
                    http_status("416"),
                    "Range Not Satisfiable",
                    content_type,
                    b"",
                    &[("Content-Range", format!("bytes */{}", bytes.len()))],
                ));
            };
            return Ok(Response::new_with_headers(
                http_status("206"),
                "Partial Content",
                content_type,
                &bytes[start..=end],
                &[
                    ("Accept-Ranges", "bytes".to_string()),
                    (
                        "Content-Range",
                        format!("bytes {start}-{end}/{}", bytes.len()),
                    ),
                ],
            ));
        }
        let mut headers = vec![("Accept-Ranges", "bytes".to_string())];
        if let Some(version) = version {
            headers.push(("X-Stado-Version", version));
        }
        Ok(Response::new_with_headers(
            http_status("200"),
            "OK",
            content_type,
            &bytes,
            &headers,
        ))
    }

    pub(crate) async fn list_objects(&self, query: &str) -> Result<Response, DashboardError> {
        let (namespace, requested_prefix) = match object_list_from_query(query) {
            Ok(scope) => scope,
            Err(response) => return Ok(response),
        };
        let prefix = if release_object_namespace(&namespace) {
            config::release_publisher_for_list(&requested_prefix).map(|(_, authorized)| authorized)
        } else {
            config::object_api_namespace(&namespace)
                .and_then(|policy| policy.authorized_list_prefix(&requested_prefix, "list"))
        };
        let Some(prefix) = prefix else {
            return Ok(send_json(
                http_status("401"),
                &json!({"error": "unauthorized"}),
            ));
        };
        let storage_prefix = crate::remote::object_store::ObjectRef::namespace_prefix(&namespace, &prefix)?;
        let objects = self
            .store
            .backend()
            .list_blobs_with_meta(&storage_prefix)
            .await?;
        let mut response = Vec::with_capacity(objects.len());
        for blob in objects {
            let object = crate::remote::object_store::ObjectRef::from_storage_path(&blob.name)?;
            response.push(json!({
                "uri": object.to_string(),
                "namespace": object.namespace(),
                "key": object.key(),
                "size": blob.size,
                "updated_at": blob.updated.map(isoformat_utc),
                "metadata": blob.metadata,
            }));
        }
        Ok(send_json(http_status("200"), &json!({"objects": response})))
    }

    pub(crate) async fn stat_object(&self, query: &str) -> Result<Response, DashboardError> {
        let object = match object_from_query(query) {
            Ok(object) => object,
            Err(response) => return Ok(response),
        };
        let path = object.storage_path();
        let blob = self
            .store
            .backend()
            .list_blobs_with_meta(&path)
            .await?
            .into_iter()
            .find(|blob| blob.name == path);
        Ok(match blob {
            Some(blob) => send_json(
                http_status("200"),
                &json!({
                    "state": "present",
                    "uri": object.to_string(),
                    "size": blob.size,
                    "updated_at": blob.updated.map(isoformat_utc),
                    "metadata": blob.metadata,
                }),
            ),
            None => send_json(
                http_status("404"),
                &json!({"state": "absent", "uri": object.to_string()}),
            ),
        })
    }
}
