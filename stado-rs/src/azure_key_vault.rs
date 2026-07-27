//! Azure Key Vault access for application credentials.
//!
//! Vault authentication deliberately accepts only non-secret operator/workload
//! identities: Azure Managed Identity (IMDS) or the current Azure CLI session.
//! An application credential must never be bootstrapped from an environment
//! variable, local file, queue blob, or another cloud's secret manager.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};

const SCOPE: &str = "https://vault.azure.net/.default";
const RESOURCE: &str = "https://vault.azure.net";
const API_VERSION: &str = "7.5";

#[derive(Debug, thiserror::Error)]
pub enum KeyVaultError {
    #[error("Azure Key Vault URL is not configured; set WC_AZURE_KEY_VAULT_URL")]
    NotConfigured,
    #[error("invalid Azure Key Vault URL {0:?}; expected https://<name>.vault.azure.net")]
    InvalidVaultUrl(String),
    #[error("invalid Azure Key Vault secret name {0:?}")]
    InvalidSecretName(String),
    #[error("cannot authenticate to Azure Key Vault: {0}")]
    Auth(#[from] crate::azure_token::TokenError),
    #[error("Azure Key Vault request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Azure Key Vault returned HTTP {status}: {detail}")]
    Response { status: u16, detail: String },
    #[error("Azure Key Vault secret {0:?} has no string value")]
    MissingValue(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct SecretInfo {
    pub name: String,
    pub updated: Option<DateTime<Utc>>,
    pub enabled: Option<bool>,
}

fn checked_vault_url(vault_url: &str) -> Result<&str, KeyVaultError> {
    let vault_url = vault_url.trim().trim_end_matches('/');
    if vault_url.is_empty() {
        return Err(KeyVaultError::NotConfigured);
    }
    let parsed = url::Url::parse(vault_url)
        .map_err(|_| KeyVaultError::InvalidVaultUrl(vault_url.to_string()))?;
    let host = parsed.host_str().unwrap_or_default();
    if parsed.scheme() != "https"
        || !host.ends_with(".vault.azure.net")
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || (parsed.path() != "/" && !parsed.path().is_empty())
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(KeyVaultError::InvalidVaultUrl(vault_url.to_string()));
    }
    Ok(vault_url)
}

fn validate_name(name: &str) -> Result<(), KeyVaultError> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(KeyVaultError::InvalidSecretName(name.to_string()));
    }
    Ok(())
}

async fn response_error(response: reqwest::Response) -> KeyVaultError {
    let status = response.status().as_u16();
    let detail = response
        .text()
        .await
        .unwrap_or_default()
        .chars()
        .take(usize::from(u16::MAX))
        .collect();
    KeyVaultError::Response { status, detail }
}

/// Read the current version of one secret. A missing secret is `None`; access
/// and transport failures are errors so callers cannot silently fall through
/// to an unapproved credential source.
pub async fn read_secret(
    client: &reqwest::Client,
    vault_url: &str,
    name: &str,
) -> Result<Option<String>, KeyVaultError> {
    let vault_url = checked_vault_url(vault_url)?;
    validate_name(name)?;

    let token = crate::azure_token::identity_bearer_token(client, SCOPE, RESOURCE).await?;
    let url = format!("{vault_url}/secrets/{name}?api-version={API_VERSION}");
    let response = client.get(url).bearer_auth(token).send().await?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(response_error(response).await);
    }

    let body: Value = response.json().await?;
    body.get("value")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .map(Some)
        .ok_or_else(|| KeyVaultError::MissingValue(name.to_string()))
}

/// Create a new current version of one Key Vault secret.
pub async fn write_secret(
    client: &reqwest::Client,
    vault_url: &str,
    name: &str,
    value: &str,
) -> Result<(), KeyVaultError> {
    let vault_url = checked_vault_url(vault_url)?;
    validate_name(name)?;
    let token = crate::azure_token::identity_bearer_token(client, SCOPE, RESOURCE).await?;
    let url = format!("{vault_url}/secrets/{name}?api-version={API_VERSION}");
    let response = client
        .put(url)
        .bearer_auth(token)
        .json(&json!({"value": value}))
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    Ok(())
}

/// List secret metadata without downloading any secret value.
pub async fn list_secrets(
    client: &reqwest::Client,
    vault_url: &str,
) -> Result<Vec<SecretInfo>, KeyVaultError> {
    let vault_url = checked_vault_url(vault_url)?;
    let token = crate::azure_token::identity_bearer_token(client, SCOPE, RESOURCE).await?;
    let mut next = Some(format!(
        "{vault_url}/secrets?api-version={API_VERSION}"
    ));
    let mut secrets = Vec::new();
    while let Some(url) = next {
        let response = client.get(url).bearer_auth(&token).send().await?;
        if !response.status().is_success() {
            return Err(response_error(response).await);
        }
        let body: Value = response.json().await?;
        for item in body
            .get("value")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = item
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| id.split_once("/secrets/").map(|(_, tail)| tail))
                .and_then(|tail| tail.split('/').next())
            else {
                continue;
            };
            let updated = item
                .pointer("/attributes/updated")
                .and_then(Value::as_i64)
                .and_then(|timestamp| DateTime::from_timestamp(timestamp, u32::MIN));
            let enabled = item.pointer("/attributes/enabled").and_then(Value::as_bool);
            secrets.push(SecretInfo {
                name: name.to_string(),
                updated,
                enabled,
            });
        }
        next = body
            .get("nextLink")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
            .map(str::to_owned);
    }
    secrets.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(secrets)
}

/// Soft-delete one secret. The flag reports whether a current secret existed.
pub async fn delete_secret(
    client: &reqwest::Client,
    vault_url: &str,
    name: &str,
) -> Result<bool, KeyVaultError> {
    let vault_url = checked_vault_url(vault_url)?;
    validate_name(name)?;
    let token = crate::azure_token::identity_bearer_token(client, SCOPE, RESOURCE).await?;
    let url = format!("{vault_url}/secrets/{name}?api-version={API_VERSION}");
    let response = client.delete(url).bearer_auth(token).send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    Ok(true)
}
