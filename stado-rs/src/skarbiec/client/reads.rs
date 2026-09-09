//! What this client reads: a whole item, one named field, the item list, and
//! the configured-consumer read that goes through the credential store.

use serde_json::{json, Value};

use super::super::{ItemInfo, SkarbiecError};
use super::Client;

impl Client {
    /// Read a whole item.
    ///
    /// Callers pick several fields out of the returned object, so this asks for
    /// the item rather than a field. Skarbiec commit 9aa7dd4 ("Rebuild vault
    /// contracts and credential lifecycle", 2026-08-04) made `field` mandatory
    /// on this route, and a broker built from it answers
    /// `400 {"error":"field required"}` to every call here. That surfaced as an
    /// unattributable failure that took out the host-health beacon and Brama's
    /// startup on the same machine, so the skew is named here rather than left
    /// as a bare 400: the request is well-formed for the contract this client
    /// was written against, and the broker is newer than the client.
    pub async fn read_item(&self, id: &str) -> Result<Value, SkarbiecError> {
        if self.route_store {
            return Box::pin(crate::credential_store::read_item_with(
                &self.base_url,
                &self.consumer,
                &self.token_file,
                self.grant_mode,
                id,
            ))
            .await;
        }
        let response = self
            .request(reqwest::Method::POST, "/v1/items/read")?
            .json(&json!({"id": id}))
            .send()
            .await?;
        let body = match Self::response_json(response).await {
            Err(SkarbiecError::Response { status, detail })
                if status == reqwest::StatusCode::BAD_REQUEST.as_u16()
                    && detail.contains("field required") =>
            {
                return Err(SkarbiecError::Response {
                    status,
                    detail: format!(
                        "{detail} — this broker requires a named field on /v1/items/read, \
                         while this client asks for the whole item {id:?}. The broker is \
                         newer than the client; read one field with read_string, or update \
                         the client to the broker's contract."
                    ),
                });
            }
            other => other?,
        };
        body.get("value")
            .cloned()
            .ok_or_else(|| SkarbiecError::MissingValue(id.to_string()))
    }

    /// Read one named field, which is what this broker's `/v1/items/read`
    /// contract asks for since Skarbiec 9aa7dd4.
    ///
    /// One round trip and one field, rather than fetching the item and picking
    /// from it: that is both the newer contract and the smaller disclosure, so
    /// there is no reason to prefer the whole-item read where the caller
    /// already knows the field it wants.
    pub async fn read_field(&self, id: &str, field: &str) -> Result<Value, SkarbiecError> {
        self.read_field_inner(id, field)
            .await
            .map_err(|error| error.naming(&self.consumer, id, field))
    }

    async fn read_field_inner(&self, id: &str, field: &str) -> Result<Value, SkarbiecError> {
        let response = self
            .request(reqwest::Method::POST, "/v1/items/read")?
            .json(&json!({"id": id, "field": field}))
            .send()
            .await?;
        let body = Self::response_json(response).await?;
        body.get("value")
            .cloned()
            .ok_or_else(|| SkarbiecError::MissingValue(format!("{id}.{field}")))
    }

    pub async fn list_items(&self) -> Result<Vec<ItemInfo>, SkarbiecError> {
        if self.route_store {
            return Box::pin(crate::credential_store::write::list_items_with(
                &self.base_url,
                &self.consumer,
                &self.token_file,
                self.grant_mode,
            ))
            .await;
        }
        let response = self
            .request(reqwest::Method::POST, "/v1/items/list")?
            .json(&json!({}))
            .send()
            .await?;
        let body = Self::response_json(response).await?;
        serde_json::from_value(body).map_err(|source| SkarbiecError::Response {
            status: reqwest::StatusCode::OK.as_u16(),
            detail: format!("invalid item-list response: {source}"),
        })
    }

    /// Resolve one optional string field through this client's scoped grant.
    pub async fn read_string(
        &self,
        id: &str,
        field: &str,
    ) -> Result<Option<String>, SkarbiecError> {
        self.read_string_inner(id, field)
            .await
            .map_err(|error| error.naming(&self.consumer, id, field))
    }

    async fn read_string_inner(
        &self,
        id: &str,
        field: &str,
    ) -> Result<Option<String>, SkarbiecError> {
        if self.route_store {
            return Box::pin(crate::credential_store::read_string_with(
                &self.base_url,
                &self.consumer,
                &self.token_file,
                self.grant_mode,
                id,
                field,
            ))
            .await;
        }
        let response = self
            .request(reqwest::Method::POST, "/v1/items/read")?
            .json(&json!({"id": id, "field": field}))
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let body = Self::response_json(response).await?;
        Ok(body
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// Read one item with the configured Stado consumer grant. Flows through
    /// the credential store selector: the default skarbiec backend calls this
    /// same client (byte-identical); the file backend answers from disk.
    pub async fn configured_item(id: &str) -> Result<Value, SkarbiecError> {
        crate::credential_store::read_item(id).await
    }
}
