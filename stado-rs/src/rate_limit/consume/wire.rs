//! The consume request and response documents on the wire.

use serde::{Deserialize, Serialize};

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
