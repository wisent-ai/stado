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

/// How many requests this window has already allowed. `usize` is at most 64
/// bits on every target Rust builds for, so the widening is lossless.
pub(super) fn used(record: &Record) -> u64 {
    record.hits.len() as u64
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

/// A restored record is refused with the one property it breaks, so the
/// state file that holds it can be repaired from the sentence alone.
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
    let refused =
        |why: String| RateLimitError::State(format!("record {id:?} is not canonical: {why}"));
    validate_request(client, &request).map_err(|error| refused(error.to_string()))?;
    if record.hits.is_empty() {
        return Err(refused("it holds no hits".to_string()));
    }
    if used(record) > record.limit {
        return Err(refused(format!(
            "it holds {} hits over its limit of {}",
            record.hits.len(),
            record.limit
        )));
    }
    if NonZeroU64::new(record.reset_at).is_none() {
        return Err(refused("its reset_at is zero".to_string()));
    }
    let expected = record_id(
        &record.client,
        &record.namespace,
        &record.key,
        record.limit,
        record.window_ms,
    );
    if id != expected {
        return Err(refused(format!("its fields name the record {expected:?}")));
    }
    Ok(())
}
