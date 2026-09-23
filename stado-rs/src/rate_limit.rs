//! Authenticated, provider-neutral fixed-window rate limiting.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::queue::{JobStorage, StorageError};
use crate::skarbiec::{Client as SkarbiecClient, SkarbiecError};

const STATE_PATH: &str = "rate-limit/records.json";
const CONSUME_ACTION: &str = "consume";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClient {
    consumer: String,
    item: String,
    namespaces: Vec<String>,
    actions: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RateLimitClient {
    name: String,
    item: String,
    namespaces: BTreeSet<String>,
}

impl RateLimitClient {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn allows_namespace(&self, namespace: &str) -> bool {
        self.namespaces.contains(namespace)
    }
}

fn canonical_name(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(crate) fn parse_clients(
    value: Option<Value>,
) -> Result<BTreeMap<String, RateLimitClient>, String> {
    let Some(Value::Object(entries)) = value else {
        return Err("rate_limit.clients must be a non-empty client mapping".to_string());
    };
    if entries.is_empty() {
        return Err("rate_limit.clients must not be empty".to_string());
    }

    let mut clients = BTreeMap::new();
    let mut consumers = BTreeSet::new();
    let mut items = BTreeSet::new();
    let mut namespaces = BTreeSet::new();
    for (name, value) in entries {
        if !canonical_name(&name) {
            return Err(format!("rate_limit.clients key {name:?} is not canonical"));
        }
        let raw: RawClient = serde_json::from_value(value)
            .map_err(|error| format!("invalid rate_limit.clients.{name}: {error}"))?;
        let expected_consumer = format!("{name}-rate-limit-client");
        let expected_item = format!("{name}-rate-limit-api");
        if raw.consumer != expected_consumer {
            return Err(format!(
                "rate_limit.clients.{name}.consumer must be {expected_consumer:?}"
            ));
        }
        if raw.item != expected_item {
            return Err(format!(
                "rate_limit.clients.{name}.item must be {expected_item:?}"
            ));
        }
        if !consumers.insert(raw.consumer.clone()) || !items.insert(raw.item.clone()) {
            return Err("rate_limit.clients must use distinct consumers and items".to_string());
        }
        if raw.actions.as_slice() != [CONSUME_ACTION] {
            return Err(format!(
                "rate_limit.clients.{name}.actions must contain only consume"
            ));
        }
        if raw.namespaces.is_empty() {
            return Err(format!(
                "rate_limit.clients.{name}.namespaces must not be empty"
            ));
        }
        let mut client_namespaces = BTreeSet::new();
        for namespace in raw.namespaces {
            if !canonical_name(&namespace) || !client_namespaces.insert(namespace.clone()) {
                return Err(format!(
                    "rate_limit.clients.{name}.namespaces contains a malformed or duplicate namespace"
                ));
            }
            if !namespaces.insert(namespace.clone()) {
                return Err(format!(
                    "rate-limit namespace {namespace:?} is assigned to more than one client"
                ));
            }
        }
        clients.insert(
            name.clone(),
            RateLimitClient {
                name,
                item: raw.item,
                namespaces: client_namespaces,
            },
        );
    }
    Ok(clients)
}

static CLIENTS: LazyLock<Result<BTreeMap<String, RateLimitClient>, String>> = LazyLock::new(|| {
    let configured = match std::env::var("WC_RATE_LIMIT_CLIENTS")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
            Ok(value) => Some(value),
            Err(error) => return Err(format!("WC_RATE_LIMIT_CLIENTS must be JSON: {error}")),
        },
        None => crate::config_file::get("rate_limit.clients"),
    };
    parse_clients(configured)
});

pub fn clients() -> Result<&'static BTreeMap<String, RateLimitClient>, &'static str> {
    CLIENTS.as_ref().map_err(String::as_str)
}

