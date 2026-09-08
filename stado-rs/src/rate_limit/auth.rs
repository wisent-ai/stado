//! Bearer authentication against the Skarbiec verifier grant.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::rate_limit::{clients, RateLimitClient, RateLimitError};
use crate::skarbiec::Client as SkarbiecClient;

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
