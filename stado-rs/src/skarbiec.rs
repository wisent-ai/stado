//! Client for the separate Skarbiec credential service.
//!
//! Skarbiec is reached over its loopback HTTP API or a TLS-protected remote
//! endpoint. Application credentials are decrypted by Skarbiec, authorized by
//! an action-scoped consumer grant, and held only in the requesting Stado
//! process. Stado has no environment, queue-storage, cloud-secret-manager, or
//! local credential fallback.

use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

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
    let loopback = match parsed.host() {
        Some(url::Host::Ipv4(host)) => host.is_loopback(),
        Some(url::Host::Ipv6(host)) => host.is_loopback(),
        Some(url::Host::Domain(host)) => host == "localhost",
        None => false,
    };
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

    /// Dedicated verifier used only for namespace-scoped product object
    /// bearers. It never reuses the coordinator's broader Skarbiec grant.
    pub fn object_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::object_skarbiec_consumer() != crate::config::OBJECT_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "object verifier consumer must be {:?}",
                crate::config::OBJECT_API_VERIFIER_CONSUMER
            )));
        }
        if crate::config::object_skarbiec_token_file() == crate::config::skarbiec_token_file() {
            return Err(SkarbiecError::Deployment(
                "object verifier token file must be distinct from the coordinator grant"
                    .to_string(),
            ));
        }
        Self::new(
            crate::config::object_skarbiec_url(),
            crate::config::object_skarbiec_consumer(),
            crate::config::object_skarbiec_token_file(),
        )
    }

    /// Dedicated verifier for immutable authenticated release publication.
    pub fn release_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::release_skarbiec_consumer()
            != crate::config::RELEASE_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "release verifier consumer must be {:?}",
                crate::config::RELEASE_API_VERIFIER_CONSUMER
            )));
        }
        let token_file = crate::config::release_skarbiec_token_file();
        if token_file == crate::config::skarbiec_token_file()
            || token_file == crate::config::object_skarbiec_token_file()
        {
            return Err(SkarbiecError::Deployment(
                "release verifier token file must be distinct from coordinator and product-object verifier grants"
                    .to_string(),
            ));
        }
        Self::new(
            crate::config::release_skarbiec_url(),
            crate::config::release_skarbiec_consumer(),
            token_file,
        )
    }

    /// Dedicated verifier for exact machine client bearers.
    pub fn machine_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::machine_skarbiec_consumer()
            != crate::config::MACHINE_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "machine verifier consumer must be {:?}",
                crate::config::MACHINE_API_VERIFIER_CONSUMER
            )));
        }
        let token_file = crate::config::machine_skarbiec_token_file();
        if token_file == crate::config::skarbiec_token_file()
            || token_file == crate::config::agent_skarbiec_token_file()
            || token_file == crate::config::object_skarbiec_token_file()
            || token_file == crate::config::release_skarbiec_token_file()
            || token_file == crate::config::service_skarbiec_token_file()
        {
            return Err(SkarbiecError::Deployment(
                "machine verifier token file must be distinct from coordinator, workload-agent, object, release, and service verifier grants"
                    .to_string(),
            ));
        }
        Self::new(
            crate::config::machine_skarbiec_url(),
            crate::config::machine_skarbiec_consumer(),
            token_file,
        )
    }

    /// Dedicated verifier for exact Stado push ingress client bearers.
    pub fn backend_push_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::backend_push_skarbiec_consumer()
            != crate::config::BACKEND_PUSH_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "backend push verifier consumer must be {:?}",
                crate::config::BACKEND_PUSH_API_VERIFIER_CONSUMER
            )));
        }
        let token_file = crate::config::backend_push_skarbiec_token_file();
        if token_file == crate::config::skarbiec_token_file()
            || token_file == crate::config::agent_skarbiec_token_file()
            || token_file == crate::config::object_skarbiec_token_file()
            || token_file == crate::config::release_skarbiec_token_file()
            || token_file == crate::config::machine_skarbiec_token_file()
            || token_file == crate::config::service_skarbiec_token_file()
            || token_file == crate::config::rate_limit_skarbiec_token_file()
            || token_file == crate::config::backend_messaging_skarbiec_token_file()
        {
            return Err(SkarbiecError::Deployment(
                "backend push verifier token file must be distinct from every control, workload, messaging, and API verifier grant"
                    .to_string(),
            ));
        }
        Self::new(
            crate::config::backend_push_skarbiec_url(),
            crate::config::backend_push_skarbiec_consumer(),
            token_file,
        )
    }

    /// Dedicated verifier for exact managed-service deployer bearers.
    pub fn service_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::service_skarbiec_consumer()
            != crate::config::SERVICE_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "service verifier consumer must be {:?}",
                crate::config::SERVICE_API_VERIFIER_CONSUMER
            )));
        }
        let token_file = crate::config::service_skarbiec_token_file();
        if token_file == crate::config::skarbiec_token_file()
            || token_file == crate::config::object_skarbiec_token_file()
            || token_file == crate::config::release_skarbiec_token_file()
            || token_file == crate::config::machine_skarbiec_token_file()
        {
            return Err(SkarbiecError::Deployment(
                "service verifier token file must be distinct from coordinator, product-object, and release verifier grants"
                    .to_string(),
            ));
        }
        Self::new(
            crate::config::service_skarbiec_url(),
            crate::config::service_skarbiec_consumer(),
            token_file,
        )
    }

    /// Dedicated verifier for shared rate-limit client bearers.
    pub fn rate_limit_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::rate_limit_skarbiec_consumer()
            != crate::config::RATE_LIMIT_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "rate-limit verifier consumer must be {:?}",
                crate::config::RATE_LIMIT_API_VERIFIER_CONSUMER
            )));
        }
        let token_file = crate::config::rate_limit_skarbiec_token_file();
        if token_file == crate::config::skarbiec_token_file()
            || token_file == crate::config::object_skarbiec_token_file()
            || token_file == crate::config::release_skarbiec_token_file()
            || token_file == crate::config::machine_skarbiec_token_file()
            || token_file == crate::config::service_skarbiec_token_file()
            || token_file == crate::config::agent_skarbiec_token_file()
            || token_file == crate::config::backend_push_skarbiec_token_file()
            || token_file == crate::config::backend_messaging_skarbiec_token_file()
        {
            return Err(SkarbiecError::Deployment(
                "rate-limit verifier token file must be distinct from every other verifier grant"
                    .to_string(),
            ));
        }
        Self::new(
            crate::config::rate_limit_skarbiec_url(),
            crate::config::rate_limit_skarbiec_consumer(),
            token_file,
        )
    }

    /// Dedicated verifier for finite integration client bearers.
    pub fn integration_verifier() -> Result<Self, SkarbiecError> {
        if crate::config::integration_skarbiec_consumer()
            != crate::config::INTEGRATION_API_VERIFIER_CONSUMER
        {
            return Err(SkarbiecError::Deployment(format!(
                "integration verifier consumer must be {:?}",
                crate::config::INTEGRATION_API_VERIFIER_CONSUMER
            )));
        }
        let token_file = crate::config::integration_skarbiec_token_file();
        if [
            crate::config::skarbiec_token_file(),
            crate::config::agent_skarbiec_token_file(),
            crate::config::object_skarbiec_token_file(),
            crate::config::release_skarbiec_token_file(),
            crate::config::machine_skarbiec_token_file(),
            crate::config::service_skarbiec_token_file(),
            crate::config::rate_limit_skarbiec_token_file(),
            crate::config::backend_push_skarbiec_token_file(),
            crate::config::backend_messaging_skarbiec_token_file(),
        ]
        .contains(&token_file)
        {
            return Err(SkarbiecError::Deployment(
                "integration verifier token file must be distinct from control-plane, workload-agent, messaging, and every other verifier grant"
                    .to_string(),
            ));
        }
        Self::new(
            crate::config::integration_skarbiec_url(),
            crate::config::integration_skarbiec_consumer(),
            token_file,
        )
    }

    /// Exact provider grant for one finite integration domain.
    pub fn integration_provider(domain: &str) -> Result<Self, SkarbiecError> {
        let provider = crate::config::integration_provider(domain).ok_or_else(|| {
            SkarbiecError::Deployment(format!(
                "integration provider domain {domain:?} is not configured"
            ))
        })?;
        let token_file = provider.token_file();
        if [
            crate::config::skarbiec_token_file(),
            crate::config::agent_skarbiec_token_file(),
            crate::config::integration_skarbiec_token_file(),
            crate::config::object_skarbiec_token_file(),
            crate::config::release_skarbiec_token_file(),
            crate::config::machine_skarbiec_token_file(),
            crate::config::service_skarbiec_token_file(),
            crate::config::rate_limit_skarbiec_token_file(),
            crate::config::backend_push_skarbiec_token_file(),
            crate::config::backend_messaging_skarbiec_token_file(),
        ]
        .contains(&token_file)
        {
            return Err(SkarbiecError::Deployment(format!(
                "integration provider token file for domain {domain:?} is not isolated"
            )));
        }
        Self::new(
            crate::config::integration_provider_skarbiec_url(),
            provider.consumer(),
            token_file,
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
        // A refusal here is almost always a grant-scope decision, not a broken token, and the
        // bare upstream body says neither which consumer asked nor what it asked for. That cost
        // real time: `billing watch` reads provider credentials through the coordinator grant,
        // which is documented as entitled only to route-local machine and host-health items, and
        // the resulting "consumer not authorized to read item" was repeatedly read as a
        // credential fault. Name both, so the reader sees a missing entitlement instead of
        // hunting a token.
        let body = Self::response_json(response)
            .await
            .map_err(|err| match err {
                SkarbiecError::Response { status, detail }
                    if status == reqwest::StatusCode::FORBIDDEN.as_u16() =>
                {
                    SkarbiecError::Response {
                        status,
                        detail: format!(
                            "{detail} (consumer {:?} asked for item {id:?}; if that consumer is \
                             not entitled to this item, the grant is the thing to change, not the \
                             token)",
                            self.consumer
                        ),
                    }
                }
                other => other,
            })?;
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

pub async fn read_integration_token(
    item: &str,
    field: &str,
) -> Result<Option<String>, SkarbiecError> {
    Client::integration_verifier()?
        .read_string(item, field)
        .await
}

/// Validate the auth verifier independently from all provider domains.
pub async fn validate_integration_verifier() -> Result<usize, SkarbiecError> {
    let clients = crate::config::integration_clients().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid integration.clients: {}",
            problems.join("; ")
        ))
    })?;
    let verifier = Client::integration_verifier()?;
    let expected = clients
        .values()
        .map(|policy| policy.item().to_string())
        .collect::<BTreeSet<_>>();
    let visible = verifier
        .list_items()
        .await?
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    if visible != expected {
        return Err(SkarbiecError::Deployment(
            "integration verifier grant item set mismatch".to_string(),
        ));
    }

    let verifier_grant = read_grant(crate::config::integration_skarbiec_token_file())?;
    let verifier_digest = Sha256::digest(verifier_grant.as_bytes()).to_vec();
    let mut bearer_digests = BTreeSet::new();
    for (name, policy) in clients {
        let bearer = verifier
            .read_string(policy.item(), "token")
            .await?
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "integration bearer item is missing for client {name:?}"
                ))
            })?;
        let digest = Sha256::digest(bearer.as_bytes()).to_vec();
        if digest == verifier_digest || !bearer_digests.insert(digest) {
            return Err(SkarbiecError::Deployment(
                "integration verifier and all client bearer values must be distinct".to_string(),
            ));
        }
    }
    Ok(clients.len())
}

