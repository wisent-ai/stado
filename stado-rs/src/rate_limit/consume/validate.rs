//! What a consume request, and a restored record, must already be.

use std::num::NonZeroU64;

use sha2::{Digest, Sha256};

use crate::rate_limit::{clients, ConsumeRequest, RateLimitClient, RateLimitError};

use super::Record;

pub(super) fn validate_request(
    client: &RateLimitClient,
    request: &ConsumeRequest,
) -> Result<(), RateLimitError> {
    if !client.allows_namespace(&request.namespace) {
        return Err(RateLimitError::InvalidRequest(
            "namespace is outside the authenticated client policy".to_string(),
        ));
    }
    let lowercase_hex = request
        .key
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !lowercase_hex
        || !hex::decode(&request.key).is_ok_and(|bytes| bytes.len() == Sha256::output_size())
    {
        return Err(RateLimitError::InvalidRequest(
            "key must be a lowercase SHA-256 digest".to_string(),
        ));
    }
    if NonZeroU64::new(request.limit).is_none() || NonZeroU64::new(request.window_ms).is_none() {
        return Err(RateLimitError::InvalidRequest(
            "limit and window_ms must be non-zero".to_string(),
        ));
    }
    Ok(())
}

/// How many requests this window has already allowed.
pub(super) fn used(record: &Record) -> u64 {
    u64::try_from(record.hits.len()).unwrap_or(u64::MAX)
}

pub(super) fn record_id(
    client: &str,
    namespace: &str,
    key: &str,
    limit: u64,
    window_ms: u64,
) -> String {
    format!("{client}/{namespace}/{key}/{limit}/{window_ms}")
}

pub(super) fn validate_record(id: &str, record: &Record) -> Result<(), RateLimitError> {
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
        || record.hits.is_empty()
        || used(record) > record.limit
        || NonZeroU64::new(record.reset_at).is_none()
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
