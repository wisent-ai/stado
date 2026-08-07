//! Operator-session authorization metadata.
//!
//! The dashboard validates operator sessions against the Supabase project
//! recorded in the `stado-supabase` Skarbiec item. That item is reachable
//! only through the dedicated business-messaging grant, so the grant contract
//! is enforced here exactly as the deployment configuration declares it. The
//! application-plane consumers of the same grant (APNs, FCM, the device
//! registry, and email) now live in the private `wisent-backend` service; only
//! this operator lookup remains in Stado.

use std::collections::BTreeSet;

use serde_json::Value;
use url::Url;

const MESSAGING_CONSUMER: &str = "wisent-backend-business-messaging";
const EMAIL_ITEM: &str = "wisent-backend-email-provider";
const APNS_ITEM: &str = "wisent-backend-apns";
const FCM_ITEM: &str = "wisent-backend-fcm";
const DEVICE_REGISTRY_ITEM: &str = "stado-supabase";

const REQUIRED_ITEMS: &[&str] = &[APNS_ITEM, FCM_ITEM, DEVICE_REGISTRY_ITEM];

#[derive(Debug, thiserror::Error)]
pub(super) enum OperatorAuthError {
    #[error("backend messaging credential boundary is not configured")]
    Configuration,
    #[error("backend messaging credential contract is unavailable")]
    Credential,
}

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
    let token_file = crate::config::backend_messaging_skarbiec_token_file();
    let distinct_grant = !token_file.is_empty()
        && token_file != crate::config::skarbiec_token_file()
        && token_file != crate::config::agent_skarbiec_token_file()
        && token_file != crate::config::object_skarbiec_token_file()
        && token_file != crate::config::release_skarbiec_token_file()
        && token_file != crate::config::service_skarbiec_token_file();
    if crate::config::backend_messaging_skarbiec_consumer() != MESSAGING_CONSUMER
        || !items_valid
        || !distinct_grant
    {
        return Err(OperatorAuthError::Configuration);
    }
    let url = crate::config::backend_messaging_skarbiec_url();
    if !url.starts_with("https://") && !url.starts_with("http://127.0.0.1:") {
        return Err(OperatorAuthError::Configuration);
    }
    crate::skarbiec::Client::new(url, MESSAGING_CONSUMER, token_file)
        .map_err(|_| OperatorAuthError::Configuration)
}

fn required(value: &Value) -> Result<&str, OperatorAuthError> {
    value
        .as_str()
        .filter(|value| !value.is_empty() && value.trim() == *value)
        .ok_or(OperatorAuthError::Credential)
}

fn supabase_base(raw: &str) -> Result<Url, OperatorAuthError> {
    let base = Url::parse(raw).map_err(|_| OperatorAuthError::Credential)?;
    let host = base.host_str().unwrap_or_default();
    if base.scheme() != "https"
        || !host.ends_with(".supabase.co")
        || !base.username().is_empty()
        || base.password().is_some()
        || base.port().is_some()
        || (base.path() != "/" && !base.path().is_empty())
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err(OperatorAuthError::Credential);
    }
    Ok(base)
}

pub(super) async fn operator_auth_metadata() -> Result<(Url, String), OperatorAuthError> {
    // Field by field: the broker refuses a read that names none, and this
    // caller has always wanted exactly these two. A whole-item read here left
    // the dashboard unable to authenticate an operator at all.
    let vault = messaging_vault()?;
    let raw_url = vault
        .read_field(DEVICE_REGISTRY_ITEM, "url")
        .await
        .map_err(|_| OperatorAuthError::Credential)?;
    let raw_key = vault
        .read_field(DEVICE_REGISTRY_ITEM, "anon_key")
        .await
        .map_err(|_| OperatorAuthError::Credential)?;
    let url = supabase_base(required(&raw_url)?)?;
    let anon_key = required(&raw_key)?.to_string();
    Ok((url, anon_key))
}