pub async fn validate_integration_provider(domain: &str) -> Result<usize, SkarbiecError> {
    let policy = crate::config::integration_provider(domain).ok_or_else(|| {
        SkarbiecError::Deployment(format!(
            "integration provider domain {domain:?} is not configured"
        ))
    })?;
    let provider = Client::integration_provider(domain)?;
    let expected = policy.items().iter().cloned().collect::<BTreeSet<_>>();
    let visible = provider
        .list_items()
        .await?
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    if visible != expected {
        return Err(SkarbiecError::Deployment(format!(
            "integration provider grant item set mismatch for domain {domain:?}"
        )));
    }
    Ok(expected.len())
}

pub async fn validate_integration_boundary() -> Result<usize, SkarbiecError> {
    let mut total = validate_integration_verifier().await?;
    let providers = crate::config::integration_providers().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid integration.providers: {}",
            problems.join("; ")
        ))
    })?;
    for domain in providers.keys() {
        total += validate_integration_provider(domain).await?;
    }
    Ok(total)
}

/// Resolve one product object bearer through the dedicated verifier grant.
/// Callers must select `item` from the canonical namespace policy first.
pub async fn read_object_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::object_verifier()?.read_string(item, field).await
}

/// Startup/doctor validation for the complete object authorization boundary.
/// The verifier grant must expose exactly the mapped items, every token must
/// be present, and tokens must be pairwise distinct so no bearer can cross a
/// namespace even after an accidental duplicate secret rotation.
pub async fn validate_object_verifier() -> Result<usize, SkarbiecError> {
    let namespaces = crate::config::object_api_namespaces().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid object_api.namespaces: {}",
            problems.join("; ")
        ))
    })?;
    let client = Client::object_verifier()?;
    let expected = namespaces
        .values()
        .map(|policy| policy.item().to_string())
        .collect::<BTreeSet<_>>();
    let visible = client
        .list_items()
        .await?
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    if visible != expected {
        let missing = expected
            .difference(&visible)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let unexpected = visible
            .difference(&expected)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        return Err(SkarbiecError::Deployment(format!(
            "object verifier grant item set mismatch (missing=[{missing}], unexpected=[{unexpected}])"
        )));
    }

    let mut token_owners = HashMap::<Vec<u8>, &str>::new();
    for (namespace, policy) in namespaces {
        let token = client
            .read_string(policy.item(), "token")
            .await?
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "Skarbiec item {}/token is missing or empty for namespace {namespace}",
                    policy.item()
                ))
            })?;
        let digest = Sha256::digest(token.as_bytes()).to_vec();
        if let Some(other) = token_owners.insert(digest, namespace.as_str()) {
            return Err(SkarbiecError::Deployment(format!(
                "object bearer values for namespaces {other} and {namespace} must be distinct"
            )));
        }
    }
    Ok(namespaces.len())
}

