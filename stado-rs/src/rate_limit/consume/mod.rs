//! The fixed-window counter itself: persisted records and the consume path.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::queue::JobStorage;

use super::error::RateLimitError;
use super::policy::RateLimitClient;

mod validate;
mod wire;

pub use wire::{ConsumeRequest, ConsumeResponse};

use validate::{max_exact_json_integer, record_id, validate_record, validate_request};

const STATE_PATH: &str = "rate-limit/records.json";

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
