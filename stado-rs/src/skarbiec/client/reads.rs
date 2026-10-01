//! What this client reads: a whole item, one named field, the item list, and
//! the configured-consumer read that goes through the credential store.
//!
//! Every read names a role (`skarbiec::roles`), never an item: the client lists
//! what its grant exposes, selects the one item carrying `stado:role:<role>`,
//! and only then reads that item by the id the vault gave it. No caller holds
//! an item id, so renaming an item changes nothing.

use serde_json::{json, Value};

use super::super::{roles, ItemInfo, SkarbiecError};
use super::Client;

impl Client {
    /// The id of the one item this consumer can see that plays `role`;
    /// `None` when no item does. Two items in one role are refused, because
    /// reading either would be a guess.
    async fn item_playing(&self, role: &str) -> Result<Option<String>, SkarbiecError> {
        let items = self.list_items().await?;
        match roles::holders(&items, role).as_slice() {
            [] => Ok(None),
            [one] => Ok(Some(one.id.clone())),
            several => Err(SkarbiecError::Deployment(format!(
                "{} items visible to consumer {} carry {}; exactly one item may play role {role}",
                several.len(),
                self.consumer,
                roles::role_tag(role)
            ))),
        }
    }

    async fn required_item(&self, role: &str) -> Result<String, SkarbiecError> {
        self.item_playing(role).await?.ok_or_else(|| {
            SkarbiecError::MissingValue(format!(
                "no item visible to consumer {} carries {}",
                self.consumer,
                roles::role_tag(role)
            ))
        })
    }

    /// Read the whole item that plays `role`.
    ///
    /// Callers pick several fields out of the returned object, so this asks for
    /// the item rather than a field. A broker that requires a named field on
    /// `/v1/items/read` answers `400 field required`; that skew is named here
    /// rather than left as a bare 400, because the request is well-formed for
    /// the contract this client was written against.
    pub async fn read_item(&self, role: &str) -> Result<Value, SkarbiecError> {
        // A routed client hands the role to the store, whose own client
        // selects the item; selecting here as well would read the id as a role.
        if self.route_store {
            return Box::pin(crate::credential_store::read_item_with(
                &self.base_url,
                &self.consumer,
                &self.token_file,
                self.grant_mode,
                role,
            ))
            .await;
        }
        let id = self.required_item(role).await?;
        let id = id.as_str();
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

    /// Read one named field of the item that plays `role`: one field rather
    /// than the whole item, which is both the broker's contract and the
    /// smaller disclosure.
    pub async fn read_field(&self, role: &str, field: &str) -> Result<Value, SkarbiecError> {
        self.read_field_inner(role, field)
            .await
            .map_err(|error| error.naming(&self.consumer, role, field))
    }

    async fn read_field_inner(&self, role: &str, field: &str) -> Result<Value, SkarbiecError> {
        let id = self.required_item(role).await?;
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

    /// Resolve one optional string field of the item that plays `role`
    /// through this client's scoped grant; `None` when no item plays it.
    pub async fn read_string(
        &self,
        role: &str,
        field: &str,
    ) -> Result<Option<String>, SkarbiecError> {
        self.read_string_inner(role, field)
            .await
            .map_err(|error| error.naming(&self.consumer, role, field))
    }

    async fn read_string_inner(
        &self,
        role: &str,
        field: &str,
    ) -> Result<Option<String>, SkarbiecError> {
        if self.route_store {
            return Box::pin(crate::credential_store::read_string_with(
                &self.base_url,
                &self.consumer,
                &self.token_file,
                self.grant_mode,
                role,
                field,
            ))
            .await;
        }
        let Some(id) = self.item_playing(role).await? else {
            return Ok(None);
        };
        let id = id.as_str();
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

    /// Resolve one optional string field of the item a boundary declaration
    /// names (`object_api.namespaces.*.item`, `release_api.publishers`,
    /// `machine_api.clients`, `service_api.deployers`, `registry_api.clients`):
    /// the configuration chose that item, so it is read as named and never
    /// looked up as a role. Selecting it by role closed every boundary of the
    /// object API with `503 object authorization unavailable`, because no
    /// verifier item plays a role. `None` when the item or field is absent.
    pub async fn read_declared_string(
        &self,
        item: &str,
        field: &str,
    ) -> Result<Option<String>, SkarbiecError> {
        if self.route_store {
            return Box::pin(crate::credential_store::read_declared_string_with(
                &self.base_url,
                &self.consumer,
                &self.token_file,
                self.grant_mode,
                item,
                field,
            ))
            .await
            .map_err(|error| error.naming(&self.consumer, item, field));
        }
        let response = self
            .request(reqwest::Method::POST, "/v1/items/read")?
            .json(&json!({"id": item, "field": field}))
            .send()
            .await
            .map_err(|error| SkarbiecError::from(error).naming(&self.consumer, item, field))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let body = Self::response_json(response)
            .await
            .map_err(|error| error.naming(&self.consumer, item, field))?;
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
