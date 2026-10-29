//! What a subscription has actually cost since its credit grant began, read
//! from Cost Management, so a grant whose balance Azure does not report can
//! still be read as the grant less what it has paid for.
//!
//! The credits balance call answers 204 with no body for a sponsorship
//! grant; the billing property states the grant (its amount, currency and
//! start), and Cost Management's query states the actual cost over any
//! period (<https://learn.microsoft.com/en-us/rest/api/cost-management/query/usage>).

use serde_json::{json, Value};

/// The Cost Management API version this reader speaks.
const API_VERSION: &str = "2023-03-01";

/// The balance of the grant `grant` (a `billingProfileSpendingLimitDetails`
/// entry) of `subscription`, when Azure reports none: its amount less the
/// actual cost since its start, in the currency the grant or the cost
/// names, with both sources as the billing section publishes them.
pub(super) async fn balance(
    client: &reqwest::Client,
    arm_base: &str,
    token: &str,
    grant: Option<&Value>,
    subscription: Option<&str>,
) -> Result<(f64, String, Value), String> {
    let field = |name: &str| grant.and_then(|detail| detail.get(name));
    let (Some(granted), Some(start), Some(subscription)) = (
        field("amount").and_then(Value::as_f64),
        field("startDate").and_then(Value::as_str),
        subscription,
    ) else {
        return Err(
            "the billing property names no grant amount and start, or no subscription_id is configured, \
             to take actual cost from"
                .to_string(),
        );
    };
    let named = field("currency").and_then(Value::as_str);
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (cost, stated) = since(client, arm_base, token, subscription, start, &now, named).await?;
    let stated = stated.ok_or_else(|| {
        format!(
            "the grant names no currency and Cost Management stated none for its cost of {cost}"
        )
    })?;
    let source = json!({
        "grant": granted,
        "actual_cost_since_grant": cost,
        "from": start,
        "to": now,
        "cost_scope": format!("subscription {subscription}"),
    });
    Ok((granted - cost, stated, source))
}

/// The actual cost of `subscription` from `start` to `now`, and the currency
/// it is stated in: `expected` when the grant names one, else the one every
/// row names. Rows in another currency are refused, since they cannot be
/// taken from the grant; no rows is no cost, in the currency the grant names.
pub(super) async fn since(
    client: &reqwest::Client,
    arm_base: &str,
    token: &str,
    subscription: &str,
    start: &str,
    now: &str,
    expected: Option<&str>,
) -> Result<(f64, Option<String>), String> {
    let url = format!(
        "{arm_base}/subscriptions/{subscription}/providers/Microsoft.CostManagement/query?api-version={API_VERSION}"
    );
    let body = json!({
        "type": "ActualCost",
        "timeframe": "Custom",
        "timePeriod": {"from": start, "to": now},
        "dataset": {
            "granularity": "None",
            "aggregation": {"totalCost": {"name": "Cost", "function": "Sum"}},
        },
    });
    // Cost Management throttles its query API and names, on every 429, how
    // long the caller is to wait in its `x-ms-ratelimit-*-retry-after`
    // headers (or `Retry-After`); that wait is held and the query asked
    // again. A 429 that names no wait is refused with its body.
    let response = loop {
        let response = crate::wait::request(client.post(&url).bearer_auth(token).json(&body))
            .await
            .map_err(|error| format!("the Cost Management query {url} did not answer: {error}"))?;
        if response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
            break response;
        }
        let Some(wait) = stated_retry_seconds(response.headers()) else {
            let text = crate::primitives::http::refused_body(response).await;
            return Err(format!(
                "the Cost Management query {url} answered HTTP 429 and named no retry wait: {text}"
            ));
        };
        let waiting = crate::wait::begin(
            crate::wait::Kind::Network,
            format!("the {wait}s Cost Management stated before the next query"),
            &url,
        );
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        waiting.done();
    };
    let status = response.status();
    if !status.is_success() {
        let text = crate::primitives::http::refused_body(response).await;
        return Err(format!(
            "the Cost Management query {url} answered HTTP {}: {text}",
            status.as_u16()
        ));
    }
    let answer: Value = response.json().await.map_err(|error| {
        format!(
            "the Cost Management query {url} answered no JSON: {}",
            error.without_url()
        )
    })?;
    let properties = answer.get("properties").unwrap_or(&answer);
    let column = |name: &str| {
        properties["columns"]
            .as_array()
            .and_then(|columns| columns.iter().position(|column| column["name"] == name))
    };
    let (Some(cost), Some(named)) = (column("Cost"), column("Currency")) else {
        return Err(format!(
            "the Cost Management query {url} names no Cost and Currency columns: {properties}"
        ));
    };
    let mut total = 0.0;
    let mut stated = expected.map(str::to_string);
    for row in properties["rows"].as_array().into_iter().flatten() {
        let (Some(amount), Some(row_currency)) = (row[cost].as_f64(), row[named].as_str()) else {
            return Err(format!(
                "the Cost Management query {url} answered a row without a cost and currency: {row}"
            ));
        };
        match &stated {
            Some(currency) if !row_currency.eq_ignore_ascii_case(currency) => {
                return Err(format!(
                    "the Cost Management query {url} states cost in {row_currency}, and the grant's cost is in {currency}"
                ))
            }
            Some(_) => {}
            None => stated = Some(row_currency.to_string()),
        }
        total += amount;
    }
    Ok((total, stated))
}

/// The longest wait, in whole seconds, a throttled Cost Management answer
/// names: its `x-ms-ratelimit-microsoft.costmanagement-*-retry-after`
/// headers (one per quota it counts against) and `Retry-After`
/// (<https://learn.microsoft.com/en-us/azure/cost-management-billing/costs/manage-automation#error-code-429---call-count-has-exceeded-rate-limits>).
/// `None` when it names none in seconds.
fn stated_retry_seconds(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            name == "retry-after"
                || (name.starts_with("x-ms-ratelimit-microsoft.costmanagement-")
                    && name.ends_with("-retry-after"))
        })
        .filter_map(|(_, value)| value.to_str().ok()?.trim().parse::<u64>().ok())
        .max()
}
