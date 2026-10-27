//! The version token one read carries, and the write conditional on it.

use crate::cli::storage::*;

impl RemoteObjectApi {
    /// The refusal for a versioned answer whose version cannot be read.
    fn unversioned() -> CmdError {
        CmdError::click(
            "Stado object API answered a versioned GET without a readable x-stado-version header",
        )
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    }

    pub(in crate::cli::storage) async fn get_versioned(
        &self,
        uri: &str,
    ) -> Result<Option<(Vec<u8>, String)>, CmdError> {
        let endpoint = self.endpoint("/api/object", &[("uri", uri), ("versioned", "true")])?;
        let bearer = self.release_bearer(uri).await?;
        let response =
            sent(self.request_as(reqwest::Method::GET, endpoint, bearer.as_deref())).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(self.response_error(response, bearer.as_deref()).await);
        }
        // The versioned answer carries its version in a header the API
        // documents as ASCII; one missing, blank or not text is an answer
        // this command cannot fence a write against.
        let version = match response.headers().get("x-stado-version") {
            Some(value) => match value.to_str() {
                Ok(text) if !text.is_empty() => text.to_string(),
                Ok(_blank) => return Err(Self::unversioned()),
                Err(_not_text) => return Err(Self::unversioned()),
            },
            None => return Err(Self::unversioned()),
        };
        let bytes = self
            .success_body(response, "versioned object GET", bearer.as_deref())
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
        let response = sent(
            self.request_as(reqwest::Method::PUT, endpoint, bearer.as_deref())
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(bytes),
        )
        .await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            // The refusal names what was presented: the publisher item the
            // release policy maps this key to, when the URI is an object
            // reference under a release-governed key and a publisher is
            // declared for it; otherwise the credential class that was sent.
            let credential_class = if bearer.is_some() {
                "a resolved release credential".to_string()
            } else {
                "the coordinator storage token".to_string()
            };
            let publisher = match crate::remote::object_store::ObjectRef::parse(uri) {
                Ok(object) => match crate::remote::object_store::release_policy_key(
                    object.namespace(),
                    object.key(),
                ) {
                    Some(key) => match crate::config::release_client_publisher_for_key(&key) {
                        Ok(publisher) => publisher,
                        Err(_declaration_refused_elsewhere) => None,
                    },
                    None => None,
                },
                Err(_not_an_object_reference) => None,
            };
            let presented = match publisher {
                Some(publisher) => format!("publisher item {}", publisher.item()),
                None => credential_class,
            };
            let refusal = self.response_error(response, bearer.as_deref()).await;
            let mut wrapped = CmdError::click(format!(
                "{refusal}; conditional PUT {uri} with if_version={expected_version:?} presented \
                 {presented}"
            ));
            wrapped.failure = refusal.failure;
            return Err(wrapped);
        }
        let payload: Value = self
            .response_json(response, "conditional object PUT", bearer.as_deref())
            .await?;
        if payload.get("state").and_then(Value::as_str) != Some("stored")
            || payload.get("uri").and_then(Value::as_str) != Some(uri)
        {
            return Err(CmdError::click(
                "Stado object API returned an inconsistent conditional PUT response",
            )
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        Ok(())
    }
}
