//! The GCP collector: everything the billing document reads out of the
//! BigQuery billing export, plus the credential and REST plumbing that read
//! needs.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Value};

use super::format::{error_section, py_repr};
use crate::config;

/// BigQuery dataset/table identifiers cannot be bound as query parameters.
/// They originate from controlled config, but we still hard-validate the
/// shape so an env override can never inject SQL (Python `_IDENT_RE`).
static IDENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_]+$").expect("static regex compiles"));

const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const BIGQUERY_BASE: &str = "https://bigquery.googleapis.com";

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
pub(super) async fn gcp_section() -> Value {
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
