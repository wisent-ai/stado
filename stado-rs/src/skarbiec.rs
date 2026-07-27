//! Client for the separate Skarbiec credential service.
//!
//! Skarbiec is reached over its loopback HTTP API or a TLS-protected remote
//! endpoint. Application credentials are decrypted by Skarbiec, authorized by
//! an action-scoped consumer grant, and held only in the requesting Stado
//! process. Stado has no environment, queue-storage, cloud-secret-manager, or
//! local credential fallback.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum SkarbiecError {
    #[error("invalid Skarbiec URL {0:?}; expected loopback HTTP or HTTPS")]
    InvalidUrl(String),
    #[error("Skarbiec consumer is not configured; set WC_SKARBIEC_CONSUMER")]
    MissingConsumer,
    #[error("Skarbiec grant file is not configured; set WC_SKARBIEC_TOKEN_FILE")]
    MissingTokenFile,
    #[error("cannot read Skarbiec grant file {path}: {source}")]
    TokenFile {
        path: String,
        source: std::io::Error,
    },
    #[error("Skarbiec grant file {0} must not be accessible by group or other users")]
    InsecureTokenFile(String),
    #[error("Skarbiec grant file {0} is empty")]
    EmptyToken(String),
    #[error("Skarbiec request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Skarbiec returned HTTP {status}: {detail}")]
    Response { status: u16, detail: String },
    #[error("Skarbiec item {0:?} has no value")]
    MissingValue(String),
    #[error("Skarbiec deployment configuration: {0}")]
    Deployment(String),
    #[error("cannot acquire GCP workload or Skarbiec identity: {0}")]
    GcpAuth(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ItemInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub item_type: Option<String>,
    pub tags: Option<Vec<String>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub deleted: Option<bool>,
    pub versions: Option<usize>,
}

fn checked_url(base_url: &str) -> Result<String, SkarbiecError> {
    let base_url = base_url.trim().trim_end_matches('/');
    let parsed =
        url::Url::parse(base_url).map_err(|_| SkarbiecError::InvalidUrl(base_url.to_string()))?;
    let loopback = parsed
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|host| host.is_loopback())
        || parsed.host_str() == Some("localhost");
    let transport_allowed = parsed.scheme() == "https" || (parsed.scheme() == "http" && loopback);
    if !transport_allowed
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || (parsed.path() != "/" && !parsed.path().is_empty())
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(SkarbiecError::InvalidUrl(base_url.to_string()));
    }
    Ok(base_url.to_string())
}

#[cfg(unix)]
fn reject_insecure_mode(path: &Path) -> Result<(), SkarbiecError> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path).map_err(|source| SkarbiecError::TokenFile {
        path: path.display().to_string(),
        source,
    })?;
    let non_owner_mask = u32::from(u8::MAX >> (u16::BITS / u8::BITS));
    if metadata.permissions().mode() & non_owner_mask != u32::MIN {
        return Err(SkarbiecError::InsecureTokenFile(path.display().to_string()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn reject_insecure_mode(_path: &Path) -> Result<(), SkarbiecError> {
    Ok(())
}

pub(crate) fn read_grant(path: &str) -> Result<String, SkarbiecError> {
    if path.trim().is_empty() {
        return Err(SkarbiecError::MissingTokenFile);
    }
    let path = Path::new(path);
    reject_insecure_mode(path)?;
    let token = std::fs::read_to_string(path).map_err(|source| SkarbiecError::TokenFile {
        path: path.display().to_string(),
        source,
    })?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(SkarbiecError::EmptyToken(path.display().to_string()));
    }
    Ok(token)
}
static AGENT_GRANTS: LazyLock<Mutex<HashMap<(String, String), String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const TRANSIENT_AGENT_GRANT_DIR: &str = "/run/stado-agent-credentials";

/// Erase the protected-settings handoff after an agent has cached its grant.
/// Persistent local/Darwin agent grants live elsewhere and intentionally
/// survive process restarts.
fn erase_transient_agent_grant(path: &str, byte_count: usize) {
    let path = Path::new(path);
    if path.parent() != Some(Path::new(TRANSIENT_AGENT_GRANT_DIR)) {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = std::io::copy(
            &mut std::io::repeat(u8::MIN).take(u64::try_from(byte_count).unwrap_or(u64::MAX)),
            &mut file,
        );
        let _ = file.set_len(u64::MIN);
        let _ = file.sync_all();
    }
    let _ = std::fs::remove_file(path);
}

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

    /// Read one item with the configured Stado consumer grant.
    pub async fn configured_item(id: &str) -> Result<Value, SkarbiecError> {
        Self::configured()?.read_item(id).await
    }
}

/// Resolve one optional string field from a Skarbiec item. A missing item is
/// `None`; authentication, transport, schema, and authorization failures remain
/// explicit errors and never trigger an alternate credential source.
pub async fn read_string(id: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::configured()?.read_string(id, field).await
}

/// GCP authentication without ADC files, process credentials, or gcloud
/// sessions. Prefer an on-platform managed identity; off GCP, resolve the
/// `service_account` JSON field from the `stado-gcp` Skarbiec item.
pub async fn gcp_provider() -> Result<std::sync::Arc<dyn gcp_auth::TokenProvider>, SkarbiecError> {
    if let Ok(identity) = gcp_auth::MetadataServiceAccount::new().await {
        return Ok(std::sync::Arc::new(identity));
    }
    let value = Client::configured_item("stado-gcp").await?;
    let service_account = value
        .get("service_account")
        .ok_or_else(|| SkarbiecError::MissingValue("stado-gcp/service_account".into()))?;
    let encoded = match service_account {
        Value::String(value) => value.clone(),
        value => {
            serde_json::to_string(value).map_err(|err| SkarbiecError::GcpAuth(err.to_string()))?
        }
    };
    let identity = gcp_auth::CustomServiceAccount::from_json(&encoded)
        .map_err(|err| SkarbiecError::GcpAuth(err.to_string()))?;
    Ok(std::sync::Arc::new(identity))
}
