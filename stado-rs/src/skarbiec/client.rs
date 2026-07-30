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
        let consumer = consumer.trim();
        if consumer.is_empty() {
            return Err(SkarbiecError::MissingConsumer);
        }
        if token_file.trim().is_empty() {
            return Err(SkarbiecError::MissingTokenFile);
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            base_url: checked_url(base_url)?,
            consumer: consumer.to_string(),
            token_file: token_file.to_string(),
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

    pub async fn read_item(&self, id: &str) -> Result<Value, SkarbiecError> {
        let response = self
            .request(reqwest::Method::POST, "/v1/items/read")?
            .json(&json!({"id": id}))
            .send()
            .await?;
        let body = Self::response_json(response).await?;
        body.get("value")
            .cloned()
            .ok_or_else(|| SkarbiecError::MissingValue(id.to_string()))
    }

    pub async fn write_item(
        &self,
        id: &str,
        item_type: &str,
        value: &Value,
    ) -> Result<(), SkarbiecError> {
        let response = self
            .request(reqwest::Method::PUT, "/v1/items")?
            .json(&json!({"id": id, "type": item_type, "value": value}))
            .send()
            .await?;
        Self::response_json(response).await?;
        Ok(())
    }

    pub async fn list_items(&self) -> Result<Vec<ItemInfo>, SkarbiecError> {
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
        match self.read_item(id).await {
            Ok(value) => Ok(value.get(field).and_then(Value::as_str).map(str::to_string)),
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
