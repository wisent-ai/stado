//! Billing-credits collector.
//!
//! Port of `stado/monitor/billing.py`. Each Cloud Function tick this writes
//! gs://<BUCKET>/billing_health/credits.json following the exact convention
//! of host_health/<host>.json: a single JSON blob, an ISO-8601 reported_at,
//! and per-source sections that either carry data or the EXACT upstream
//! error (never a mock, never a silent skip — a failed source records its
//! real status/detail so the cause is visible without log spelunking).
//!
//! GCP section: derived entirely from the BigQuery billing export. Gross
//! cost, credits applied (negative), net cost, per-credit cumulative
//! consumption, and a 7-day credit burn rate. The depletion signal is the
//! latest-month net_cost crossing BILLING_NET_ALERT_USD — this needs no
//! knowledge of the original grant ceiling, which no GCP API exposes, so the
//! tracker stays fully automated.
//!
//! Azure section: available credit balance via the ARM REST API,
//! authenticated with a service principal whose JSON value is read only from
//! the separate Skarbiec service. Local files, process-environment secrets,
//! queue blobs, Azure Key Vault, and GCP Secret Manager are not credential
//! sources. A missing service, grant, or item is explicit in the provider
//! section; there is no fallback that can silently re-couple clouds.
//!
//! Transport deviations from Python (same on-the-wire data):
//! - Python uses the google-cloud-bigquery library; here the queries run
//!   over the BigQuery REST `queries` endpoint, so rows are parsed from the
//!   REST `rows[].f[].v` shape (all values string-typed) instead of the
//!   library's typed Row objects. The three SQL strings are byte-identical.
//! - The Python implementation's GCP Secret Manager credential lookup is
//!   replaced by an action-scoped request to the separate Skarbiec service.
//!   This removes the cross-cloud credential dependency.
//! - Python's `ClientSecretCredential` is the literal OAuth2 client-
//!   credentials POST to login.microsoftonline.com.
//! - Python records exception detail as `{type(e).__name__}: {e}`; Rust has
//!   no exception class names, so the detail is the error's Display text
//!   (the exact upstream error is preserved either way).
//!
//! Account health (NO Python original — this is new in the Rust runtime).
//! The credit signals above are computed INSIDE the `ok` branch of a
//! provider section: `credit_depleted` and `available_balance` only exist
//! when the query actually succeeded. So the moment an account is closed,
//! its billing export is revoked, or its service principal is disabled, the
//! section flips to `no_credentials`/`error`, every balance field vanishes,
//! and the balance alerts go quiet — the monitoring falls silent precisely
//! when it matters. That is how the GCP billing outage arrived with zero
//! warning.
//!
//! [`apply_health`] therefore folds a per-provider health record forward
//! across ticks inside the same blob ([`HEALTH_KEY`]): the last `ok`
//! timestamp, the start of the current failing run, and its length, so
//! "how long has this been broken" is answerable from the snapshot alone.
//! A section non-`ok` for longer than [`HEALTH_GRACE_SECONDS`] raises its
//! own alert naming the provider and the exact upstream cause, entirely
//! independent of any balance threshold. Every condition is keyed
//! ([`Signal::key`]) and the firing set is persisted, so a failure that
//! stays broken alerts on the transition rather than once per poll.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use chrono::{DateTime, SecondsFormat, Utc};
use regex::Regex;
use serde_json::{json, Value};

use super::alerts::send_alert;
use crate::config;
#[cfg(test)]
use crate::queue::secrets;
use crate::queue::{JobStorage, StorageError};

/// Blob written every tick (Python `_BLOB`).
pub const BLOB: &str = "billing_health/credits.json";

/// BigQuery dataset/table identifiers cannot be bound as query parameters.
/// They originate from controlled config, but we still hard-validate the
/// shape so an env override can never inject SQL (Python `_IDENT_RE`).
static IDENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_]+$").expect("static regex compiles"));

const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const BIGQUERY_BASE: &str = "https://bigquery.googleapis.com";
const AZURE_LOGIN_BASE: &str = "https://login.microsoftonline.com";
/// Python `_ARM`.
const ARM_BASE: &str = "https://management.azure.com";

fn log(msg: &str) {
    eprintln!("[tick] {msg}");
}

fn error_section(detail: String) -> Value {
    json!({"status": "error", "detail": detail})
}

/// Python `repr()` of a string: single quotes.
fn py_repr(value: &str) -> String {
    format!("'{value}'")
}

/// Python `str()` of a string list: `['a', 'b']`.
fn py_list_repr(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| format!("'{i}'")).collect();
    format!("[{}]", quoted.join(", "))
}

