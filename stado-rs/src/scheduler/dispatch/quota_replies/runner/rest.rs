//! Azure Support REST plumbing behind the production runner: the argv
//! flag reader, the bearer-auth request/response step, and the router
//! that maps az argv onto Microsoft.Support endpoints.

use serde_json::{json, Value};

use super::RepliesError;

fn arg_value<'a>(args: &'a [&str], flag: &str) -> Option<&'a str> {
    args.windows(
        std::iter::once(())
            .count()
            .saturating_add(std::iter::once(()).count()),
    )
    .find(|pair| pair.first() == Some(&flag))
    .and_then(|pair| pair.get(std::iter::once(()).count()).copied())
}

async fn azure_response(
    http: &reqwest::Client,
    token: &str,
    method: reqwest::Method,
    url: String,
    body: Option<Value>,
) -> Result<Value, RepliesError> {
    let mut request = http.request(method, url).bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|err| RepliesError::Spawn(err.to_string()))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|err| RepliesError::Spawn(err.to_string()))?;
    if !status.is_success() {
        return Err(RepliesError::CalledProcess {
            cmd: "Azure Support REST".into(),
            code: i32::from(status.as_u16()),
            stderr: text,
        });
    }
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(RepliesError::Json)
}

pub(super) async fn run_azure_rest(args: &[&str]) -> Result<String, RepliesError> {
    let subscription = crate::config::azure_subscription_id();
    if subscription.is_empty() {
        return Err(RepliesError::Spawn(
            "AZURE_SUBSCRIPTION_ID is required".into(),
        ));
    }
    if matches!(args, ["account", "show", ..]) {
        return Ok(json!(subscription).to_string());
    }
    let http = reqwest::Client::new();
    let token = crate::remote::azure_token::identity_bearer_token(
        &http,
        "https://management.azure.com/.default",
        "https://management.azure.com",
    )
    .await
    .map_err(|err| RepliesError::Spawn(err.to_string()))?;
    let base = format!("https://management.azure.com/subscriptions/{subscription}");

    if matches!(args, ["rest", ..]) {
        let value = azure_response(
            &http,
            &token,
            reqwest::Method::GET,
            format!("{base}?api-version=2022-12-01"),
            None,
        )
        .await?;
        return Ok(value
            .pointer("/properties/subscriptionPolicies/quotaId")
            .cloned()
            .unwrap_or(Value::Null)
            .to_string());
    }
    if matches!(args, ["support", "in-subscription", "tickets", "list", ..]) {
        let value = azure_response(
            &http,
            &token,
            reqwest::Method::GET,
            format!("{base}/providers/Microsoft.Support/supportTickets?api-version=2024-04-01"),
            None,
        )
        .await?;
        let rows: Vec<Value> = value
            .get("value")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| {
                let properties = row.get("properties")?;
                if properties.get("status").and_then(Value::as_str) != Some("Open") {
                    return None;
                }
                Some(json!({
                    "name": row.get("name").and_then(Value::as_str).unwrap_or(""),
                    "title": properties.get("title").and_then(Value::as_str).unwrap_or(""),
                    "problem": properties
                        .get("problemClassificationDisplayName")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                }))
            })
            .collect();
        return Ok(json!(rows).to_string());
    }
    if matches!(
        args,
        ["support", "in-subscription", "communication", "list", ..]
    ) {
        let ticket = arg_value(args, "--ticket-name").unwrap_or_default();
        let value = azure_response(
            &http,
            &token,
            reqwest::Method::GET,
            format!(
                "{base}/providers/Microsoft.Support/supportTickets/{ticket}/communications?api-version=2024-04-01"
            ),
            None,
        )
        .await?;
        let latest = value
            .get("value")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .max_by(|left, right| {
                let left_created = left
                    .pointer("/properties/createdDate")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let right_created = right
                    .pointer("/properties/createdDate")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                left_created.cmp(right_created)
            })
            .and_then(|row| row.get("properties"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        return Ok(latest.to_string());
    }
    if matches!(
        args,
        ["support", "in-subscription", "communication", "create", ..]
    ) {
        let ticket = arg_value(args, "--ticket-name").unwrap_or_default();
        let name = arg_value(args, "--communication-name").unwrap_or_default();
        let subject = arg_value(args, "--communication-subject").unwrap_or_default();
        let body = arg_value(args, "--communication-body").unwrap_or_default();
        let value = azure_response(
            &http,
            &token,
            reqwest::Method::PUT,
            format!(
                "{base}/providers/Microsoft.Support/supportTickets/{ticket}/communications/{name}?api-version=2024-04-01"
            ),
            Some(json!({
                "properties": {
                    "communicationType": "web",
                    "subject": subject,
                    "body": body,
                }
            })),
        )
        .await?;
        return Ok(value.to_string());
    }
    Err(RepliesError::Spawn(format!(
        "unsupported Azure Support operation: {}",
        args.join(" ")
    )))
}
