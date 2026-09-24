//! Authentication for the native Desktop command API, retained independently
//! of the retired HTML dashboard.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use serde_json::{json, Value};
use url::Url;

use super::{trusted_request_host, Request};

const EMAIL_ITEM: &str = "wisent-backend-email-provider";
const APNS_ITEM: &str = "wisent-backend-apns";
const FCM_ITEM: &str = "wisent-backend-fcm";
const DEVICE_REGISTRY_ITEM: &str = "stado-supabase";
const REQUIRED_ITEMS: &[&str] = &[APNS_ITEM, FCM_ITEM, DEVICE_REGISTRY_ITEM];
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

#[derive(Debug, thiserror::Error)]
pub(super) enum OperatorAuthError {
    #[error("backend messaging credential boundary is not configured")]
    Configuration,
    #[error("backend messaging credential contract is unavailable")]
    Credential,
    #[error("operator permission request failed: {0}")]
    Request(reqwest::Error),
    #[error("operator permission request returned HTTP {0}")]
    Response(reqwest::StatusCode),
}

/// Backend messaging items are read through Stado's Skarbiec identity.
fn messaging_vault() -> Result<crate::skarbiec::Client, OperatorAuthError> {
    let configured = crate::config::backend_messaging_skarbiec_items();
    let required = REQUIRED_ITEMS.iter().copied().collect::<BTreeSet<_>>();
    let actual = configured
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let items_valid = actual.len() == configured.len()
        && required.is_subset(&actual)
        && actual
            .iter()
            .all(|item| required.contains(item) || *item == EMAIL_ITEM);
    if !items_valid {
        return Err(OperatorAuthError::Configuration);
    }
    crate::skarbiec::Client::stado().map_err(|_| OperatorAuthError::Configuration)
}

fn required(value: &Value) -> Result<&str, OperatorAuthError> {
    value
        .as_str()
        .filter(|value| !value.is_empty() && value.trim() == *value)
        .ok_or(OperatorAuthError::Credential)
}

async fn metadata() -> Result<(Url, String), OperatorAuthError> {
    let vault = messaging_vault()?;
    let raw_url = vault
        .read_field(DEVICE_REGISTRY_ITEM, "url")
        .await
        .map_err(|_| OperatorAuthError::Credential)?;
    let raw_key = vault
        .read_field(DEVICE_REGISTRY_ITEM, "anon_key")
        .await
        .map_err(|_| OperatorAuthError::Credential)?;
    let base = Url::parse(required(&raw_url)?).map_err(|_| OperatorAuthError::Credential)?;
    if base.scheme() != "https"
        || !base
            .host_str()
            .unwrap_or_default()
            .ends_with(".supabase.co")
        || !base.username().is_empty()
        || base.password().is_some()
        || base.port().is_some()
        || (base.path() != "/" && !base.path().is_empty())
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err(OperatorAuthError::Credential);
    }
    Ok((base, required(&raw_key)?.to_string()))
}

pub(super) async fn authorized(request: &Request) -> Result<bool, OperatorAuthError> {
    // Preserve the native local contract: the loopback listener's Host guard
    // rejects DNS rebinding, and a forwarded request never receives local trust.
    if request.header("x-forwarded-proto").is_none()
        && trusted_request_host(request.header("host"), None, false)
    {
        return Ok(true);
    }
    let deployment_id = crate::config::stado_deployment_id();
    let authorization = request.header("authorization").unwrap_or("").trim();
    if deployment_id.is_empty() || !authorization.starts_with("Bearer ") {
        return Ok(false);
    }
    let (base, anon_key) = metadata().await?;
    let endpoint = base
        .join("/rest/v1/rpc/stado_can_access")
        .map_err(|_| OperatorAuthError::Credential)?;
    let response = HTTP
        .post(endpoint)
        .header("apikey", anon_key)
        .header("Authorization", authorization)
        .json(&json!({
            "target_deployment_id": deployment_id,
            "requested_permission": "operate",
        }))
        .send()
        .await
        .map_err(OperatorAuthError::Request)?;
    if matches!(
        response.status(),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ) {
        return Ok(false);
    }
    if response.status() != reqwest::StatusCode::OK {
        return Err(OperatorAuthError::Response(response.status()));
    }
    response
        .json::<Value>()
        .await
        .map(|value| value == Value::Bool(true))
        .map_err(OperatorAuthError::Request)
}