/// Python `str()` of a JSON value: strings unquoted, null -> "None",
/// numbers in Python float style (serde_json prints 8.0 as "8.0").
fn py_value(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Python `str()` of a float: integral floats keep a trailing ".0".
fn py_f64(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

async fn gcp_token() -> Result<String, String> {
    let auth = crate::skarbiec::gcp_provider()
        .await
        .map_err(|e| e.to_string())?;
    let token = auth
        .token(&[CLOUD_PLATFORM_SCOPE])
        .await
        .map_err(|e| e.to_string())?;
    Ok(token.as_str().to_string())
}

// ---------------------------------------------------------------------------
// GCP section — BigQuery billing export
// ---------------------------------------------------------------------------

/// Spend/credits/burn from the BigQuery billing export. Fails into a section
/// error only on a genuine client/permission fault; the caller records that
/// as the section's error so one broken source never suppresses the other
/// (Python `_gcp_section`).
async fn gcp_section() -> Value {
    let dataset = config::billing_dataset();
    let table = config::billing_table();
    if !IDENT_RE.is_match(dataset) || !IDENT_RE.is_match(table) {
        return json!({
            "status": "config_error",
            "detail": format!(
                "invalid dataset/table identifier {}/{}",
                py_repr(dataset),
                py_repr(table)
            ),
        });
    }
    let token = match gcp_token().await {
        Ok(token) => token,
        Err(err) => return error_section(err),
    };
    let client = reqwest::Client::new();
    gcp_section_with(&client, BIGQUERY_BASE, config::project(), &token).await
}

/// POST one query job; the rows array (possibly absent) on success. A
/// non-2xx response is an error whose detail carries the exact upstream
/// body.
async fn run_bq_query(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    sql: &str,
) -> Result<Vec<Value>, String> {
    let response = client
        .post(url)
        .bearer_auth(token)
        .json(&json!({"query": sql, "useLegacySql": false}))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {body}"));
    }
    let body: Value = response.json().await.map_err(|e| e.to_string())?;
    Ok(body
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// The `v` values of one REST row (`rows[].f[].v`). BigQuery REST returns
/// every value as a string regardless of column type.
fn bq_cols(row: &Value) -> Vec<Value> {
    row.get("f")
        .and_then(Value::as_array)
        .map(|cols| {
            cols.iter()
                .map(|c| c.get("v").cloned().unwrap_or(Value::Null))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a REST string value into a JSON number (or null when absent /
/// unparseable) — the REST equivalent of the library's typed Row fields.
fn bq_num(value: Option<&Value>) -> Value {
    match value {
        Some(Value::Number(n)) => json!(n),
        Some(Value::String(s)) => s.parse::<f64>().ok().map_or(Value::Null, |n| json!(n)),
        _ => Value::Null,
    }
}

fn bq_str(value: Option<&Value>) -> Value {
    match value {
        Some(Value::String(s)) => json!(s),
        Some(Value::Number(n)) => json!(n.to_string()),
        _ => Value::Null,
    }
}

/// The three hand-written SQL strings are byte-identical to Python
/// (including the backtick-quoted `{project}.{dataset}.{table}` and the
/// 90-day / 7-day windows).
fn billing_sql(project: &str) -> (String, String, String) {
    let fq = format!(
        "`{project}.{}.{}`",
        config::billing_dataset(),
        config::billing_table()
    );
    let monthly_sql = format!(
        "\n        SELECT FORMAT_TIMESTAMP('%Y-%m', usage_start_time) AS month,\n               ROUND(SUM(cost), 2) AS gross,\n               ROUND(SUM(IFNULL((SELECT SUM(c.amount)\n                       FROM UNNEST(credits) c), 0)), 2) AS credits,\n               ROUND(SUM(cost) + SUM(IFNULL((SELECT SUM(c.amount)\n                       FROM UNNEST(credits) c), 0)), 2) AS net,\n               ANY_VALUE(currency) AS currency\n        FROM {fq}\n        WHERE usage_start_time >= TIMESTAMP_SUB(CURRENT_TIMESTAMP(),\n                                                INTERVAL 90 DAY)\n        GROUP BY month ORDER BY month\n    "
    );
    let credit_sql = format!(
        "\n        SELECT c.name AS name, c.type AS type,\n               ROUND(SUM(c.amount), 2) AS cumulative,\n               ANY_VALUE(currency) AS currency\n        FROM {fq}, UNNEST(credits) c\n        GROUP BY name, type ORDER BY cumulative\n    "
    );
    let burn_sql = format!(
        "\n        SELECT ROUND(AVG(daily), 2) AS avg_daily_credit_7d FROM (\n          SELECT DATE(usage_start_time) AS d,\n                 SUM(IFNULL((SELECT SUM(c.amount)\n                     FROM UNNEST(credits) c), 0)) AS daily\n          FROM {fq}\n          WHERE usage_start_time >= TIMESTAMP_SUB(CURRENT_TIMESTAMP(),\n                                                  INTERVAL 7 DAY)\n          GROUP BY d)\n    "
    );
    (monthly_sql, credit_sql, burn_sql)
}

/// Injectable twin of [`gcp_section`] (base URL + token explicit) so tests
/// can point BigQuery at the loopback mock.
async fn gcp_section_with(
    client: &reqwest::Client,
    base_url: &str,
    project: &str,
    token: &str,
) -> Value {
    let url = format!("{base_url}/bigquery/v2/projects/{project}/queries");
    let (monthly_sql, credit_sql, burn_sql) = billing_sql(project);

    let monthly_rows = match run_bq_query(client, &url, token, &monthly_sql).await {
        Ok(rows) => rows,
        Err(err) => return error_section(err),
    };
    let credit_rows = match run_bq_query(client, &url, token, &credit_sql).await {
        Ok(rows) => rows,
        Err(err) => return error_section(err),
    };
    let burn_rows = match run_bq_query(client, &url, token, &burn_sql).await {
        Ok(rows) => rows,
        Err(err) => return error_section(err),
    };

    let monthly: Vec<Value> = monthly_rows
        .iter()
        .map(|row| {
            let cols = bq_cols(row);
            json!({
                "month": bq_str(cols.first()),
                "gross": bq_num(cols.get(1)),
                "credits": bq_num(cols.get(2)),
                "net": bq_num(cols.get(3)),
                "currency": bq_str(cols.get(4)),
            })
        })
        .collect();
    let credits: Vec<Value> = credit_rows
        .iter()
        .map(|row| {
            let cols = bq_cols(row);
            json!({
                "name": bq_str(cols.first()),
                "type": bq_str(cols.get(1)),
                "cumulative": bq_num(cols.get(2)),
                "currency": bq_str(cols.get(3)),
            })
        })
        .collect();
    let burn = burn_rows
        .first()
        .map_or(Value::Null, |row| bq_num(bq_cols(row).first()));

    let threshold = config::billing_net_alert_usd();
    let latest_net = monthly.last().map_or(Value::Null, |m| {
        m.get("net").cloned().unwrap_or(Value::Null)
    });
    let depleted = latest_net.as_f64().is_some_and(|net| net > threshold);

    json!({
        "status": "ok",
        "monthly": monthly,
        "credits": credits,
        "avg_daily_credit_applied_7d": burn,
        "latest_month_net_usd": latest_net,
        "net_alert_threshold_usd": threshold,
        "credit_depleted": depleted,
    })
}

// ---------------------------------------------------------------------------
// Azure section — Skarbiec SP + ARM available balance
// ---------------------------------------------------------------------------

#[cfg(test)]
/// Read the Azure SP JSON from Secret Manager over REST. Returns `Ok(None)`
/// (not an error) when the secret simply does not exist (404) or is not
/// accessible (403), so the caller can record an explicit no_credentials
/// status (Python `_fetch_azure_sp`).
async fn fetch_azure_sp_with(
    client: &reqwest::Client,
    base_url: &str,
    project: &str,
    secret: &str,
    token: &str,
) -> Result<Option<Value>, String> {
    use base64::Engine;
    let url = format!("{base_url}/v1/projects/{project}/secrets/{secret}/versions/latest:access");
    let response = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if status.as_u16() == 404 || status.as_u16() == 403 {
        return Ok(None);
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {body}"));
    }
    let payload: Value = response.json().await.map_err(|e| e.to_string())?;
    let data = payload
        .get("payload")
        .and_then(|p| p.get("data"))
        .and_then(Value::as_str)
        .ok_or_else(|| "secret response missing payload.data".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| e.to_string())?;
    let sp: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(Some(sp))
}

#[cfg(test)]
/// Both credential locations, named in the order they were checked, so the
/// status says where to put the secret rather than only that it is missing.
fn no_credentials_section(store: &JobStorage) -> Value {
    let secret = config::azure_billing_secret();
    let blob = secrets::blob_path(secret).unwrap_or_else(|_| secret.to_string());
    json!({
        "status": "no_credentials",
        "detail": format!(
            "no Azure billing service principal in the stado secret store \
             ({blob} on the {} store — put one there with \
             `stado secrets put {secret}`), and Secret Manager secret \
             '{secret}' not present in project {}; alternatively set \
             AZURE_TENANT_ID/AZURE_CLIENT_ID/AZURE_CLIENT_SECRET plus \
             WC_AZURE_BILLING_ACCOUNT and WC_AZURE_BILLING_PROFILE_SYSTEM_ID \
             to activate Azure credit tracking",
            store.backend_name(),
            config::project()
        ),
    })
}

/// Available credit balance via ARM. The service-principal object is read from
/// the separate Skarbiec repository/service and nowhere else. A
/// vault/auth/request failure is terminal for this source: silently falling
/// through would bypass the credential policy.
async fn azure_section(_store: &JobStorage) -> Value {
    let secret_name = config::azure_billing_secret();
    let vault = match crate::skarbiec::Client::configured() {
        Ok(vault) => vault,
        Err(err) => return azure_error("skarbiec_error", err.to_string()),
    };
    let sp = match vault.read_item(secret_name).await {
        Ok(sp) => sp,
        Err(crate::skarbiec::SkarbiecError::Response { status, .. })
            if status == reqwest::StatusCode::NOT_FOUND.as_u16() =>
        {
            return json!({
                "status": "no_credentials",
                "detail": format!("Skarbiec item {secret_name:?} does not exist"),
            })
        }
        Err(err) => return azure_error("skarbiec_error", err.to_string()),
    };
    let client = reqwest::Client::new();
    azure_section_with(&client, &sp, AZURE_LOGIN_BASE, ARM_BASE).await
}

fn azure_error(status: &str, detail: String) -> Value {
    json!({"status": status, "detail": detail})
}

/// Injectable twin of the post-secret half of [`azure_section`]: SP JSON +
/// login/ARM base URLs explicit, so tests can run the OAuth + ARM exchange
/// against the loopback mock.
async fn azure_section_with(
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
            "Azure SP secret needs billing_account+billing_profile or subscription_id".to_string(),
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
    if status == reqwest::StatusCode::NO_CONTENT {
        return json!({
            "status": "no_credits",
            "detail": "Azure returned no credit balance for this billing profile",
            "endpoint": url,
        });
    }
    let body: Value = match serde_json::from_str(&body_text) {
        Ok(body) => body,
        Err(err) => return error_section(err.to_string()),
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

// ---------------------------------------------------------------------------
// collect_billing
// ---------------------------------------------------------------------------

/// Query every billing source and return the canonical snapshot without
/// persisting it. Used by both the coordinator collector and `billing
/// refresh`. `store` remains the eventual snapshot destination; the Azure
/// service-principal credential is resolved independently from Azure Key
/// Vault.
pub async fn live_snapshot(store: &JobStorage) -> Value {
    let variants = crate::capabilities::get(crate::capabilities::RuntimeFacet::Billing.as_str())
        .map(|capability| capability.variants)
        .unwrap_or_default();
    let sections = futures::future::join_all(variants.iter().map(|variant| async move {
        let value = match variant.adapter {
            crate::capabilities::RuntimeAdapter::Billing(
                crate::capabilities::BillingAdapter::Gcp,
            ) => gcp_section().await,
            crate::capabilities::RuntimeAdapter::Billing(
                crate::capabilities::BillingAdapter::Azure,
            ) => azure_section(store).await,
            _ => error_section(format!(
                "billing catalog variant {:?} has no billing adapter",
                variant.id
            )),
        };
        (variant.id, value)
    }))
    .await;
    billing_document_from_sections(sections)
}

fn billing_document_from_sections(
    sections: impl IntoIterator<Item = (&'static str, Value)>,
) -> Value {
    let mut document = serde_json::Map::new();
    document.insert(
        "reported_at".to_string(),
        Value::String(Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false)),
    );
    document.insert(
        "project".to_string(),
        Value::String(config::project().to_string()),
    );
    for (provider, section) in sections {
        document.insert(provider.to_string(), section);
    }
    Value::Object(document)
}

fn billing_document(gcp: Value, azure: Value) -> Value {
    billing_document_from_sections([
        (crate::capabilities::ProviderId::Gcp.as_str(), gcp),
        (crate::capabilities::ProviderId::Azure.as_str(), azure),
    ])
}

/// Persist one already-built billing document.
pub async fn persist_snapshot(store: &JobStorage, document: &Value) -> Result<(), StorageError> {
    let pretty = serde_json::to_string_pretty(document).expect("json value serializes");
    store.upload_text(BLOB, &pretty).await
}

/// The last published snapshot, or `None` when the blob has never been
/// written. Unparseable JSON also reads as `None`: a corrupt record must
/// never stop the next tick from publishing a good one.
pub async fn load_snapshot(store: &JobStorage) -> Result<Option<Value>, StorageError> {
    Ok(store
        .download_text(BLOB)
        .await?
        .and_then(|text| serde_json::from_str(&text).ok()))
}

/// Assemble and upload billing_health/credits.json. Each source is isolated:
/// a failure from one is captured into its section as the exact error string.
pub async fn collect_billing(store: &JobStorage) {
    publish(store, live_snapshot(store).await).await;
}

/// Compatibility helper for callers that already hold provider sections.
pub async fn write_billing_blob(store: &JobStorage, gcp: Value, azure: Value) {
    publish(store, billing_document(gcp, azure)).await;
}

/// Fold health forward, commit the firing set, persist, log, and dispatch
/// the transitions. Every alerting caller (the coordinator collector and
/// `stado billing watch`) goes through here, so the de-duplication state in
/// the blob has exactly one writer discipline.
async fn publish(store: &JobStorage, mut document: Value) {
    let previous = match load_snapshot(store).await {
        Ok(previous) => previous,
        Err(err) => {
            // A history read failure must not suppress this tick; it only
            // costs the elapsed-time context on any alert it raises.
            log(&format!("billing history unreadable: {err}"));
            None
        }
    };
    let evaluation = apply_health(previous.as_ref(), &mut document, Utc::now());
    commit_firing(&mut document, &evaluation);
    if let Err(err) = persist_snapshot(store, &document).await {
        log(&format!("billing upload failed: {err}"));
        return;
    }
    emit_alerts(&document);
    dispatch_signals(&evaluation).await;
}

fn emit_alerts(document: &Value) {
    if let Some(capability) = crate::capabilities::get("billing") {
        for variant in capability.variants {
            let section = &document[variant.id];
            match variant.adapter {
                crate::capabilities::RuntimeAdapter::Billing(
                    crate::capabilities::BillingAdapter::Gcp,
                ) if section
                    .get("credit_depleted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false) =>
                {
                    log(&format!(
                        "BILLING ALERT: GCP latest-month net ${} exceeds ${} — promotion credit exhausted or rate-capped",
                        py_value(section.get("latest_month_net_usd")),
                        py_value(section.get("net_alert_threshold_usd")),
                    ));
                }
                crate::capabilities::RuntimeAdapter::Billing(
                    crate::capabilities::BillingAdapter::Azure,
                ) => {
                    let threshold = config::billing_net_alert_usd();
                    if let Some(balance) = section.get("available_balance").and_then(Value::as_f64)
                    {
                        if balance < threshold {
                            log(&format!(
                                "BILLING ALERT: Azure available credit balance {} below {}",
                                py_f64(balance),
                                py_f64(threshold),
                            ));
                        }
                    }
                    if section.get("overage_risk").and_then(Value::as_bool) == Some(true) {
                        log(
                            "BILLING WARNING: Azure spending limit is off; paid overage can continue after credits",
                        );
                    }
                }
                _ => {}
            }
        }
    }
    let provider_health = document[HEALTH_KEY]
        .get("providers")
        .and_then(Value::as_object);
    if let Some(providers) = provider_health {
        for (provider, health) in providers {
            if health.get("degraded").and_then(Value::as_bool) == Some(true) {
                log(&format!(
                    "BILLING ALERT: {provider} account unhealthy — status {} for {}, last good report {}",
                    py_value(health.get("status")),
                    humanize(health.get("failing_seconds").and_then(Value::as_i64).unwrap_or_default()),
                    py_value(health.get("last_ok")),
                ));
            }
        }
    }
    log(&format!(
        "billing: gcp={} azure={} -> {BLOB}",
        py_value(
            provider_health
                .and_then(|providers| providers.get("gcp"))
                .and_then(|health| health.get("status"))
        ),
        py_value(
            provider_health
                .and_then(|providers| providers.get("azure"))
                .and_then(|health| health.get("status"))
        ),
    ));
}

// ---------------------------------------------------------------------------
// Account health
// ---------------------------------------------------------------------------

/// Time-unit ladder. The base unit is explicit; larger units are derived
/// from standard-library integer constants and prior entries in the ladder.
pub const SECONDS_PER_SECOND: u64 = true as u64;
/// `64 - 32/8 == 60`.
pub const SECONDS_PER_MINUTE: u64 = (u64::BITS - u32::BITS / u8::BITS) as u64;
/// `60 * 60 == 3600`.
pub const SECONDS_PER_HOUR: u64 = SECONDS_PER_MINUTE * SECONDS_PER_MINUTE;
/// `3600 * (32 - 8) == 86400`.
pub const SECONDS_PER_DAY: u64 = SECONDS_PER_HOUR * (u32::BITS - u8::BITS) as u64;

/// How long a provider section may report a non-`ok` status before it is
/// alerted on. One hour: long enough to ride out one failed collector tick
/// or a transient ARM/BigQuery 5xx, short enough that a closed account or a
/// disabled service principal is reported within the hour it breaks.
pub const HEALTH_GRACE_SECONDS: i64 = SECONDS_PER_HOUR as i64;

/// Key of the health record inside the billing document. It lives in the
/// same blob as the sections it describes, so the last-good timestamps
/// travel with the snapshot — and with `queue::copy`, whose
/// `CANONICAL_PREFIXES` already carries `billing_health/`.
pub const HEALTH_KEY: &str = "account_health";

/// Provider sections carried by the billing document, in catalog order.
pub fn providers() -> Vec<&'static str> {
    crate::capabilities::provider_ids(crate::capabilities::RuntimeFacet::Billing)
        .into_iter()
        .map(|provider| provider.as_str())
        .collect()
}

/// The one section status that means "this query actually succeeded".
const OK_STATUS: &str = "ok";
/// Status recorded for a provider key the document does not carry at all —
/// itself a defect worth alerting on, never a silent skip.
const MISSING_STATUS: &str = "missing";

/// Per-provider account health, folded forward across ticks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealth {
    pub provider: String,
    /// The section's own `status` (`ok`, `no_credentials`, `error`, ...).
    pub status: String,
    /// The section's own `detail`: the exact upstream cause, verbatim.
    pub detail: String,
    /// Last tick at which this section reported `ok`, RFC-3339.
    pub last_ok: Option<String>,
    /// First tick of the current non-`ok` run, RFC-3339.
    pub failing_since: Option<String>,
    /// Length of the current non-`ok` run, in seconds.
    pub failing_seconds: i64,
    /// Non-`ok` for longer than [`HEALTH_GRACE_SECONDS`].
    pub degraded: bool,
}

impl ProviderHealth {
    /// Whether the section reported a successful query this tick.
    pub fn healthy(&self) -> bool {
        self.status == OK_STATUS
    }
}

/// One alert condition. `key` is stable across ticks, so a condition that
/// stays true alerts on the transition into it rather than once per poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub key: String,
    pub subject: String,
    pub message: String,
}

/// Result of evaluating one billing document against the previous one.
#[derive(Debug, Clone, Default)]
pub struct HealthEvaluation {
    /// Health of every billing provider in catalog order.
    pub providers: Vec<ProviderHealth>,
    /// Every condition true right now.
    pub firing: Vec<Signal>,
    /// Conditions that were NOT firing at the previous tick.
    pub new_signals: Vec<Signal>,
    /// Keys that were firing at the previous tick and are not any more.
    pub cleared: Vec<String>,
}

/// Fold the previous snapshot's health record into `document`, write the
/// updated record under [`HEALTH_KEY`], and report what is firing.
///
/// The firing set carried into `document` is the PREVIOUS one, untouched:
/// only [`commit_firing`] advances it, and only alert-dispatching callers
/// may call that. A read-only republish (`stado billing refresh`) therefore
/// cannot swallow a transition the collector or `billing watch` still owes.
pub fn apply_health(
    previous: Option<&Value>,
    document: &mut Value,
    now: DateTime<Utc>,
) -> HealthEvaluation {
    let stamp = now.to_rfc3339_opts(SecondsFormat::Micros, false);
    let history = previous.map(|doc| &doc[HEALTH_KEY]);
    let previously_firing: BTreeSet<String> = history
        .and_then(|health| health.get("firing"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();

    let billing_providers = providers();
    let mut providers = Vec::with_capacity(billing_providers.len());
    let mut record = serde_json::Map::new();
    for provider in billing_providers {
        let prior = history
            .and_then(|health| health.get("providers"))
            .and_then(|map| map.get(provider));
        let health = fold_provider(provider, document.get(provider), prior, &stamp, now);
        record.insert(provider.to_string(), health_value(&health));
        providers.push(health);
    }
    document[HEALTH_KEY] = json!({
        "grace_seconds": HEALTH_GRACE_SECONDS,
        "providers": Value::Object(record),
        "firing": previously_firing.iter().collect::<Vec<_>>(),
    });

    let firing = signals(document, &providers);
    let live: BTreeSet<&str> = firing.iter().map(|signal| signal.key.as_str()).collect();
    let new_signals = firing
        .iter()
        .filter(|signal| !previously_firing.contains(&signal.key))
        .cloned()
        .collect();
    let cleared = previously_firing
        .iter()
        .filter(|key| !live.contains(key.as_str()))
        .cloned()
        .collect();
    HealthEvaluation {
        providers,
        firing,
        new_signals,
        cleared,
    }
}

/// Replace the document's firing-signal set with what is firing NOW. Call
/// this only from a path that also dispatches, never from a read-only one —
/// see [`apply_health`].
pub fn commit_firing(document: &mut Value, evaluation: &HealthEvaluation) {
    let keys: Vec<Value> = evaluation
        .firing
        .iter()
        .map(|signal| json!(signal.key))
        .collect();
    document[HEALTH_KEY]["firing"] = Value::Array(keys);
}

/// Fan every newly-firing signal out through
/// [`crate::monitor::alerts::send_alert`], which fault-isolates each
/// channel. Recovery is logged, not alerted: an outage that ends does not
/// need to wake anyone.
pub async fn dispatch_signals(evaluation: &HealthEvaluation) {
    for signal in &evaluation.new_signals {
        log(&format!("ALERT {}: {}", signal.key, signal.message));
        send_alert(config::alerts_topic(), &signal.message, &signal.subject).await;
    }
    for key in &evaluation.cleared {
        log(&format!("RECOVERED {key}"));
    }
}

/// Carry one provider's history forward against this tick's section.
fn fold_provider(
    provider: &str,
    section: Option<&Value>,
    prior: Option<&Value>,
    stamp: &str,
    now: DateTime<Utc>,
) -> ProviderHealth {
    let status = section
        .and_then(|section| section.get("status"))
        .and_then(Value::as_str)
        .unwrap_or(MISSING_STATUS)
        .to_string();
    let detail = section
        .and_then(|section| section.get("detail"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let prior_str = |field: &str| {
        prior
            .and_then(|prior| prior.get(field))
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    if status == OK_STATUS {
        return ProviderHealth {
            provider: provider.to_string(),
            status,
            detail,
            last_ok: Some(stamp.to_string()),
            failing_since: None,
            failing_seconds: i64::default(),
            degraded: false,
        };
    }
    // An already-open run keeps its original start, so the elapsed figure
    // survives restarts of whatever process happens to be collecting.
    let failing_since = prior_str("failing_since").unwrap_or_else(|| stamp.to_string());
    let failing_seconds = elapsed_seconds(&failing_since, now);
    ProviderHealth {
        provider: provider.to_string(),
        status,
        detail,
        last_ok: prior_str("last_ok"),
        failing_since: Some(failing_since),
        degraded: failing_seconds >= HEALTH_GRACE_SECONDS,
        failing_seconds,
    }
}

fn health_value(health: &ProviderHealth) -> Value {
    json!({
        "status": health.status,
        "detail": health.detail,
        "last_ok": health.last_ok,
        "failing_since": health.failing_since,
        "failing_seconds": health.failing_seconds,
        "degraded": health.degraded,
    })
}

/// Seconds between an RFC-3339 stamp and `now`, floored at zero. An
/// unparseable stamp yields zero, which keeps a corrupt record quiet rather
/// than alert-storming on garbage.
fn elapsed_seconds(since: &str, now: DateTime<Utc>) -> i64 {
    DateTime::parse_from_rfc3339(since)
        .map(|start| {
            (now - start.with_timezone(&Utc))
                .num_seconds()
                .max(i64::default())
        })
        .unwrap_or_default()
}

/// Every alert condition true for this document.
///
/// Account health comes FIRST and is computed from the section status
/// alone, never from a balance field. That is the whole point: a section
/// that is not `ok` carries no `credit_depleted` and no `available_balance`
/// — both live inside the success branch — so the three balance conditions
/// below are structurally incapable of firing for a provider whose account
/// or credentials just died. The health signal is what speaks then.
fn signals(document: &Value, providers: &[ProviderHealth]) -> Vec<Signal> {
    let mut firing = Vec::new();
    for health in providers.iter().filter(|health| health.degraded) {
        let cause = if health.detail.is_empty() {
            "no detail reported by the provider"
        } else {
            health.detail.as_str()
        };
        firing.push(Signal {
            key: format!("account_health:{}", health.provider),
            subject: format!("stado billing: {} account unhealthy", health.provider),
            message: format!(
                "BILLING ACCOUNT HEALTH: the {} billing section has reported '{}' since {} \
                 ({} and counting), past the {} grace period. Last good report: {}. \
                 Cause: {}. While this persists {} publishes no balance at all, so the \
                 credit-threshold alert CANNOT fire — treat this as the outage warning.",
                health.provider,
                health.status,
                health.failing_since.as_deref().unwrap_or("an unknown time"),
                humanize(health.failing_seconds),
                humanize(HEALTH_GRACE_SECONDS),
                health.last_ok.as_deref().unwrap_or("never"),
                cause,
                health.provider,
            ),
        });
    }

    if let Some(capability) = crate::capabilities::get("billing") {
        for variant in capability.variants {
            let section = &document[variant.id];
            match variant.adapter {
                crate::capabilities::RuntimeAdapter::Billing(
                    crate::capabilities::BillingAdapter::Gcp,
                ) if section
                    .get("credit_depleted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false) =>
                {
                    firing.push(Signal {
                        key: format!("credit_depleted:{}", variant.id),
                        subject: "stado billing: GCP promotion credit exhausted".to_string(),
                        message: format!(
                            "BILLING ALERT: GCP latest-month net ${} exceeds ${} — promotion credit exhausted or rate-capped",
                            py_value(section.get("latest_month_net_usd")),
                            py_value(section.get("net_alert_threshold_usd")),
                        ),
                    });
                }
                crate::capabilities::RuntimeAdapter::Billing(
                    crate::capabilities::BillingAdapter::Azure,
                ) => {
                    let threshold = config::billing_net_alert_usd();
                    if let Some(balance) = section.get("available_balance").and_then(Value::as_f64)
                    {
                        if balance < threshold {
                            firing.push(Signal {
                                key: format!("balance_low:{}", variant.id),
                                subject: "stado billing: Azure credit balance low".to_string(),
                                message: format!(
                                    "BILLING ALERT: Azure available credit balance {} below {}",
                                    py_f64(balance),
                                    py_f64(threshold),
                                ),
                            });
                        }
                    }
                    if section.get("overage_risk").and_then(Value::as_bool) == Some(true) {
                        firing.push(Signal {
                            key: format!("overage_risk:{}", variant.id),
                            subject: "stado billing: Azure spending limit is off".to_string(),
                            message: "BILLING WARNING: Azure spending limit is off; paid overage can continue after credits".to_string(),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    firing
}

/// Elapsed seconds as `1d 2h 3m`. Fed only by [`elapsed_seconds`], so the
/// input is already non-negative; a negative one degrades to `0m`.
pub fn humanize(seconds: i64) -> String {
    let total = u64::try_from(seconds).unwrap_or_default();
    let days = total / SECONDS_PER_DAY;
    let hours = (total % SECONDS_PER_DAY) / SECONDS_PER_HOUR;
    let minutes = (total % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE;
    let mut parts = Vec::new();
    if days > u64::default() {
        parts.push(format!("{days}d"));
    }
    if hours > u64::default() {
        parts.push(format!("{hours}h"));
    }
    if minutes > u64::default() || parts.is_empty() {
        parts.push(format!("{minutes}m"));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::queue::LocalBackend;
    use crate::testutil::{http_response, mock_http};

    #[tokio::test]
    async fn gcp_section_sends_verbatim_sql_and_parses_rows() {
        let monthly = r#"{"rows":[{"f":[{"v":"2026-06"},{"v":"120.5"},{"v":"-20.5"},{"v":"100.0"},{"v":"USD"}]},{"f":[{"v":"2026-07"},{"v":"10.5"},{"v":"-2.5"},{"v":"8.0"},{"v":"USD"}]}]}"#;
        let credits = r#"{"rows":[{"f":[{"v":"Promotion"},{"v":"CREDIT_TYPE_PROMOTION"},{"v":"-500.0"},{"v":"USD"}]}]}"#;
        let burn = r#"{"rows":[{"f":[{"v":"-3.25"}]}]}"#;
        let mock = mock_http(vec![
            http_response(200, "OK", monthly),
            http_response(200, "OK", credits),
            http_response(200, "OK", burn),
        ])
        .await;
        let client = reqwest::Client::new();
        let section = gcp_section_with(&client, &mock.base_url, "test-project", "tok").await;

        assert_eq!(section["status"], "ok");
        assert_eq!(section["monthly"][0]["month"], "2026-06");
        assert_eq!(section["monthly"][0]["gross"], json!(120.5));
        assert_eq!(section["monthly"][0]["credits"], json!(-20.5));
        assert_eq!(section["monthly"][1]["net"], json!(8.0));
        assert_eq!(section["credits"][0]["name"], "Promotion");
        assert_eq!(section["credits"][0]["cumulative"], json!(-500.0));
        assert_eq!(section["avg_daily_credit_applied_7d"], json!(-3.25));
        assert_eq!(section["latest_month_net_usd"], json!(8.0));
        assert_eq!(section["credit_depleted"], json!(false));

        let requests = mock.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 3);
        let fq = "`test-project.billing_export.gcp_billing_export_v1_017364_D3B657_F207B5`";
        assert!(requests[0].contains("INTERVAL 90 DAY"), "{}", requests[0]);
        assert!(
            requests[0].contains("GROUP BY month ORDER BY month"),
            "{}",
            requests[0]
        );
        assert!(requests[1].contains("UNNEST(credits) c"), "{}", requests[1]);
        assert!(
            requests[1].contains("GROUP BY name, type ORDER BY cumulative"),
            "{}",
            requests[1]
        );
        assert!(
            requests[2].contains("avg_daily_credit_7d"),
            "{}",
            requests[2]
        );
        assert!(requests[2].contains("INTERVAL 7 DAY"), "{}", requests[2]);
        for request in requests.iter() {
            assert!(request.contains(fq), "{request}");
            assert!(request.contains(r#""useLegacySql":false"#), "{request}");
            assert!(
                request.contains("POST /bigquery/v2/projects/test-project/queries "),
                "{request}"
            );
            assert!(request.contains("authorization: Bearer tok"), "{request}");
        }
    }

    #[tokio::test]
    async fn gcp_section_records_exact_upstream_error() {
        let mock = mock_http(vec![http_response(
            403,
            "Forbidden",
            r#"{"error":{"message":"Denied"}}"#,
        )])
        .await;
        let client = reqwest::Client::new();
        let section = gcp_section_with(&client, &mock.base_url, "p", "tok").await;
        assert_eq!(section["status"], "error");
        let detail = section["detail"].as_str().expect("detail string");
        assert!(detail.contains("403"), "{detail}");
        assert!(detail.contains("Denied"), "{detail}");
    }

    #[tokio::test]
    async fn write_billing_blob_persists_sections_and_error_states() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_str().expect("utf8 path")).expect("backend");
        let store = JobStorage::with_backend_and_bucket(Arc::new(backend), "local", "test-bucket");

        let gcp = json!({"status": "ok", "credit_depleted": false, "latest_month_net_usd": 8.0});
        let azure = json!({"status": "error", "detail": "boom: secret manager unreachable"});
        write_billing_blob(&store, gcp, azure).await;

        let text = store
            .download_text(BLOB)
            .await
            .expect("download")
            .expect("blob written");
        let doc: Value = serde_json::from_str(&text).expect("valid json");
        assert!(doc["reported_at"].is_string());
        assert_eq!(doc["project"], json!(config::project()));
        assert_eq!(doc["gcp"]["status"], "ok");
        // A failing source lands as the exact error section, never skipped.
        assert_eq!(doc["azure"]["status"], "error");
        assert_eq!(doc["azure"]["detail"], "boom: secret manager unreachable");
    }

    #[tokio::test]
    async fn azure_section_exchanges_token_and_parses_balance() {
        let token_response = r#"{"access_token":"armtok","token_type":"Bearer","expires_in":3600}"#;
        let arm_response = r#"{"properties":{"amount":{"value":42.5}}}"#;
        let mock = mock_http(vec![
            http_response(200, "OK", token_response),
            http_response(200, "OK", arm_response),
        ])
        .await;
        let client = reqwest::Client::new();
        let sp = json!({
            "tenant_id": "tid",
            "client_id": "cid",
            "client_secret": "sec",
            "subscription_id": "sub-1",
        });
        let section = azure_section_with(&client, &sp, &mock.base_url, &mock.base_url).await;

        assert_eq!(section["status"], "ok");
        assert_eq!(section["available_balance"], json!(42.5));
        assert_eq!(section["raw"]["amount"]["value"], json!(42.5));

        let requests = mock.requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0].starts_with("POST /tid/oauth2/v2.0/token "),
            "{}",
            requests[0]
        );
        assert!(
            requests[0].contains("grant_type=client_credentials"),
            "{}",
            requests[0]
        );
        assert!(requests[0].contains("client_id=cid"), "{}", requests[0]);
        assert!(requests[0].contains("client_secret=sec"), "{}", requests[0]);
        assert!(
            requests[0].contains("scope=https%3A%2F%2Fmanagement.azure.com%2F.default"),
            "{}",
            requests[0]
        );
        assert!(
            requests[1].starts_with(
                "GET /subscriptions/sub-1/providers/Microsoft.Consumption/balances?api-version=2019-10-01 "
            ),
            "{}",
            requests[1]
        );
        let auth_line = requests[1]
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
            .expect("authorization header");
        assert_eq!(auth_line.trim(), "authorization: Bearer armtok");
    }

    #[tokio::test]
    async fn azure_section_billing_account_path_and_arm_error() {
        let token_response = r#"{"access_token":"armtok"}"#;
        let mock = mock_http(vec![
            http_response(200, "OK", token_response),
            http_response(400, "Bad Request", "ARM said no"),
        ])
        .await;
        let client = reqwest::Client::new();
        let sp = json!({
            "tenant_id": "tid",
            "client_id": "cid",
            "client_secret": "sec",
            "billing_account": "ba-1",
            "billing_profile": "bp-1",
        });
        let section = azure_section_with(&client, &sp, &mock.base_url, &mock.base_url).await;
        assert_eq!(section["status"], "arm_error");
        assert_eq!(section["detail"], "HTTP 400: ARM said no");
        let endpoint = section["endpoint"].as_str().expect("endpoint");
        assert!(
            endpoint.contains(
                "/providers/Microsoft.Billing/billingAccounts/ba-1/billingProfiles/bp-1/availableBalance?api-version=2023-05-01"
            ),
            "{endpoint}"
        );
    }

    #[tokio::test]
    async fn azure_section_auth_error_and_config_error() {
        // Token endpoint rejects -> auth_error with the upstream body.
        let mock = mock_http(vec![http_response(
            400,
            "Bad Request",
            r#"{"error":"invalid_client"}"#,
        )])
        .await;
        let client = reqwest::Client::new();
        let sp = json!({"tenant_id": "t", "client_id": "c", "client_secret": "s",
                        "subscription_id": "sub"});
        let section = azure_section_with(&client, &sp, &mock.base_url, &mock.base_url).await;
        assert_eq!(section["status"], "auth_error");
        assert!(section["detail"]
            .as_str()
            .expect("detail")
            .contains("invalid_client"));

        // Missing SP keys -> config_error listing them (no HTTP at all).
        let section = azure_section_with(
            &client,
            &json!({"tenant_id": "t"}),
            "http://127.0.0.1:1",
            "http://127.0.0.1:1",
        )
        .await;
        assert_eq!(section["status"], "config_error");
        assert_eq!(
            section["detail"],
            "Azure SP secret missing keys: ['client_id', 'client_secret']"
        );

        // Full credentials but no billing scope -> config_error after a
        // successful token exchange (no ARM request is made).
        let mock = mock_http(vec![http_response(
            200,
            "OK",
            r#"{"access_token":"armtok"}"#,
        )])
        .await;
        let sp = json!({"tenant_id": "t", "client_id": "c", "client_secret": "s"});
        let section = azure_section_with(&client, &sp, &mock.base_url, &mock.base_url).await;
        assert_eq!(section["status"], "config_error");
        assert_eq!(
            section["detail"],
            "Azure SP secret needs billing_account+billing_profile or subscription_id"
        );
        assert_eq!(mock.requests.lock().expect("requests lock").len(), 1);
    }

    #[tokio::test]
    async fn missing_secret_maps_to_no_credentials() {
        let mock = mock_http(vec![http_response(
            404,
            "Not Found",
            r#"{"error":{"code":404}}"#,
        )])
        .await;
        let client = reqwest::Client::new();
        let sp = fetch_azure_sp_with(&client, &mock.base_url, "proj", "sec", "tok")
            .await
            .expect("404 is not an error");
        assert!(sp.is_none());
        // The section the caller records for that None. It now names both
        // credential locations, so it needs the store to name the first.
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = LocalBackend::new(dir.path().to_str().expect("utf8 path")).expect("backend");
        let store = JobStorage::with_backend_and_bucket(Arc::new(backend), "local", "test-bucket");
        let section = no_credentials_section(&store);
        assert_eq!(section["status"], "no_credentials");
        let detail = section["detail"].as_str().expect("detail");
        assert!(detail.contains("not present in project"), "{detail}");
        assert!(detail.contains("stado secret store"), "{detail}");
        assert!(
            detail.contains("to activate Azure credit tracking"),
            "{detail}"
        );

        // A non-404/403 failure IS an error.
        let mock = mock_http(vec![http_response(500, "Internal Server Error", "sm down")]).await;
        let err = fetch_azure_sp_with(&client, &mock.base_url, "proj", "sec", "tok")
            .await
            .expect_err("500 propagates");
        assert!(err.contains("500"), "{err}");
        assert!(err.contains("sm down"), "{err}");
    }
}
