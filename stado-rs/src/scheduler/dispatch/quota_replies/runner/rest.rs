//! Azure Support REST plumbing behind the production runner: the argv
//! flag reader, the bearer-auth request/response step, and the router
//! that maps az argv onto Microsoft.Support endpoints.

use serde_json::{json, Value};

use super::RepliesError;

/// The value after `flag` in the az argv this runner was handed, or a
/// refusal naming the flag: read as "", a missing ticket name would ask Azure
/// for `supportTickets//communications` and a missing body would post an
/// empty reply. As az reads it, the flag takes the argument after it.
fn arg_value<'a>(args: &'a [&str], flag: &str) -> Result<&'a str, RepliesError> {
    let mut remaining = args.iter();
    while let Some(argument) = remaining.next() {
        if *argument == flag {
            if let Some(value) = remaining.next() {
                return Ok(value);
            }
        }
    }
    Err(RepliesError::Config(format!(
        "the az argv {args:?} carries no value for {flag}, so the Azure Support request cannot be made"
    )))
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
    let response = crate::wait::request(request)
        .await
        .map_err(RepliesError::Transport)?;
    let status = response.status();
    let text = response.text().await.map_err(RepliesError::Transport)?;
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
        return Err(RepliesError::Config(
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
    .map_err(|err| RepliesError::Auth(err.to_string()))?;
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
        // `az rest --query` prints null for a path the answer does not carry,
        // and the caller refuses that null naming the missing field.
        return Ok(
            match value.pointer("/properties/subscriptionPolicies/quotaId") {
                Some(quota) => quota.to_string(),
                None => Value::Null.to_string(),
            },
        );
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
        let rows: Vec<Value> = listing(&value, "the support ticket listing")?
            .iter()
            .filter_map(|row| {
                let properties = row.get("properties")?;
                if properties.get("status").and_then(Value::as_str) != Some("Open") {
                    return None;
                }
                Some(json!({
                    // As Azure sent them: a field it left out is null, as
                    // `az` prints it, not "" read downstream as stated.
                    "name": row.get("name"),
                    "title": properties.get("title"),
                    "serviceId": properties.get("serviceId"),
                }))
            })
            .collect();
        return Ok(json!(rows).to_string());
    }
    if matches!(
        args,
        ["support", "in-subscription", "communication", "list", ..]
    ) {
        let ticket = arg_value(args, "--ticket-name")?;
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
        // The latest communication is the one Azure created last; one that
        // carries no creation date cannot be placed among them, so it is
        // refused rather than sorted first. Of equal dates the last listed
        // is taken.
        let mut latest: Option<(&str, &Value)> = None;
        for row in listing(&value, &format!("the communications of ticket {ticket}"))? {
            let Some(created) = row
                .pointer("/properties/createdDate")
                .and_then(Value::as_str)
            else {
                return Err(RepliesError::Config(format!(
                    "a communication of ticket {ticket} carries no createdDate, so the latest \
                     one cannot be told: {row}"
                )));
            };
            let newer = match latest {
                Some((newest, _)) => created >= newest,
                None => true,
            };
            if newer {
                latest = Some((created, row));
            }
        }
        // A ticket with no communication answers as `az --query [0]` reads
        // to the caller: an empty object, which it reads as no message yet.
        return Ok(match latest {
            Some((_, row)) => row["properties"].to_string(),
            None => json!({}).to_string(),
        });
    }
    if matches!(
        args,
        ["support", "in-subscription", "communication", "create", ..]
    ) {
        let ticket = arg_value(args, "--ticket-name")?;
        let name = arg_value(args, "--communication-name")?;
        let subject = arg_value(args, "--communication-subject")?;
        let body = arg_value(args, "--communication-body")?;
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
    Err(RepliesError::Config(format!(
        "unsupported Azure Support operation: {}",
        args.join(" ")
    )))
}

/// The rows of one Azure Support listing. An answer without its `value`
/// array was read as no open tickets, or no messages on a ticket, and the
/// responder then skipped a ticket Microsoft was waiting on.
fn listing<'a>(value: &'a Value, what: &str) -> Result<&'a Vec<Value>, RepliesError> {
    value.get("value").and_then(Value::as_array).ok_or_else(|| {
        RepliesError::Config(format!(
            "Azure Support answered {what} without a `value` list: {value}"
        ))
    })
}
