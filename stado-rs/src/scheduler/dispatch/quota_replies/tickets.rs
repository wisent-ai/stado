//! Read side: the latest communication on a ticket, the
//! Microsoft-sender gate, the Open quota-classification filter, the
//! region parsed out of a ticket title, and the joined enumerator the
//! CLI renders.

use serde_json::{json, Value};

use super::patterns::{html_tag_re, region_re, ws_re, MS_SENDER};
use super::runner::{az, AzRunner, RepliesError};

/// Return the latest communication on a ticket as a plain dict {sender,
/// createdDate, subject, body_snippet}. Empty dict if none. Python
/// `_last_communication`.
fn last_communication(runner: &dyn AzRunner, ticket_name: &str) -> Result<Value, RepliesError> {
    let comms = az(
        runner,
        &[
            "support",
            "in-subscription",
            "communication",
            "list",
            "--ticket-name",
            ticket_name,
            "--query",
            "[0]",
        ],
    )?;
    let Some(comms) = comms.as_object() else {
        return Ok(json!({}));
    };
    let body = comms.get("body").and_then(Value::as_str).unwrap_or("");
    let no_html = html_tag_re().replace_all(body, "");
    let snippet: String = ws_re()
        .replace_all(&no_html, " ")
        .trim()
        .chars()
        .take(240)
        .collect();
    Ok(json!({
        "sender": comms.get("sender").and_then(Value::as_str).unwrap_or(""),
        "createdDate": comms.get("createdDate").and_then(Value::as_str).unwrap_or(""),
        "subject": comms.get("subject").and_then(Value::as_str).unwrap_or(""),
        "body_snippet": snippet,
    }))
}

/// Python `_last_communication_is_from_ms`.
pub fn last_communication_is_from_ms(
    runner: &dyn AzRunner,
    ticket_name: &str,
) -> Result<bool, RepliesError> {
    let last = last_communication(runner, ticket_name)?;
    let sender = last
        .get("sender")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    Ok(MS_SENDER.iter().any(|dom| sender.contains(dom)))
}

/// Python `_open_quota_tickets`.
fn open_quota_tickets(runner: &dyn AzRunner) -> Result<Vec<Value>, RepliesError> {
    let rows = az(
        runner,
        &[
            "support",
            "in-subscription",
            "tickets",
            "list",
            "--query",
            "[?status=='Open'].{name:name, title:title, \
             problem:problemClassificationDisplayName}",
        ],
    )?;
    let Some(rows) = rows.as_array() else {
        return Ok(vec![]);
    };
    Ok(rows
        .iter()
        .filter(|r| {
            let problem = r
                .get("problem")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            problem.contains("quota") || problem.contains("subscription limit")
        })
        .cloned()
        .collect())
}

/// Python `_region_from_title`.
pub fn region_from_title(title: &str) -> String {
    region_re()
        .captures(title)
        .map(|caps| caps[1].trim().to_string())
        .unwrap_or_default()
}

/// Reusable enumerator: one row per Open quota-classification Azure
/// support ticket, joined with the latest communication's sender / sent /
/// subject / body_snippet. Python `list_open_azure_tickets`.
pub fn list_open_azure_tickets(runner: &dyn AzRunner) -> Result<Vec<Value>, RepliesError> {
    let mut out = Vec::new();
    for ticket in open_quota_tickets(runner)? {
        let name = ticket.get("name").and_then(Value::as_str).unwrap_or("");
        let title = ticket.get("title").and_then(Value::as_str).unwrap_or("");
        let last = last_communication(runner, name)?;
        let sender = last
            .get("sender")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let awaiting = MS_SENDER.iter().any(|dom| sender.contains(dom));
        out.push(json!({
            "name": name,
            "title": title,
            "region": region_from_title(title),
            "last_sender": last.get("sender").and_then(Value::as_str).unwrap_or(""),
            "last_sent": last.get("createdDate").and_then(Value::as_str).unwrap_or(""),
            "last_subject": last.get("subject").and_then(Value::as_str).unwrap_or(""),
            "last_body_snippet": last.get("body_snippet").and_then(Value::as_str).unwrap_or(""),
            "awaiting_customer": awaiting,
        }));
    }
    Ok(out)
}
