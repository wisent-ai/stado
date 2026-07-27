//! Shared Azure bearer-token chain (DefaultAzureCredential equivalent).
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
//! Sources, in order (the practical DefaultAzureCredential chain):
//! (a) env service principal (AZURE_CLIENT_ID / AZURE_CLIENT_SECRET /
//! AZURE_TENANT_ID) client-credentials POST, (b) IMDS managed identity,
//! (c) `az account get-access-token`. Tokens are cached per scope with
//! their expiry and refreshed 5 min early.

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
/// string, the client-credentials endpoint as a number).
fn json_i64(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// (a) Env service principal -> client-credentials POST (Python
/// EnvironmentCredential).
async fn client_credentials_token(
    http: &reqwest::Client,
    tenant: &str,
    client_id: &str,
    client_secret: &str,
    scope: &str,
) -> Result<TokenGrant, TokenError> {
    let url = format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token");
    let response = http
        .post(url)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("scope", scope),
            ("grant_type", "client_credentials"),
        ])
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        return Err(TokenError::Auth(format!(
            "client-credentials POST -> HTTP {status}: {}",
            text.chars().take(280).collect::<String>()
        )));
    }
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let access_token =
        body.get("access_token").and_then(Value::as_str).unwrap_or_default().to_string();
    if access_token.is_empty() {
        return Err(TokenError::Auth("client-credentials response has no access_token".into()));
    }
    Ok(TokenGrant {
        access_token,
        expires_in: json_i64(body.get("expires_in")).unwrap_or(3600),
    })
}

/// (b) IMDS managed identity (Python ManagedIdentityCredential). Short
/// timeout: off-Azure this endpoint hangs, and the chain must fall
/// through to the CLI.
async fn imds_token(http: &reqwest::Client, resource: &str) -> Result<TokenGrant, TokenError> {
    let response = http
        .get("http://169.254.169.254/metadata/identity/oauth2/token")
        .header("Metadata", "true")
        .query(&[("api-version", "2018-02-01"), ("resource", resource)])
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
    let access_token =
        body.get("access_token").and_then(Value::as_str).unwrap_or_default().to_string();
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
fn az_cli_expires_in(expires_on: &str, now_unix: i64) -> Option<i64> {
    let naive = chrono::NaiveDateTime::parse_from_str(expires_on, "%Y-%m-%d %H:%M:%S%.f").ok()?;
    let local = naive.and_local_timezone(chrono::Local).single()?;
    Some(local.timestamp() - now_unix)
}

/// (c) Azure CLI (Python AzureCliCredential).
async fn cli_token(resource: &str) -> Result<TokenGrant, TokenError> {
    let output = tokio::process::Command::new("az")
        .args(["account", "get-access-token", "--resource", resource, "--output", "json"])
        .output()
        .await
        .map_err(|err| TokenError::Auth(format!("az CLI not runnable: {err}")))?;
    if !output.status.success() {
        return Err(TokenError::Auth(format!(
            "az account get-access-token -> {}",
            String::from_utf8_lossy(&output.stderr).chars().take(280).collect::<String>()
        )));
    }
    let body: Value = serde_json::from_slice(&output.stdout)
        .map_err(|err| TokenError::Auth(format!("az CLI output is not JSON: {err}")))?;
    let access_token =
        body.get("accessToken").and_then(Value::as_str).unwrap_or_default().to_string();
    if access_token.is_empty() {
        return Err(TokenError::Auth("az CLI output has no accessToken".into()));
    }
    let now = chrono::Utc::now().timestamp();
    let expires_in = body
        .get("expiresOn")
        .and_then(Value::as_str)
        .and_then(|expires_on| az_cli_expires_in(expires_on, now))
        .unwrap_or(3600);
    Ok(TokenGrant { access_token, expires_in })
}

/// DefaultAzureCredential's practical sources, in order.
async fn fetch_token(http: &reqwest::Client, scope: &str, resource: &str) -> Result<TokenGrant, TokenError> {
    // (a) env service principal. Complete env config that fails the token
    // request is a hard error (DefaultAzureCredential propagates a failed
    // EnvironmentCredential rather than falling through).
    let client_id = std::env::var("AZURE_CLIENT_ID").unwrap_or_default();
    let client_secret = std::env::var("AZURE_CLIENT_SECRET").unwrap_or_default();
    let tenant_id = std::env::var("AZURE_TENANT_ID").unwrap_or_default();
    if !client_id.is_empty() && !client_secret.is_empty() && !tenant_id.is_empty() {
        return client_credentials_token(http, &tenant_id, &client_id, &client_secret, scope).await;
    }
    // (b) IMDS managed identity; unreachable off-Azure, so any failure
    // falls through to the CLI.
    if let Ok(grant) = imds_token(http, resource).await {
        return Ok(grant);
    }
    // (c) Azure CLI.
    cli_token(resource).await
}

/// Fresh bearer token for `scope` (OAuth) / `resource` (IMDS + az CLI
/// naming for the same audience), from cache or the chain.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_cache_freshness_with_injected_clock() {
        let token = CachedToken { access_token: "t".into(), expires_at_unix: 1_000_000 };
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
        let grant_a = TokenGrant { access_token: "arm-token".into(), expires_in: 3600 };
        let grant_b = TokenGrant { access_token: "storage-token".into(), expires_in: 3600 };
        cache_token("scope-a", &grant_a, now);
        cache_token("scope-b", &grant_b, now);
        assert_eq!(cached_token("scope-a", now).as_deref(), Some("arm-token"));
        assert_eq!(cached_token("scope-b", now).as_deref(), Some("storage-token"));
        assert_eq!(cached_token("scope-c", now), None);
    }
}
