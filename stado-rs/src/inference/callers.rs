//! Outside callers of an inference deployment: who may call it, with which
//! limits and at which price, and what each called per hour.
//!
//! A caller is declared per deployment at
//! `inference/callers/<deployment>/<caller>.json` in the fleet store. The
//! declaration holds the SHA-256 of the caller's key, never the key: the key
//! is written once to the vault item `inference-caller-<deployment>-<caller>`
//! (field `token`) and printed once when issued, and the gateway compares the
//! hash of what a request presents. Usage is kept per caller and hour at
//! `inference/usage/<deployment>/<caller>/<hour>.json`, written by the one
//! gateway that serves the deployment.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::queue::{JobStorage, Listing, StorageError};

pub const CALLERS_PREFIX: &str = "inference/callers/";
pub const USAGE_PREFIX: &str = "inference/usage/";

/// What one caller may spend.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Requests in any 60 seconds.
    pub requests_per_minute: u32,
    /// Prompt plus generated tokens in one UTC day.
    pub tokens_per_day: u64,
}

/// What one caller is charged, per million tokens, when a price is declared.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub usd_per_million_prompt: f64,
    pub usd_per_million_generated: f64,
}

/// One caller of one deployment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Caller {
    pub caller: String,
    pub deployment: String,
    /// Hex SHA-256 of the caller's key.
    pub key_sha256: String,
    pub limits: Limits,
    pub price: Option<Price>,
    pub issued_at: String,
    /// When set, the key is refused from then on.
    pub revoked_at: Option<String>,
}

/// One caller's use in one hour.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HourUsage {
    pub caller: String,
    pub deployment: String,
    pub hour: String,
    pub requests: u64,
    pub prompt_tokens: u64,
    pub generated_tokens: u64,
    /// Answered requests whose answer stated no usage, so their tokens are
    /// not counted; never read as zero tokens silently.
    pub usage_unread: u64,
    /// Requests refused for a limit.
    pub refused: u64,
    /// Seconds the engine took, summed.
    pub seconds: f64,
}

impl HourUsage {
    pub fn tokens(&self) -> u64 {
        self.prompt_tokens + self.generated_tokens
    }

    /// The hour's charge at `price`.
    pub fn charge(&self, price: &Price) -> f64 {
        self.prompt_tokens as f64 / 1e6 * price.usd_per_million_prompt
            + self.generated_tokens as f64 / 1e6 * price.usd_per_million_generated
    }
}

/// Hex SHA-256 of a key.
pub fn key_hash(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

/// The vault item a caller's key is written to.
pub fn vault_item(deployment: &str, caller: &str) -> String {
    format!("inference-caller-{deployment}-{caller}")
}

/// A caller name: lowercase letters, digits and inner hyphens, since it is
/// an object key and part of a vault item id.
pub fn validate_name(caller: &str) -> Result<(), String> {
    let shaped = !caller.is_empty()
        && !caller.starts_with('-')
        && !caller.ends_with('-')
        && caller
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if shaped {
        Ok(())
    } else {
        Err(format!(
            "caller name {caller:?} must be lowercase letters, digits and inner hyphens"
        ))
    }
}

fn caller_path(deployment: &str, caller: &str) -> String {
    format!("{CALLERS_PREFIX}{deployment}/{caller}.json")
}

fn usage_path(deployment: &str, caller: &str, hour: &str) -> String {
    format!("{USAGE_PREFIX}{deployment}/{caller}/{hour}.json")
}

/// The start of the UTC hour `at` falls in, as usage keys name it.
pub fn hour_of(at: chrono::DateTime<chrono::Utc>) -> String {
    chrono::DurationRound::duration_trunc(at, chrono::Duration::hours(1))
        .unwrap_or(at)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub async fn load(
    store: &JobStorage,
    deployment: &str,
    caller: &str,
) -> Result<Option<Caller>, StorageError> {
    let key = caller_path(deployment, caller);
    let Some(text) = store.download_text(&key).await? else {
        return Ok(None);
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| StorageError::Other(format!("{key} is not a caller declaration: {error}")))
}

pub async fn save(store: &JobStorage, caller: &Caller) -> Result<(), StorageError> {
    let body = serde_json::to_string_pretty(caller)
        .map_err(|error| StorageError::Other(format!("a caller cannot be encoded: {error}")))?;
    store
        .upload_text(&caller_path(&caller.deployment, &caller.caller), &body)
        .await
}

/// Every caller declared for `deployment`, revoked ones included.
pub async fn list(store: &JobStorage, deployment: &str) -> Result<Vec<Caller>, StorageError> {
    let prefix = format!("{CALLERS_PREFIX}{deployment}/");
    let mut callers = Vec::new();
    for key in store.list_paths(&prefix, Listing::Every).await? {
        let Some(text) = store.download_text(&key).await? else {
            continue;
        };
        callers.push(serde_json::from_str(&text).map_err(|error| {
            StorageError::Other(format!("{key} is not a caller declaration: {error}"))
        })?);
    }
    callers.sort_by(|left: &Caller, right| left.caller.cmp(&right.caller));
    Ok(callers)
}

/// Every kept hour of `caller` (every caller when `None`) of `deployment`
/// at or after `since`.
pub async fn usage(
    store: &JobStorage,
    deployment: &str,
    caller: Option<&str>,
    since: &str,
) -> Result<Vec<HourUsage>, StorageError> {
    let prefix = match caller {
        Some(caller) => format!("{USAGE_PREFIX}{deployment}/{caller}/"),
        None => format!("{USAGE_PREFIX}{deployment}/"),
    };
    let mut hours = Vec::new();
    for key in store.list_paths(&prefix, Listing::Every).await? {
        let Some(text) = store.download_text(&key).await? else {
            continue;
        };
        let hour: HourUsage = serde_json::from_str(&text).map_err(|error| {
            StorageError::Other(format!("{key} is not an hour of usage: {error}"))
        })?;
        if hour.hour.as_str() >= since {
            hours.push(hour);
        }
    }
    hours.sort_by(|left, right| (&left.caller, &left.hour).cmp(&(&right.caller, &right.hour)));
    Ok(hours)
}

pub async fn save_usage(store: &JobStorage, hour: &HourUsage) -> Result<(), StorageError> {
    let body = serde_json::to_string(hour)
        .map_err(|error| StorageError::Other(format!("usage cannot be encoded: {error}")))?;
    store
        .upload_text(
            &usage_path(&hour.deployment, &hour.caller, &hour.hour),
            &body,
        )
        .await
}
