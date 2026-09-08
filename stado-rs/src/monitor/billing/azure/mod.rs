//! The Azure collector: the Skarbiec service-principal read that authorizes
//! it, and — in [`balance`] — the OAuth exchange plus the ARM credit,
//! grant and billing-property reads it performs with that principal.

mod balance;

use serde_json::{json, Value};

use crate::config;
use crate::queue::JobStorage;

use balance::azure_section_with;

const AZURE_LOGIN_BASE: &str = "https://login.microsoftonline.com";
/// Python `_ARM`.
const ARM_BASE: &str = "https://management.azure.com";

// ---------------------------------------------------------------------------
// Azure section — Skarbiec SP + ARM available balance
// ---------------------------------------------------------------------------

/// Available credit balance via ARM. The service-principal object is read from
/// the separate Skarbiec repository/service and nowhere else. A
/// vault/auth/request failure is terminal for this source: silently falling
/// through would bypass the credential policy.
pub(super) async fn azure_section(_store: &JobStorage) -> Value {
    let secret_name = config::azure_billing_secret();
    let vault = match crate::skarbiec::Client::configured() {
        Ok(vault) => vault,
        Err(err) => return azure_error("skarbiec_error", err.to_string()),
    };
    // Field by field: a broker that requires a named field refuses the
    // whole-item form outright, which turned a configured Azure credential
    // into a "skarbiec_error" row on every billing tick.
    let mut sp = serde_json::Map::new();
    for field in [
        "tenant_id",
        "client_id",
        "client_secret",
        "billing_account",
        "billing_profile",
        "billing_profile_system_id",
        "subscription_id",
    ] {
        match vault.read_string(secret_name, field).await {
            Ok(Some(value)) => {
                sp.insert(field.to_string(), Value::from(value));
            }
            Ok(None) => {}
            Err(err) => return azure_error("skarbiec_error", err.to_string()),
        }
    }
    if sp.is_empty() {
        return json!({
            "status": "no_credentials",
            "detail": format!("Skarbiec item {secret_name:?} does not exist"),
        });
    }
    let sp = Value::Object(sp);
    let client = reqwest::Client::new();
    azure_section_with(&client, &sp, AZURE_LOGIN_BASE, ARM_BASE).await
}

fn azure_error(status: &str, detail: String) -> Value {
    json!({"status": status, "detail": detail})
}
