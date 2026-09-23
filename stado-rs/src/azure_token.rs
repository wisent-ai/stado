//! Shared Azure bearer-token chain for identity-based authentication.
//!
//! Factored out of `providers/azure/mod.rs` (ARM) so the Azure Blob queue
//! backend (`queue/azure_blob.rs`) reuses the exact same acquisition logic
//! with a different scope/resource pair:
//!
//! - ARM:     scope `https://management.azure.com/.default`,
//!   resource `https://management.azure.com`
//! - Blob:    scope `https://storage.azure.com/.default`,
//!   resource `https://storage.azure.com`
//!
//! The sole credential source is Azure IMDS managed identity. Static service
//! principals, process-environment secrets, local credential files, and Azure
//! CLI sessions are not credential sources.
//! Tokens are cached per scope with their expiry and refreshed early.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

use serde_json::Value;

/// Azure token acquisition / transport failure. Consumers map this onto
/// their own error type (`providers::azure::AzureError` for ARM,
/// `queue::StorageError` for the blob backend).
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    /// Token acquisition failed (or every chain source unavailable).
    #[error("no Azure credentials: {0}")]
    Auth(String),
    /// Transport failure.
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

/// Refresh the cached token this many seconds before its expiry.
const TOKEN_REFRESH_SKEW_S: i64 = 300;

/// IMDS API version. The Instance Metadata Service versions the whole
/// service rather than the endpoint, so the same pin covers the
/// managed-identity token request here and the instance-metadata probe
/// in [`crate::providers::local::azure_self`].
pub(crate) const IMDS_API_VERSION: &str = "2018-02-01";

/// A freshly acquired token: value + seconds until expiry.
struct TokenGrant {
    access_token: String,
    expires_in: i64,
}

/// Cached token. Fresh until [`TOKEN_REFRESH_SKEW_S`] before expiry.
#[derive(Clone)]
struct CachedToken {
    access_token: String,
    expires_at_unix: i64,
}

impl CachedToken {
    /// Split out for tests (injected clock).
    fn fresh_at(&self, now_unix: i64) -> bool {
        self.expires_at_unix - now_unix > TOKEN_REFRESH_SKEW_S
    }
}

/// Per-scope cache: ARM and storage tokens are not interchangeable.
static TOKEN_CACHE: LazyLock<RwLock<HashMap<String, CachedToken>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

fn cached_token(scope: &str, now_unix: i64) -> Option<String> {
    TOKEN_CACHE
        .read()
        .expect("token cache lock")
        .get(scope)
        .filter(|token| token.fresh_at(now_unix))
        .map(|token| token.access_token.clone())
}

fn cache_token(scope: &str, grant: &TokenGrant, now_unix: i64) {
    TOKEN_CACHE.write().expect("token cache lock").insert(
        scope.to_string(),
        CachedToken {
            access_token: grant.access_token.clone(),
            expires_at_unix: now_unix + grant.expires_in,
        },
    );
}

/// Number-or-string JSON field as i64 (IMDS returns `expires_in` as a
/// string; Azure CLI expiry is represented separately).
fn json_i64(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// IMDS managed identity. Short timeout: off-Azure this endpoint hangs, then
/// the chain falls through to Skarbiec.
async fn imds_token(http: &reqwest::Client, resource: &str) -> Result<TokenGrant, TokenError> {
    let response = http
        .get("http://169.254.169.254/metadata/identity/oauth2/token")
        .header("Metadata", "true")
        .query(&[("api-version", IMDS_API_VERSION), ("resource", resource)])
        .timeout(Duration::from_secs(2))
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        return Err(TokenError::Auth(format!(
            "IMDS GET -> HTTP {status}: {}",
            text.chars().take(280).collect::<String>()
        )));
    }
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let access_token = body
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if access_token.is_empty() {
        return Err(TokenError::Auth("IMDS response has no access_token".into()));
    }
    Ok(TokenGrant {
        access_token,
        expires_in: json_i64(body.get("expires_in")).unwrap_or(3600),
    })
}

/// Seconds until an `expiresOn` value from `az account get-access-token`
/// ("2026-07-25 21:02:38.000000", local time — the same interpretation
/// azure-identity's AzureCliCredential uses). Split out pure for tests.
#[cfg(test)]
fn az_cli_expires_in(expires_on: &str, now_unix: i64) -> Option<i64> {
    let naive = chrono::NaiveDateTime::parse_from_str(expires_on, "%Y-%m-%d %H:%M:%S%.f").ok()?;
    let local = naive.and_local_timezone(chrono::Local).single()?;
    Some(local.timestamp() - now_unix)
}

/// Azure access is available only through the adapter host's managed identity.
/// Static service-principal keys, Azure CLI sessions, and workload-agent grants
/// are deliberately unsupported provider credential sources.
async fn fetch_token(
    http: &reqwest::Client,
    _scope: &str,
    resource: &str,
) -> Result<TokenGrant, TokenError> {
    imds_token(http, resource).await.map_err(|error| {
        TokenError::Auth(format!(
            "Azure adapter managed identity is unavailable: {error}"
        ))
    })
}

/// Fresh bearer token for `scope` (OAuth) / `resource` (IMDS naming for the
/// same audience), from cache or the managed identity.
pub(crate) async fn bearer_token(
    http: &reqwest::Client,
    scope: &str,
    resource: &str,
) -> Result<String, TokenError> {
    let now = chrono::Utc::now().timestamp();
    if let Some(token) = cached_token(scope, now) {
        return Ok(token);
    }
    let grant = fetch_token(http, scope, resource).await?;
    cache_token(scope, &grant, now);
    Ok(grant.access_token)
}

/// Explicit identity-only entry point used by Key Vault.
pub(crate) async fn identity_bearer_token(
    http: &reqwest::Client,
    scope: &str,
    resource: &str,
) -> Result<String, TokenError> {
    bearer_token(http, scope, resource).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_cache_freshness_with_injected_clock() {
        let token = CachedToken {
            access_token: "t".into(),
            expires_at_unix: 1_000_000,
        };
        // More than 5 min left: fresh.
        assert!(token.fresh_at(1_000_000 - TOKEN_REFRESH_SKEW_S - 1));
        // Inside the 5 min skew window: stale (refresh early).
        assert!(!token.fresh_at(1_000_000 - TOKEN_REFRESH_SKEW_S));
        assert!(!token.fresh_at(1_000_000 - 1));
        assert!(!token.fresh_at(1_000_000));
    }

    #[test]
    fn az_cli_expires_on_parses_local_time() {
        let now = chrono::Utc::now().timestamp();
        let future = chrono::DateTime::from_timestamp(now + 3600, 0)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S%.6f")
            .to_string();
        let expires_in = az_cli_expires_in(&future, now).unwrap();
        assert!((expires_in - 3600).abs() <= 1, "{expires_in}");
        assert_eq!(az_cli_expires_in("not a date", now), None);
    }

    #[test]
    fn cache_is_keyed_by_scope() {
        let now = chrono::Utc::now().timestamp();
        let grant_a = TokenGrant {
            access_token: "arm-token".into(),
            expires_in: 3600,
        };
        let grant_b = TokenGrant {
            access_token: "storage-token".into(),
            expires_in: 3600,
        };
        cache_token("scope-a", &grant_a, now);
        cache_token("scope-b", &grant_b, now);
        assert_eq!(cached_token("scope-a", now).as_deref(), Some("arm-token"));
        assert_eq!(
            cached_token("scope-b", now).as_deref(),
            Some("storage-token")
        );
        assert_eq!(cached_token("scope-c", now), None);
    }
}