pub async fn read_release_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::release_verifier()?.read_string(item, field).await
}

/// Validate the immutable release-publisher verifier and ensure its bearers
/// cannot collide with any product object bearer.
pub async fn validate_release_verifier() -> Result<usize, SkarbiecError> {
    let publishers = crate::config::release_api_publishers().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid release_api.publishers: {}",
            problems.join("; ")
        ))
    })?;
    let client = Client::release_verifier()?;
    let expected = publishers
        .values()
        .map(|policy| policy.item().to_string())
        .collect::<BTreeSet<_>>();
    let visible = client
        .list_items()
        .await?
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    if visible != expected {
        let missing = expected
            .difference(&visible)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let unexpected = visible
            .difference(&expected)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        return Err(SkarbiecError::Deployment(format!(
            "release verifier grant item set mismatch (missing=[{missing}], unexpected=[{unexpected}])"
        )));
    }

    let object_namespaces = crate::config::object_api_namespaces().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid object_api.namespaces: {}",
            problems.join("; ")
        ))
    })?;
    let object_client = Client::object_verifier()?;
    let mut token_owners = HashMap::<Vec<u8>, String>::new();
    for (namespace, policy) in object_namespaces {
        let token = object_client
            .read_string(policy.item(), "token")
            .await?
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "Skarbiec item {}/token is missing or empty for namespace {namespace}",
                    policy.item()
                ))
            })?;
        token_owners.insert(
            Sha256::digest(token.as_bytes()).to_vec(),
            format!("object namespace {namespace}"),
        );
    }
    for (product, policy) in publishers {
        let token = client
            .read_string(policy.item(), "token")
            .await?
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "Skarbiec item {}/token is missing or empty for release publisher {product}",
                    policy.item()
                ))
            })?;
        let digest = Sha256::digest(token.as_bytes()).to_vec();
        if let Some(other) = token_owners.insert(digest, format!("release publisher {product}")) {
            return Err(SkarbiecError::Deployment(format!(
                "bearer values for {other} and release publisher {product} must be distinct"
            )));
        }
    }
    Ok(publishers.len())
}

