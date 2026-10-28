//! The GCP collector: everything the billing document reads out of the
//! BigQuery billing export, plus the credential and REST plumbing that read
//! needs.

mod rows;

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use self::rows::{bq_num, bq_str, named_rows};
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
        .map_err(|error| format!("the GCP credential for BigQuery cannot be read: {error}"))?;
    let token = auth
        .token(&[CLOUD_PLATFORM_SCOPE])
        .await
        .map_err(|error| format!("a BigQuery access token cannot be minted: {error}"))?;
    Ok(token.as_str().to_string())
}

/// Spend/credits/burn from the BigQuery billing export. Fails into a section
/// error only on a genuine client/permission fault; the caller records that
/// as the section's error so one broken source never suppresses the other
/// (Python `_gcp_section`).
pub(super) async fn gcp_section() -> Value {
    let dataset = config::billing_dataset();
    let table = config::billing_table();
    if dataset.is_empty() || table.is_empty() {
        return json!({
            "status": "config_error",
            "detail": "the BigQuery billing export is not declared: set WC_BILLING_DATASET and WC_BILLING_TABLE",
        });
    }
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
    if config::project().is_empty() {
        return json!({
            "status": "config_error",
            "detail": "the GCP project the billing export lives in is not declared: set GCP_PROJECT",
        });
    }
    let token = match gcp_token().await {
        Ok(token) => token,
        Err(err) => return error_section(err),
    };
    let client = reqwest::Client::new();
    match gcp_section_with(&client, BIGQUERY_BASE, config::project(), &token).await {
        Ok(section) => section,
        Err(err) => error_section(err),
    }
}

/// One BigQuery REST answer, as JSON, or the refusal with the upstream body.
async fn answer(request: reqwest::RequestBuilder, what: &str) -> Result<Value, String> {
    let response = crate::wait::request(request)
        .await
        .map_err(|error| format!("BigQuery {what} could not be sent: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = crate::primitives::http::refused_body(response).await;
        return Err(format!("BigQuery {what} answered HTTP {status}: {body}"));
    }
    response.json().await.map_err(|error| {
        format!("BigQuery {what} answered {status} with a body that is not JSON: {error}")
    })
}

/// Run one query to its end and answer every row, by column name. A job still
/// running when `jobs.query` answers comes back `jobComplete: false` with no
/// rows; read as empty, an unfinished billing scan was a month that billed
/// nothing. It is asked again through `jobs.getQueryResults` (BigQuery holds
/// each call until the job ends or its window closes) until complete, and
/// every page of a split result is read.
async fn run_bq_query(
    client: &reqwest::Client,
    base_url: &str,
    project: &str,
    token: &str,
    sql: &str,
) -> Result<Vec<Map<String, Value>>, String> {
    let queries = format!("{base_url}/bigquery/v2/projects/{project}/queries");
    let mut body = answer(
        client
            .post(&queries)
            .bearer_auth(token)
            .json(&json!({"query": sql, "useLegacySql": false})),
        "jobs.query",
    )
    .await?;
    let mut rows = Vec::new();
    loop {
        let complete = match body.get("jobComplete") {
            Some(Value::Bool(complete)) => *complete,
            other => {
                return Err(format!(
                    "BigQuery answered jobComplete {other:?}, not true or false"
                ))
            }
        };
        if complete {
            rows.extend(named_rows(&body)?);
        }
        let page = body.get("pageToken").and_then(Value::as_str);
        if complete && page.is_none() {
            return Ok(rows);
        }
        let reference = &body["jobReference"];
        let Some(job) = reference.get("jobId").and_then(Value::as_str) else {
            return Err(format!(
                "BigQuery left the billing query unfinished without a job to ask again: {reference}"
            ));
        };
        let mut query = Vec::new();
        if let Some(location) = reference.get("location").and_then(Value::as_str) {
            query.push(("location", location.to_string()));
        }
        if let (true, Some(page)) = (complete, page) {
            query.push(("pageToken", page.to_string()));
        }
        body = answer(
            client
                .get(format!("{queries}/{job}"))
                .bearer_auth(token)
                .query(&query),
            &format!("jobs.getQueryResults for {job}"),
        )
        .await?;
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

/// The section from one base URL and token, or the first query's refusal.
async fn gcp_section_with(
    client: &reqwest::Client,
    base_url: &str,
    project: &str,
    token: &str,
) -> Result<Value, String> {
    let (monthly_sql, credit_sql, burn_sql) = billing_sql(project);
    let monthly_rows = run_bq_query(client, base_url, project, token, &monthly_sql).await?;
    let credit_rows = run_bq_query(client, base_url, project, token, &credit_sql).await?;
    let burn_rows = run_bq_query(client, base_url, project, token, &burn_sql).await?;

    let monthly: Vec<Value> = monthly_rows
        .iter()
        .map(|row| {
            Ok(json!({
                "month": bq_str(row, "month")?,
                "gross": bq_num(row, "gross")?,
                "credits": bq_num(row, "credits")?,
                "net": bq_num(row, "net")?,
                "currency": bq_str(row, "currency")?,
            }))
        })
        .collect::<Result<_, String>>()?;
    let credits: Vec<Value> = credit_rows
        .iter()
        .map(|row| {
            Ok(json!({
                "name": bq_str(row, "name")?,
                "type": bq_str(row, "type")?,
                "cumulative": bq_num(row, "cumulative")?,
                "currency": bq_str(row, "currency")?,
            }))
        })
        .collect::<Result<_, String>>()?;
    // AVG over no day is SQL NULL; a burn query that answered no row at all
    // has no average either.
    let burn = match burn_rows.first() {
        Some(row) => bq_num(row, "avg_daily_credit_7d")?,
        None => Value::Null,
    };
    let latest_net = match monthly.last() {
        Some(month) => month["net"].clone(),
        None => Value::Null,
    };
    let (threshold, detail, depleted) = match config::billing_net_alert_usd() {
        Ok(threshold) => (
            json!(threshold),
            Value::Null,
            json!(latest_net.as_f64().is_some_and(|net| net > threshold)),
        ),
        Err(detail) => (Value::Null, json!(detail), Value::Null),
    };

    Ok(json!({
        "status": "ok",
        "monthly": monthly,
        "credits": credits,
        "avg_daily_credit_applied_7d": burn,
        "latest_month_net_usd": latest_net,
        "net_alert_threshold_usd": threshold,
        "net_alert_detail": detail,
        "credit_depleted": depleted,
    }))
}
