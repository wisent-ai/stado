//! The Skarbiec HTTP client: constructor, request plumbing, and item CRUD.
//! Verifier-grant constructors live in `verifiers.rs`; this is the plain
//! client every read path ultimately talks through.

use serde_json::{json, Value};

use super::{
    checked_url, erase_transient_agent_grant, read_grant, ItemInfo, SkarbiecError, AGENT_GRANTS,
};

pub struct Client {
    http: reqwest::Client,
    base_url: String,
    consumer: String,
    token_file: String,
    route_store: bool,
}

impl Client {
    pub fn configured() -> Result<Self, SkarbiecError> {
        Self::new(
            crate::config::skarbiec_url(),
            crate::config::skarbiec_consumer(),
            crate::config::skarbiec_token_file(),
        )
    }

    pub fn new(base_url: &str, consumer: &str, token_file: &str) -> Result<Self, SkarbiecError> {
        Self::build(base_url, consumer, token_file, true)
    }

    pub(crate) fn direct(
        base_url: &str,
        consumer: &str,
        token_file: &str,
    ) -> Result<Self, SkarbiecError> {
        Self::build(base_url, consumer, token_file, false)
    }

    fn build(
        base_url: &str,
        consumer: &str,
        token_file: &str,
        route_store: bool,
    ) -> Result<Self, SkarbiecError> {
        let consumer = consumer.trim().to_string();
        let token_file = token_file.trim().to_string();
        if !route_store && consumer.is_empty() {
            return Err(SkarbiecError::MissingConsumer);
        }
        if !route_store && token_file.is_empty() {
            return Err(SkarbiecError::MissingTokenFile);
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let base_url = if route_store {
            base_url.trim().to_string()
        } else {
            checked_url(base_url)?
        };
        Ok(Self {
            http,
            base_url,
            consumer,
            token_file,
            route_store,
        })
    }

    fn request_token(&self) -> Result<String, SkarbiecError> {
        if !self.consumer.ends_with("-agent") {
            return read_grant(&self.token_file);
        }
        let key = (self.consumer.clone(), self.token_file.clone());
        let mut cached = AGENT_GRANTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(token) = cached.get(&key) {
            return Ok(token.clone());
        }
        let token = read_grant(&self.token_file)?;
        let byte_count = token.len();
        cached.insert(key, token.clone());
        erase_transient_agent_grant(&self.token_file, byte_count);
        Ok(token)
    }

    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, SkarbiecError> {
        let token = self.request_token()?;
        Ok(self
            .http
            .request(method, format!("{}{path}", self.base_url))
            .header("X-Consumer", &self.consumer)
            .bearer_auth(token))
    }

    async fn response_json(response: reqwest::Response) -> Result<Value, SkarbiecError> {
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(SkarbiecError::Response {
                status: status.as_u16(),
                detail: body.chars().take(usize::from(u16::MAX)).collect(),
            });
        }
        serde_json::from_str(&body).map_err(|source| SkarbiecError::Response {
            status: status.as_u16(),
            detail: format!("invalid JSON response: {source}"),
        })
    }

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

    pub async fn write_item(
        &self,
        id: &str,
        item_type: &str,
        value: &Value,
    ) -> Result<(), SkarbiecError> {
        if self.route_store {
            return Box::pin(crate::credential_store::write::write_item_with(
                &self.base_url,
                &self.consumer,
                &self.token_file,
                id,
                item_type,
                value,
            ))
            .await;
        }
        let response = self
            .request(reqwest::Method::PUT, "/v1/items")?
            .json(&json!({"id": id, "type": item_type, "value": value}))
            .send()
            .await?;
        Self::response_json(response).await?;
        Ok(())
    }

    pub async fn list_items(&self) -> Result<Vec<ItemInfo>, SkarbiecError> {
        if self.route_store {
            return Box::pin(crate::credential_store::write::list_items_with(
                &self.base_url,
                &self.consumer,
                &self.token_file,
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

    pub async fn delete_item(&self, id: &str) -> Result<(), SkarbiecError> {
        if self.route_store {
            return Box::pin(crate::credential_store::write::delete_item_with(
                &self.base_url,
                &self.consumer,
                &self.token_file,
                id,
            ))
            .await;
        }
        let response = self
            .request(reqwest::Method::DELETE, "/v1/items")?
            .json(&json!({"id": id}))
            .send()
            .await?;
        Self::response_json(response).await?;
        Ok(())
    }

    /// Resolve one optional string field through this client's scoped grant.
    pub async fn read_string(
        &self,
        id: &str,
        field: &str,
    ) -> Result<Option<String>, SkarbiecError> {
        // One field, one request. Going through read_item asked the broker
        // for the whole item and then discarded all but this field, which
        // stopped working the moment the broker began requiring a named field.
        match self.read_field(id, field).await {
            Ok(value) => Ok(value.as_str().map(str::to_string)),
            Err(SkarbiecError::Response { status, .. })
                if status == reqwest::StatusCode::NOT_FOUND.as_u16() =>
            {
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    /// Read one item with the configured Stado consumer grant. Flows through
    /// the credential store selector: the default skarbiec backend calls this
    /// same client (byte-identical); the file backend answers from disk.
    pub async fn configured_item(id: &str) -> Result<Value, SkarbiecError> {
        crate::credential_store::read_item(id).await
    }
}
