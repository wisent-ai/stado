//! What a consume request, and a restored record, must already be.

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

pub(super) fn max_exact_json_integer() -> u64 {
    (u64::from(true) << f64::MANTISSA_DIGITS) - u64::from(true)
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
