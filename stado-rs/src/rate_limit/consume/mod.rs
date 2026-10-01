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

use validate::{record_id, used, validate_record, validate_request};

/// One window per record; the file name changed when a record stopped
/// carrying a counter and started carrying the hits it counts.
const STATE_PATH: &str = "rate-limit/windows.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    client: String,
    namespace: String,
    key: String,
    limit: u64,
    window_ms: u64,
    /// When each allowed request in this window arrived, epoch milliseconds.
    hits: Vec<u64>,
    reset_at: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
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
                let persisted: State = serde_json::from_str(&encoded)
                    .map_err(|error| RateLimitError::State(error.to_string()))?;
                for (id, record) in &persisted.records {
                    validate_record(id, record)?;
                }
                persisted
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
            if used(record) >= request.limit {
                let wait = Duration::from_millis(record.reset_at.saturating_sub(now));
                ConsumeResponse {
                    allowed: false,
                    limit: request.limit,
                    remaining: request.limit.saturating_sub(used(record)),
                    reset_at: record.reset_at,
                    retry_after_seconds: Some(wait.as_secs_f64().ceil() as u64),
                }
            } else {
                record.hits.push(now);
                ConsumeResponse {
                    allowed: true,
                    limit: request.limit,
                    remaining: request.limit.saturating_sub(used(record)),
                    reset_at: record.reset_at,
                    retry_after_seconds: None,
                }
            }
        } else {
            let record = Record {
                client: client.name().to_string(),
                namespace: request.namespace.clone(),
                key: request.key.clone(),
                limit: request.limit,
                window_ms: request.window_ms,
                hits: vec![now],
                reset_at,
            };
            let response = ConsumeResponse {
                allowed: true,
                limit: request.limit,
                remaining: request.limit.saturating_sub(used(&record)),
                reset_at,
                retry_after_seconds: None,
            };
            state.records.insert(id, record);
            response
        };

        if state.records != previous.records {
            let encoded = match serde_json::to_string(&*state) {
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
