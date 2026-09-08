//! The version token one read carries, and the write conditional on it.

use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) async fn get_versioned(
        &self,
        uri: &str,
    ) -> Result<Option<(Vec<u8>, String)>, CmdError> {
        let endpoint = self.endpoint("/api/object", &[("uri", uri), ("versioned", "true")])?;
        let bearer = self.release_bearer(uri).await?;
        let response = self
            .request_as(reqwest::Method::GET, endpoint, bearer.as_deref())
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(self.response_error(response, bearer.as_deref()).await);
        }
        let version = response
            .headers()
            .get("x-stado-version")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CmdError::click("Stado object API omitted the CAS version"))?
            .to_string();
        let bytes = self
            .success_body(
                response,
                max_object_api_download_body(),
                "versioned object GET",
                bearer.as_deref(),
            )
            .await?;
        Ok(Some((bytes, version)))
    }

    pub(in crate::cli::storage) async fn put_if_version(
        &self,
        uri: &str,
        content_type: &str,
        expected_version: &str,
        bytes: Vec<u8>,
    ) -> Result<(), CmdError> {
        let endpoint = self.endpoint(
            "/api/object",
            &[("uri", uri), ("if_version", expected_version)],
        )?;
        let bearer = self.release_bearer(uri).await?;
        let response = self
            .request_as(reqwest::Method::PUT, endpoint, bearer.as_deref())
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes)
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            let presented = crate::object_store::ObjectRef::parse(uri)
                .ok()
                .and_then(|object| {
                    crate::object_store::release_policy_key(object.namespace(), object.key())
                })
                .and_then(|key| {
                    crate::config::release_client_publisher_for_key(&key)
                        .ok()
                        .flatten()
                })
                .map(|publisher| format!("publisher item {}", publisher.item()))
                .unwrap_or_else(|| {
                    if bearer.is_some() {
                        "a resolved release credential".to_string()
                    } else {
                        "the coordinator storage token".to_string()
                    }
                });
            let refusal = self.response_error(response, bearer.as_deref()).await;
            return Err(CmdError::click(format!(
                "{refusal}; conditional PUT {uri} with if_version={expected_version:?} presented \
                 {presented}"
            )));
        }
        let payload: Value = self
            .response_json(response, "conditional object PUT", bearer.as_deref())
            .await?;
        if payload.get("state").and_then(Value::as_str) != Some("stored")
            || payload.get("uri").and_then(Value::as_str) != Some(uri)
        {
            return Err(CmdError::click(
                "Stado object API returned an inconsistent conditional PUT response",
            ));
        }
        Ok(())
    }
}