#[derive(Debug, thiserror::Error)]
pub enum RateLimitError {
    #[error("invalid rate-limit configuration: {0}")]
    Configuration(String),
    #[error("invalid rate-limit request: {0}")]
    InvalidRequest(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Skarbiec(#[from] SkarbiecError),
    #[error("invalid persisted rate-limit state: {0}")]
    State(String),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumeRequest {
    pub namespace: String,
    pub key: String,
    pub limit: u64,
    pub window_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConsumeResponse {
    pub allowed: bool,
    pub limit: u64,
    pub remaining: u64,
    pub reset_at: u64,
    pub retry_after_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    client: String,
    namespace: String,
    key: String,
    limit: u64,
    window_ms: u64,
    count: u64,
    reset_at: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    records: BTreeMap<String, Record>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedState {
    schema_version: u8,
    records: BTreeMap<String, Record>,
}

#[derive(Clone)]
pub struct RateLimiter {
    store: JobStorage,
    state: Arc<Mutex<State>>,
}

impl RateLimiter {
    pub fn new(store: JobStorage) -> Self {
        Self {
            store,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub async fn restore(&self) -> Result<(), RateLimitError> {
        let restored = match self.store.download_text(STATE_PATH).await? {
            Some(encoded) => {
                let persisted: PersistedState = serde_json::from_str(&encoded)
                    .map_err(|error| RateLimitError::State(error.to_string()))?;
                if persisted.schema_version != u8::from(true) {
                    return Err(RateLimitError::State(format!(
                        "unsupported schema version {}",
                        persisted.schema_version
                    )));
                }
                for (id, record) in &persisted.records {
                    validate_record(id, record)?;
                }
                State {
                    records: persisted.records,
                }
            }
            None => State::default(),
        };
        *self.state.lock().await = restored;
        Ok(())
    }

    pub async fn consume(
        &self,
        client: &RateLimitClient,
        request: &ConsumeRequest,
    ) -> Result<ConsumeResponse, RateLimitError> {
        validate_request(client, request)?;
        let now = u64::try_from(chrono::Utc::now().timestamp_millis()).map_err(|_| {
            RateLimitError::InvalidRequest("system clock is before epoch".to_string())
        })?;
        let reset_at = now.checked_add(request.window_ms).ok_or_else(|| {
            RateLimitError::InvalidRequest("window overflows epoch time".to_string())
        })?;
        if reset_at > max_exact_json_integer() {
            return Err(RateLimitError::InvalidRequest(
                "window overflows exact JSON epoch time".to_string(),
            ));
        }
        let id = record_id(
            client.name(),
            &request.namespace,
            &request.key,
            request.limit,
            request.window_ms,
        );

        let mut state = self.state.lock().await;
        let previous = state.clone();
        state.records.retain(|_, record| record.reset_at > now);

        let response = if let Some(record) = state.records.get_mut(&id) {
            if record.count >= request.limit {
                let millis_per_second =
                    u64::try_from(Duration::from_secs(u64::from(true)).as_millis())
                        .expect("one second fits u64 milliseconds");
                ConsumeResponse {
                    allowed: false,
                    limit: request.limit,
                    remaining: u64::MIN,
                    reset_at: record.reset_at,
                    retry_after_seconds: Some(
                        record
                            .reset_at
                            .saturating_sub(now)
                            .div_ceil(millis_per_second),
                    ),
                }
            } else {
                record.count = record.count.checked_add(u64::from(true)).ok_or_else(|| {
                    RateLimitError::InvalidRequest("counter overflow".to_string())
                })?;
                ConsumeResponse {
                    allowed: true,
                    limit: request.limit,
                    remaining: request.limit - record.count,
                    reset_at: record.reset_at,
                    retry_after_seconds: None,
                }
            }
        } else {
            state.records.insert(
                id,
                Record {
                    client: client.name().to_string(),
                    namespace: request.namespace.clone(),
                    key: request.key.clone(),
                    limit: request.limit,
                    window_ms: request.window_ms,
                    count: u64::from(true),
                    reset_at,
                },
            );
            ConsumeResponse {
                allowed: true,
                limit: request.limit,
                remaining: request.limit - u64::from(true),
                reset_at,
                retry_after_seconds: None,
            }
        };

        if state.records != previous.records {
            let encoded = match serde_json::to_string(&PersistedState {
                schema_version: u8::from(true),
                records: state.records.clone(),
            }) {
                Ok(encoded) => encoded,
                Err(error) => {
                    *state = previous;
                    return Err(RateLimitError::State(error.to_string()));
                }
            };
            if let Err(error) = self.store.upload_text(STATE_PATH, &encoded).await {
                *state = previous;
                return Err(error.into());
            }
        }
        Ok(response)
    }
}

fn validate_request(
    client: &RateLimitClient,
    request: &ConsumeRequest,
) -> Result<(), RateLimitError> {
    if !client.allows_namespace(&request.namespace) {
        return Err(RateLimitError::InvalidRequest(
            "namespace is outside the authenticated client policy".to_string(),
        ));
    }
    if request.key.len() != Sha256::output_size()
        || !request
            .key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(RateLimitError::InvalidRequest(
            "key must be a lowercase SHA-256 digest".to_string(),
        ));
    }
    if request.limit == u64::MIN || request.window_ms == u64::MIN {
        return Err(RateLimitError::InvalidRequest(
            "limit and window_ms must be non-zero".to_string(),
        ));
    }
    if request.limit > max_exact_json_integer() || request.window_ms > max_exact_json_integer() {
        return Err(RateLimitError::InvalidRequest(
            "limit and window_ms must be exact JSON integers".to_string(),
        ));
    }
    Ok(())
}

fn max_exact_json_integer() -> u64 {
    (u64::from(true) << f64::MANTISSA_DIGITS) - u64::from(true)
}

fn record_id(client: &str, namespace: &str, key: &str, limit: u64, window_ms: u64) -> String {
    format!("{client}/{namespace}/{key}/{limit}/{window_ms}")
}

fn validate_record(id: &str, record: &Record) -> Result<(), RateLimitError> {
    let configured = clients()
        .map_err(|error| RateLimitError::State(format!("invalid client policy: {error}")))?;
    let client = configured
        .get(&record.client)
        .ok_or_else(|| RateLimitError::State(format!("record {id:?} names an unknown client")))?;
    let request = ConsumeRequest {
        namespace: record.namespace.clone(),
        key: record.key.clone(),
        limit: record.limit,
        window_ms: record.window_ms,
    };
    if validate_request(client, &request).is_err()
        || record.count == u64::MIN
        || record.count > record.limit
        || record.reset_at == u64::MIN
        || record.reset_at > max_exact_json_integer()
        || id
            != record_id(
                &record.client,
                &record.namespace,
                &record.key,
                record.limit,
                record.window_ms,
            )
    {
        return Err(RateLimitError::State(format!(
            "record {id:?} is not canonical"
        )));
    }
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let left = Sha256::digest(left);
    let right = Sha256::digest(right);
    let mut difference = u8::default();
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == u8::default()
}

pub async fn authenticate(
    supplied: &str,
) -> Result<Option<&'static RateLimitClient>, RateLimitError> {
    if supplied.is_empty() {
        return Ok(None);
    }
    let verifier = SkarbiecClient::rate_limit_verifier()?;
    let configured = clients().map_err(|error| RateLimitError::Configuration(error.to_string()))?;
    let mut matched = None;
    for client in configured.values() {
        let expected = verifier.read_string(client.item(), "token").await?;
        let Some(expected) = expected.filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        if constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
            if matched.is_some() {
                return Ok(None);
            }
            matched = Some(client);
        }
    }
    Ok(matched)
}

pub async fn validate_verifier() -> Result<usize, RateLimitError> {
    let configured = clients().map_err(|error| RateLimitError::Configuration(error.to_string()))?;
    let verifier = SkarbiecClient::rate_limit_verifier()?;
    let expected = configured
        .values()
        .map(|client| client.item().to_string())
        .collect::<BTreeSet<_>>();
    let visible = verifier
        .list_items()
        .await?
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    if visible != expected {
        return Err(RateLimitError::Configuration(
            "rate-limit verifier grant item set does not exactly match rate_limit.clients"
                .to_string(),
        ));
    }
    let mut tokens = BTreeSet::new();
    for client in configured.values() {
        let token = verifier
            .read_string(client.item(), "token")
            .await?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                RateLimitError::Configuration(format!(
                    "Skarbiec item {}/token is missing",
                    client.item()
                ))
            })?;
        if !tokens.insert(token) {
            return Err(RateLimitError::Configuration(
                "rate-limit client bearer values must be distinct".to_string(),
            ));
        }
    }
    Ok(configured.len())
}
