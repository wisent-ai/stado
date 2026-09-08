//! The credit read itself: the OAuth client-credentials exchange, the ARM
//! balance endpoint the configured scope selects, and the subscription
//! billing-property lookup that turns a balance into a grant figure.

use serde_json::{json, Value};

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
    let missing: Vec<&str> = ["tenant_id", "client_id", "client_secret"]
        .into_iter()
        .filter(|key| {
            sp.get(*key)
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        })
        .collect();
    if !missing.is_empty() {
        return azure_error(
            "config_error",
            format!("Azure SP secret missing keys: {}", py_list_repr(&missing)),
        );
    }
    let tenant_id = sp["tenant_id"].as_str().expect("validated above");
    let client_id = sp["client_id"].as_str().expect("validated above");
    let client_secret = sp["client_secret"].as_str().expect("validated above");

    // Python ClientSecretCredential.get_token("https://management.azure.com/.default").
    let token_url = format!("{login_base}/{tenant_id}/oauth2/v2.0/token");
    let token = match client
        .post(&token_url)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("scope", "https://management.azure.com/.default"),
            ("grant_type", "client_credentials"),
        ])
        .send()
        .await
    {
        Err(err) => return azure_error("auth_error", err.to_string()),
        Ok(response) => {
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
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

    let response = match client.get(&url).bearer_auth(&token).send().await {
        Ok(response) => response,
        Err(err) => {
            return json!({"status": "arm_error", "detail": err.to_string(), "endpoint": url})
        }
    };
    let status = response.status();
    let body_text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let truncated: String = body_text.chars().take(usize::from(u16::MAX)).collect();
        return json!({
            "status": "arm_error",
            "detail": format!("HTTP {}: {truncated}", status.as_u16()),
            "endpoint": url,
        });
    }
    let balance_reported = status != reqwest::StatusCode::NO_CONTENT;
    let body: Value = if balance_reported {
        match serde_json::from_str(&body_text) {
            Ok(body) => body,
            Err(err) => return error_section(err.to_string()),
        }
    } else {
        json!({})
    };
    let props = body.get("properties").unwrap_or(&body).clone();
    let amount = extract_amount(&props);
    let estimated = props
        .pointer("/balanceSummary/estimatedBalance/value")
        .cloned()
        .unwrap_or_else(|| amount.clone());
    let currency = props
        .get("creditCurrency")
        .or_else(|| props.get("billingCurrency"))
        .or_else(|| props.pointer("/balanceSummary/currentBalance/currency"))
        .or_else(|| props.pointer("/amount/currency"))
        .cloned()
        .unwrap_or_else(|| json!("USD"));
    let expired = props
        .pointer("/expiredCredit/value")
        .cloned()
        .unwrap_or(Value::Null);
    let pending = props
        .pointer("/pendingEligibleCharges/value")
        .cloned()
        .unwrap_or(Value::Null);

    let mut billing_property = Value::Null;
    let mut scope_detail = Value::Null;
    if modern_scope.is_some() {
        if let Some(subscription) = subscription {
            let property_url = format!(
                "{arm_base}/subscriptions/{subscription}/providers/Microsoft.Billing/billingProperty/default?api-version=2024-04-01"
            );
            match client.get(&property_url).bearer_auth(&token).send().await {
                Err(err) => scope_detail = json!(err.to_string()),
                Ok(response) => {
                    let status = response.status();
                    let text = response.text().await.unwrap_or_default();
                    if status.is_success() {
                        billing_property = serde_json::from_str(&text)
                            .unwrap_or_else(|err| json!({"parse_error": err.to_string()}));
                    } else {
                        scope_detail = json!(format!("HTTP {}: {text}", status.as_u16()));
                    }
                }
            }
        }
    }
    let property_props = billing_property
        .get("properties")
        .unwrap_or(&billing_property);
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
    let grant_amount = grant
        .and_then(|detail| detail.get("amount"))
        .cloned()
        .unwrap_or(Value::Null);
    let credit_used = grant_amount
        .as_f64()
        .zip(amount.as_f64())
        .map(|(grant, balance)| json!(grant - balance))
        .unwrap_or(Value::Null);

    json!({
        "status": "ok",
        "balance_reported": balance_reported,
        "detail": if balance_reported {
            Value::Null
        } else {
            json!("Azure returned no credit balance; subscription grant metadata remains authoritative")
        },
        "available_balance": amount,
        "estimated_balance": estimated,
        "currency": currency,
        "expired_credit": expired,
        "pending_eligible_charges": pending,
        "pending_credit_adjustments": props.pointer("/pendingCreditAdjustments/value").cloned().unwrap_or(Value::Null),
        "is_estimated_balance": props.get("isEstimatedBalance").cloned().unwrap_or(Value::Null),
        "credit_depleted": extract_amount(&props).as_f64().is_some_and(|balance| balance <= config::billing_net_alert_usd()),
        "grant_amount": grant_amount,
        "credit_used": credit_used,
        "grant_start_date": grant.and_then(|detail| detail.get("startDate")).cloned().unwrap_or(Value::Null),
        "grant_end_date": grant.and_then(|detail| detail.get("endDate")).cloned().unwrap_or(Value::Null),
        "grant_type": grant.and_then(|detail| detail.get("type")).cloned().unwrap_or(Value::Null),
        "grant_status": grant.and_then(|detail| detail.get("status")).cloned().unwrap_or(Value::Null),
        "billing_account": property_props.get("billingAccountDisplayName").cloned().unwrap_or(Value::Null),
        "billing_profile": property_props.get("billingProfileDisplayName").cloned().unwrap_or(Value::Null),
        "billing_profile_status": property_props.get("billingProfileStatus").cloned().unwrap_or(Value::Null),
        "subscription_billing_status": property_props.get("subscriptionBillingStatus").cloned().unwrap_or(Value::Null),
        "subscription_billing_type": property_props.get("subscriptionBillingType").cloned().unwrap_or(Value::Null),
        "overage_risk": property_props.get("billingProfileSpendingLimit").and_then(Value::as_str) == Some("Off"),
        "scope_detail": scope_detail,
        "raw": props,
    })
}

/// MCA credit balance lives under balanceSummary/currentBalance/value. Legacy
/// APIs return `amount` or `availableBalance`; object values resolve through
/// their `value` member.
fn extract_amount(props: &Value) -> Value {
    if let Some(value) = props.pointer("/balanceSummary/currentBalance/value") {
        return value.clone();
    }
    let Some(map) = props.as_object() else {
        return Value::Null;
    };
    let amount = map
        .get("amount")
        .filter(|value| !value.is_null())
        .or_else(|| map.get("availableBalance"));
    match amount {
        Some(Value::Object(object)) => object.get("value").cloned().unwrap_or(Value::Null),
        Some(value) => value.clone(),
        None => Value::Null,
    }
}
