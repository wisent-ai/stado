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
//! authenticated with a service-principal stored in Secret Manager. A
//! missing secret is an explicit no_credentials status so Azure tracking
//! activates automatically the moment the secret is provisioned, with zero
//! code change.
//!
//! Transport deviations from Python (same on-the-wire data):
//! - Python uses the google-cloud-bigquery library; here the queries run
//!   over the BigQuery REST `queries` endpoint, so rows are parsed from the
//!   REST `rows[].f[].v` shape (all values string-typed) instead of the
//!   library's typed Row objects. The three SQL strings are byte-identical.
//! - Python reads the Azure SP via the Secret Manager SDK; here it is the
//!   REST `versions/latest:access` endpoint (same pattern as
//!   `providers/vast.rs::fetch_secret_manager_key`).
//! - Python's `ClientSecretCredential` is the literal OAuth2 client-
//!   credentials POST to login.microsoftonline.com.
//! - Python records exception detail as `{type(e).__name__}: {e}`; Rust has
//!   no exception class names, so the detail is the error's Display text
//!   (the exact upstream error is preserved either way).

use std::sync::LazyLock;

use chrono::{SecondsFormat, Utc};
use regex::Regex;
use serde_json::{json, Value};

use crate::config;
use crate::queue::JobStorage;

/// Blob written every tick (Python `_BLOB`).
pub const BLOB: &str = "billing_health/credits.json";

/// BigQuery dataset/table identifiers cannot be bound as query parameters.
/// They originate from controlled config, but we still hard-validate the
/// shape so an env override can never inject SQL (Python `_IDENT_RE`).
static IDENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_]+$").expect("static regex compiles"));

const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const BIGQUERY_BASE: &str = "https://bigquery.googleapis.com";
const SECRET_MANAGER_BASE: &str = "https://secretmanager.googleapis.com";
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
    let auth = gcp_auth::provider().await.map_err(|e| e.to_string())?;
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
// Azure section — Secret Manager SP + ARM available balance
// ---------------------------------------------------------------------------

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

fn no_credentials_section() -> Value {
    json!({
        "status": "no_credentials",
        "detail": format!(
            "Secret Manager secret '{}' not present in project {}; create it (JSON: tenant_id, \
             client_id, client_secret, billing_account+billing_profile or subscription_id) to \
             activate Azure credit tracking",
            config::azure_billing_secret(),
            config::project()
        ),
    })
}

/// Available credit balance via ARM. Every failure path records the EXACT
/// cause (missing secret, auth failure, ARM HTTP body) so the status is
/// actionable without reading logs (Python `_azure_section`).
async fn azure_section() -> Value {
    let token = match gcp_token().await {
        Ok(token) => token,
        Err(err) => return error_section(err),
    };
    let client = reqwest::Client::new();
    let sp = match fetch_azure_sp_with(
        &client,
        SECRET_MANAGER_BASE,
        config::project(),
        config::azure_billing_secret(),
        &token,
    )
    .await
    {
        Ok(sp) => sp,
        Err(err) => return error_section(err),
    };
    match sp {
        None => no_credentials_section(),
        Some(sp) => azure_section_with(&client, &sp, AZURE_LOGIN_BASE, ARM_BASE).await,
    }
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
        .filter(|v| !v.is_empty());
    let billing_profile = sp
        .get("billing_profile")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty());
    let subscription = sp
        .get("subscription_id")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty());
    let url = if let (Some(ba), Some(bp)) = (billing_account, billing_profile) {
        format!(
            "{arm_base}/providers/Microsoft.Billing/billingAccounts/{ba}/billingProfiles/{bp}/availableBalance?api-version=2023-05-01"
        )
    } else if let Some(sub) = subscription {
        format!(
            "{arm_base}/subscriptions/{sub}/providers/Microsoft.Consumption/balances?api-version=2019-10-01"
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
        // Python: f"HTTP {e.code}: {e.read()[:400].decode('utf-8', 'replace')}"
        let truncated: String = body_text.chars().take(400).collect();
        return json!({
            "status": "arm_error",
            "detail": format!("HTTP {}: {truncated}", status.as_u16()),
            "endpoint": url,
        });
    }
    let body: Value = match serde_json::from_str(&body_text) {
        Ok(body) => body,
        Err(err) => return error_section(err.to_string()),
    };

    let props = body.get("properties").unwrap_or(&body).clone();
    let amount = extract_amount(&props);
    json!({"status": "ok", "available_balance": amount, "raw": props})
}

/// Python: `amt = props.get("amount") or props.get("availableBalance")`;
/// a dict amount resolves through its `"value"` key.
fn extract_amount(props: &Value) -> Value {
    let Some(map) = props.as_object() else {
        return Value::Null;
    };
    let amount = map
        .get("amount")
        .filter(|v| !v.is_null())
        .or_else(|| map.get("availableBalance"));
    match amount {
        Some(Value::Object(obj)) => obj.get("value").cloned().unwrap_or(Value::Null),
        Some(value) => value.clone(),
        None => Value::Null,
    }
}

// ---------------------------------------------------------------------------
// collect_billing
// ---------------------------------------------------------------------------

/// Assemble and upload billing_health/credits.json. Each source is isolated:
/// a failure from one is captured into its section as the exact error
/// string, so the blob is always written and the other source is never lost.
/// Emits a [tick] BILLING ALERT log line on credit depletion so existing
/// log-based alerting fires with no extra wiring (Python `collect_billing`).
pub async fn collect_billing(store: &JobStorage) {
    let gcp = gcp_section().await;
    let azure = azure_section().await;
    write_billing_blob(store, gcp, azure).await;
}

/// Upload + alert half of [`collect_billing`], split out so tests can inject
/// sections without any live source.
pub async fn write_billing_blob(store: &JobStorage, gcp: Value, azure: Value) {
    let doc = json!({
        "reported_at": Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false),
        "project": config::project(),
        "gcp": gcp,
        "azure": azure,
    });
    // Python: json.dumps(doc, indent=2, default=str).
    let pretty = serde_json::to_string_pretty(&doc).expect("json! macro output serializes");
    if let Err(err) = store.upload_text(BLOB, &pretty).await {
        // Python lets the upload exception propagate; the no-Result Rust
        // signature logs instead. Nothing else is logged on this path there.
        log(&format!("billing upload failed: {err}"));
        return;
    }

    if gcp
        .get("credit_depleted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        log(&format!(
            "BILLING ALERT: GCP latest-month net ${} exceeds ${} — promotion credit exhausted or rate-capped",
            py_value(gcp.get("latest_month_net_usd")),
            py_value(gcp.get("net_alert_threshold_usd")),
        ));
    }
    let threshold = config::billing_net_alert_usd();
    let balance = azure.get("available_balance").and_then(Value::as_f64);
    if azure.get("status").and_then(Value::as_str) == Some("ok") {
        if let Some(balance) = balance {
            if balance < threshold {
                log(&format!(
                    "BILLING ALERT: Azure available credit balance {} below {}",
                    py_f64(balance),
                    py_f64(threshold),
                ));
            }
        }
    }
    log(&format!(
        "billing: gcp={} azure={} -> {BLOB}",
        py_value(gcp.get("status")),
        py_value(azure.get("status")),
    ));
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
        // The section the caller records for that None.
        let section = no_credentials_section();
        assert_eq!(section["status"], "no_credentials");
        let detail = section["detail"].as_str().expect("detail");
        assert!(detail.contains("not present in project"), "{detail}");
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