pub async fn read_machine_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::machine_verifier()?.read_string(item, field).await
}

/// Validate the complete machine-client authorization boundary. The verifier
/// sees exactly the mapped client items, every bearer is present, and machine
/// bearers are distinct from every other ingress bearer.
pub async fn validate_machine_verifier() -> Result<usize, SkarbiecError> {
    let clients = crate::config::machine_api_clients().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid machine_api.clients: {}",
            problems.join("; ")
        ))
    })?;
    let client = Client::machine_verifier()?;
    let expected = clients
        .values()
        .map(|policy| policy.item().to_string())
        .collect::<BTreeSet<_>>();
    let visible = client
        .list_items()
        .await?
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    if visible != expected {
        let missing = expected
            .difference(&visible)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let unexpected = visible
            .difference(&expected)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        return Err(SkarbiecError::Deployment(format!(
            "machine verifier grant item set mismatch (missing=[{missing}], unexpected=[{unexpected}])"
        )));
    }

    let mut token_owners = HashMap::<Vec<u8>, String>::new();
    let object_client = Client::object_verifier()?;
    let namespaces = crate::config::object_api_namespaces().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid object_api.namespaces while validating machine bearers: {}",
            problems.join("; ")
        ))
    })?;
    for (namespace, policy) in namespaces {
        if let Some(token) = object_client.read_string(policy.item(), "token").await? {
            token_owners.insert(
                Sha256::digest(token.as_bytes()).to_vec(),
                format!("object namespace {namespace}"),
            );
        }
    }
    let release_client = Client::release_verifier()?;
    let publishers = crate::config::release_api_publishers().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid release_api.publishers while validating machine bearers: {}",
            problems.join("; ")
        ))
    })?;
    for (product, policy) in publishers {
        if let Some(token) = release_client.read_string(policy.item(), "token").await? {
            token_owners.insert(
                Sha256::digest(token.as_bytes()).to_vec(),
                format!("release publisher {product}"),
            );
        }
    }
    let service_client = Client::service_verifier()?;
    let deployers = crate::config::service_api_deployers().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid service_api.deployers while validating machine bearers: {}",
            problems.join("; ")
        ))
    })?;
    for (product, policy) in deployers {
        if let Some(token) = service_client.read_string(policy.item(), "token").await? {
            token_owners.insert(
                Sha256::digest(token.as_bytes()).to_vec(),
                format!("service deployer {product}"),
            );
        }
    }
    for (name, policy) in clients {
        let token = client
            .read_string(policy.item(), "token")
            .await?
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "Skarbiec item {}/token is missing or empty for machine client {name}",
                    policy.item()
                ))
            })?;
        let digest = Sha256::digest(token.as_bytes()).to_vec();
        if let Some(other) = token_owners.insert(digest, format!("machine client {name}")) {
            return Err(SkarbiecError::Deployment(format!(
                "bearer values for {other} and machine client {name} must be distinct"
            )));
        }
    }
    Ok(clients.len())
}

