//! The credit read itself: the OAuth client-credentials exchange, the ARM
//! balance endpoint the configured scope selects, and the subscription
//! billing-property lookup that turns a balance into a grant figure.

mod amount;

use serde_json::{json, Value};

use self::amount::extract_amount;
use super::azure_error;
use crate::config;
use crate::monitor::billing::format::{error_section, py_list_repr};

/// Injectable twin of the post-secret half of [`azure_section`](super::azure_section): SP JSON +
/// login/ARM base URLs explicit, so tests can run the OAuth + ARM exchange
/// against the loopback mock.
pub(super) async fn azure_section_with(
    client: &reqwest::Client,
    sp: &Value,
    login_base: &str,
    arm_base: &str,
) -> Value {
    let field = |key: &str| {
        sp.get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let (tenant_id, client_id, client_secret) = match (
        field("tenant_id"),
        field("client_id"),
        field("client_secret"),
    ) {
        (Some(tenant), Some(client), Some(secret)) => (tenant, client, secret),
        (tenant, client, secret) => {
            let missing: Vec<&str> = [
                ("tenant_id", tenant),
                ("client_id", client),
                ("client_secret", secret),
            ]
            .into_iter()
            .filter(|(_key, value)| value.is_none())
            .map(|(key, _value)| key)
            .collect();
            return azure_error(
                "config_error",
                format!("Azure SP secret missing keys: {}", py_list_repr(&missing)),
            );
        }
    };

    // Python ClientSecretCredential.get_token("https://management.azure.com/.default").
    let token_url = format!("{login_base}/{tenant_id}/oauth2/v2.0/token");
    let token = match crate::wait::request(client.post(&token_url).form(&[
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("scope", "https://management.azure.com/.default"),
        ("grant_type", "client_credentials"),
    ]))
    .await
    {
        Err(err) => return azure_error("auth_error", err.to_string()),
        Ok(response) => {
            let status = response.status();
            if !status.is_success() {
                let body = crate::primitives::http::refused_body(response).await;
                return azure_error("auth_error", format!("HTTP {status}: {body}"));
            }
            match response.json::<Value>().await {
                Ok(body) => match body.get("access_token").and_then(Value::as_str) {
                    Some(token) => token.to_string(),
                    None => {
                        return azure_error(
                            "auth_error",
                            "token response missing access_token".to_string(),
                        )
                    }
                },
                Err(err) => return azure_error("auth_error", err.to_string()),
            }
        }
    };

    let billing_account = sp
        .get("billing_account")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let billing_profile = sp
        .get("billing_profile")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let billing_profile_system_id = sp
        .get("billing_profile_system_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let subscription = sp
        .get("subscription_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());

    let modern_scope = billing_account.zip(billing_profile_system_id);
    let url = if let Some((account, profile_system_id)) = modern_scope {
        format!(
            "{arm_base}/providers/Microsoft.Billing/billingAccounts/{account}/billingProfiles/{profile_system_id}/providers/Microsoft.Consumption/credits/balanceSummary?api-version=2023-05-01"
        )
    } else if let (Some(account), Some(profile)) = (billing_account, billing_profile) {
        // Backward-compatible legacy path. New configuration should include
        // billing_profile_system_id and use the MCA credits API above.
        format!(
            "{arm_base}/providers/Microsoft.Billing/billingAccounts/{account}/billingProfiles/{profile}/availableBalance?api-version=2023-05-01"
        )
    } else if let Some(subscription) = subscription {
        format!(
            "{arm_base}/subscriptions/{subscription}/providers/Microsoft.Consumption/balances?api-version=2019-10-01"
        )
    } else {
        return azure_error(
            "config_error",
            "Azure SP secret needs billing_account+billing_profile_system_id, \
             billing_account+billing_profile, or subscription_id"
                .to_string(),
        );
    };

    let response = match crate::wait::request(client.get(&url).bearer_auth(&token)).await {
        Ok(response) => response,
        Err(err) => {
            return json!({"status": "arm_error", "detail": err.to_string(), "endpoint": url})
        }
    };
    let status = response.status();
    if !status.is_success() {
        let body_text = crate::primitives::http::refused_body(response).await;
        let truncated: String = body_text.chars().take(usize::from(u16::MAX)).collect();
        return json!({
            "status": "arm_error",
            "detail": format!("HTTP {}: {truncated}", status.as_u16()),
            "endpoint": url,
        });
    }
    let body_text = match response.text().await {
        Ok(text) => text,
        Err(err) => {
            return json!({
                "status": "arm_error",
                "detail": format!(
                    "HTTP {} but the balance body could not be read: {}",
                    status.as_u16(),
                    err.without_url()
                ),
                "endpoint": url,
            })
        }
    };
    let balance_reported = status != reqwest::StatusCode::NO_CONTENT;
    let body: Value = if balance_reported {
        match serde_json::from_str(&body_text) {
            Ok(body) => body,
            Err(err) => return error_section(err.to_string()),
        }
    } else {
        json!({})
    };
    // A balance document carries its fields under `properties`; one without
    // that member is read as the fields themselves.
    let props = match body.get("properties") {
        Some(properties) => properties.clone(),
        None => body.clone(),
    };
    let amount = extract_amount(&props);
    // The estimated balance, or the current one when Azure estimates none.
    let estimated = match props.pointer("/balanceSummary/estimatedBalance/value") {
        Some(value) => value.clone(),
        None => amount.clone(),
    };
    // The currency Azure names for the balance, in the first of the places
    // its APIs name it; null when it names none, never a currency assumed.
    let currency = match props
        .get("creditCurrency")
        .or_else(|| props.get("billingCurrency"))
        .or_else(|| props.pointer("/balanceSummary/currentBalance/currency"))
        .or_else(|| props.pointer("/amount/currency"))
    {
        Some(currency) => currency.clone(),
        None => Value::Null,
    };
    let expired = match props.pointer("/expiredCredit/value") {
        Some(value) => value.clone(),
        None => Value::Null,
    };
    let pending = match props.pointer("/pendingEligibleCharges/value") {
        Some(value) => value.clone(),
        None => Value::Null,
    };

    let mut billing_property = Value::Null;
    let mut scope_detail = Value::Null;
    if modern_scope.is_some() {
        if let Some(subscription) = subscription {
            let property_url = format!(
                "{arm_base}/subscriptions/{subscription}/providers/Microsoft.Billing/billingProperty/default?api-version=2024-04-01"
            );
            match crate::wait::request(client.get(&property_url).bearer_auth(&token)).await {
                Err(err) => scope_detail = json!(err.to_string()),
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        billing_property = match response.text().await {
                            Ok(text) => match serde_json::from_str(&text) {
                                Ok(property) => property,
                                Err(err) => json!({"parse_error": err.to_string()}),
                            },
                            Err(err) => json!({"read_error": err.without_url().to_string()}),
                        };
                    } else {
                        let text = crate::primitives::http::refused_body(response).await;
                        scope_detail = json!(format!("HTTP {}: {text}", status.as_u16()));
                    }
                }
            }
        }
    }
    // A billing property document carries its fields under `properties`;
    // one without that member is read as the fields themselves.
    let property_props = match billing_property.get("properties") {
        Some(properties) => properties,
        None => &billing_property,
    };
    let grant = property_props
        .get("billingProfileSpendingLimitDetails")
        .and_then(Value::as_array)
        .and_then(|details| {
            details
                .iter()
                .find(|detail| {
                    detail.get("type").and_then(Value::as_str) == Some("StartupSponsorship")
                })
                .or_else(|| details.first())
        });
    // `Value` indexing writes an absent member as null.
    let grant_amount = match grant {
        Some(detail) => detail["amount"].clone(),
        None => Value::Null,
    };
    // A grant whose balance Azure does not report is read as the grant less
    // its actual cost since it began ([`super::spent`]), both sources named;
    // one that cannot be read leaves the balance unknown, with why.
    let (amount, currency, balance_source) = if balance_reported {
        (amount, currency, json!("credits balance"))
    } else {
        match super::spent::balance(client, arm_base, &token, grant, subscription).await {
            Ok((available, stated, source)) => (json!(available), json!(stated), source),
            Err(cause) => (Value::Null, currency, json!({ "unread": cause })),
        }
    };
    let credit_used = match (grant_amount.as_f64(), amount.as_f64()) {
        (Some(grant), Some(balance)) => json!(grant - balance),
        _ => Value::Null,
    };

    json!({
        "status": "ok",
        "balance_reported": balance_reported,
        "detail": if balance_reported {
            Value::Null
        } else {
            json!("Azure returned no credit balance; the available balance is the grant less its actual cost since it began (balance_source)")
        },
        "available_balance": amount,
        "estimated_balance": estimated,
        "currency": currency,
        "balance_source": balance_source,
        "expired_credit": expired,
        "pending_eligible_charges": pending,
        // A field Azure did not answer is null, as `json!` writes an absent
        // option.
        "pending_credit_adjustments": props.pointer("/pendingCreditAdjustments/value"),
        "is_estimated_balance": props.get("isEstimatedBalance"),
        "credit_depleted": match config::billing_net_alert_usd() {
            Ok(threshold) => json!(amount.as_f64().is_some_and(|balance| balance <= threshold)),
            Err(_) => Value::Null,
        },
        "net_alert_detail": config::billing_net_alert_usd().err(),
        "grant_amount": grant_amount,
        "credit_used": credit_used,
        "grant_start_date": grant.and_then(|detail| detail.get("startDate")),
        "grant_end_date": grant.and_then(|detail| detail.get("endDate")),
        "grant_type": grant.and_then(|detail| detail.get("type")),
        "grant_status": grant.and_then(|detail| detail.get("status")),
        "billing_account": property_props.get("billingAccountDisplayName"),
        "billing_profile": property_props.get("billingProfileDisplayName"),
        "billing_profile_status": property_props.get("billingProfileStatus"),
        "subscription_billing_status": property_props.get("subscriptionBillingStatus"),
        "subscription_billing_type": property_props.get("subscriptionBillingType"),
        "overage_risk": property_props.get("billingProfileSpendingLimit").and_then(Value::as_str) == Some("Off"),
        "scope_detail": scope_detail,
        "raw": props,
    })
}