pub async fn read_backend_push_token(
    item: &str,
    field: &str,
) -> Result<Option<String>, SkarbiecError> {
    Client::backend_push_verifier()?
        .read_string(item, field)
        .await
}

/// Validate exact push-client visibility and reject any reused client bearer.
pub async fn validate_backend_push_verifier() -> Result<usize, SkarbiecError> {
    let clients = crate::config::backend_push_clients().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid backend.push_clients: {}",
            problems.join("; ")
        ))
    })?;
    let client = Client::backend_push_verifier()?;
    let expected = clients
        .values()
        .map(|policy| policy.item().to_string())
        .collect::<BTreeSet<_>>();
    let visible = client
        .list_items()
        .await?
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    if visible != expected {
        let missing = expected
            .difference(&visible)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let unexpected = visible
            .difference(&expected)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        return Err(SkarbiecError::Deployment(format!(
            "backend push verifier grant item set mismatch (missing=[{missing}], unexpected=[{unexpected}])"
        )));
    }
    let mut token_owners = HashMap::<Vec<u8>, String>::new();
    for (name, policy) in clients {
        let token = client
            .read_string(policy.item(), "token")
            .await?
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "Skarbiec item {}/token is missing or empty for backend push client {name}",
                    policy.item()
                ))
            })?;
        let digest = Sha256::digest(token.as_bytes()).to_vec();
        if let Some(other) = token_owners.insert(digest, format!("backend push client {name}")) {
            return Err(SkarbiecError::Deployment(format!(
                "bearer values for {other} and backend push client {name} must be distinct"
            )));
        }
    }
    Ok(clients.len())
}

pub async fn read_service_token(item: &str, field: &str) -> Result<Option<String>, SkarbiecError> {
    Client::service_verifier()?.read_string(item, field).await
}

/// Validate the complete managed-service authorization boundary. The verifier
/// sees exactly the mapped deployer items, each token is non-empty, and no
/// service bearer collides with another service, object, or release bearer.
pub async fn validate_service_verifier() -> Result<usize, SkarbiecError> {
    let deployers = crate::config::service_api_deployers().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid service_api.deployers: {}",
            problems.join("; ")
        ))
    })?;
    let client = Client::service_verifier()?;
    let expected = deployers
        .values()
        .map(|policy| policy.item().to_string())
        .collect::<BTreeSet<_>>();
    let visible = client
        .list_items()
        .await?
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    if visible != expected {
        let missing = expected
            .difference(&visible)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let unexpected = visible
            .difference(&expected)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        return Err(SkarbiecError::Deployment(format!(
            "service verifier grant item set mismatch (missing=[{missing}], unexpected=[{unexpected}])"
        )));
    }
    let mut token_owners = HashMap::<Vec<u8>, String>::new();
    let object_client = Client::object_verifier()?;
    let namespaces = crate::config::object_api_namespaces().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid object_api.namespaces while validating service bearers: {}",
            problems.join("; ")
        ))
    })?;
    for (namespace, policy) in namespaces {
        if let Some(token) = object_client.read_string(policy.item(), "token").await? {
            token_owners.insert(
                Sha256::digest(token.as_bytes()).to_vec(),
                format!("object namespace {namespace}"),
            );
        }
    }
    let release_client = Client::release_verifier()?;
    let publishers = crate::config::release_api_publishers().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid release_api.publishers while validating service bearers: {}",
            problems.join("; ")
        ))
    })?;
    for (product, policy) in publishers {
        if let Some(token) = release_client.read_string(policy.item(), "token").await? {
            token_owners.insert(
                Sha256::digest(token.as_bytes()).to_vec(),
                format!("release publisher {product}"),
            );
        }
    }
    for (product, policy) in deployers {
        let token = client
            .read_string(policy.item(), "token")
            .await?
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "Skarbiec item {}/token is missing or empty for service deployer {product}",
                    policy.item()
                ))
            })?;
        let digest = Sha256::digest(token.as_bytes()).to_vec();
        if let Some(other) = token_owners.insert(digest, format!("service deployer {product}")) {
            return Err(SkarbiecError::Deployment(format!(
                "bearer values for {other} and service deployer {product} must be distinct"
            )));
        }
    }
    Ok(deployers.len())
}

/// GCP authentication through the adapter host's metadata identity or the
/// adapter's scoped `stado-gcp` Skarbiec item. Static ADC files, gcloud
/// sessions, process-environment credentials, and workload-agent grants are
/// deliberately unsupported provider credential sources.
pub async fn gcp_provider() -> Result<std::sync::Arc<dyn gcp_auth::TokenProvider>, SkarbiecError> {
    match gcp_auth::MetadataServiceAccount::new().await {
        Ok(identity) => Ok(std::sync::Arc::new(identity)),
        Err(metadata_error) => {
            let item = Client::configured_item("stado-gcp").await.map_err(|error| {
                SkarbiecError::GcpAuth(format!(
                    "GCP metadata identity is unavailable ({metadata_error}); scoped stado-gcp read failed: {error}"
                ))
            })?;
            let credential_json = item
                .get("service_account_json")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    (item.get("client_email").is_some() && item.get("private_key").is_some())
                        .then(|| serde_json::to_string(&item))
                        .transpose()
                        .ok()
                        .flatten()
                })
                .ok_or_else(|| {
                    SkarbiecError::GcpAuth(
                        "stado-gcp must contain service_account_json or a service-account JSON object"
                            .to_string(),
                    )
                })?;
            let identity =
                gcp_auth::CustomServiceAccount::from_json(&credential_json).map_err(|error| {
                    SkarbiecError::GcpAuth(format!(
                        "stado-gcp service-account JSON is invalid: {error}"
                    ))
                })?;
            Ok(std::sync::Arc::new(identity))
        }
    }
}
