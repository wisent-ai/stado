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
    // No message at all is an empty object; a message is only read with its
    // sender, because the sender decides whether Microsoft is waiting on us.
    // A message without one was read as sender "" and the ticket skipped as
    // already answered.
    let Some(comms) = comms.as_object().filter(|comms| !comms.is_empty()) else {
        return Ok(json!({}));
    };
    let Some(sender) = comms.get("sender").and_then(Value::as_str) else {
        return Err(RepliesError::Config(format!(
            "the latest message on ticket {ticket_name} names no sender, so whether Microsoft awaits a reply cannot be told: {}",
            Value::Object(comms.clone())
        )));
    };
    let snippet = comms.get("body").and_then(Value::as_str).map(|body| {
        let no_html = html_tag_re().replace_all(body, "");
        ws_re().replace_all(&no_html, " ").trim().to_string()
    });
    Ok(json!({
        "sender": sender,
        "createdDate": comms.get("createdDate").and_then(Value::as_str),
        "subject": comms.get("subject").and_then(Value::as_str),
        "body_snippet": snippet,
    }))
}

/// Whether the latest message came from Microsoft; a ticket with no message
/// yet is not awaiting us.
fn from_microsoft(last: &Value) -> bool {
    match last.get("sender").and_then(Value::as_str) {
        Some(sender) => {
            let sender = sender.to_lowercase();
            MS_SENDER.iter().any(|dom| sender.contains(dom))
        }
        None => false,
    }
}

/// Python `_last_communication_is_from_ms`.
pub fn last_communication_is_from_ms(
    runner: &dyn AzRunner,
    ticket_name: &str,
) -> Result<bool, RepliesError> {
    Ok(from_microsoft(&last_communication(runner, ticket_name)?))
}

/// Azure Support's service "Service and subscription limits (quotas)": a
/// ticket belongs to quota triage when its `serviceId` names this service.
const QUOTA_SERVICE_NAME: &str = "06bfd9d3-516b-d5c6-5802-169c800dec89";

/// Open tickets filed under the quota service, read from the ticket's
/// `serviceId` field rather than words in its classification's display name.
fn open_quota_tickets(runner: &dyn AzRunner) -> Result<Vec<Value>, RepliesError> {
    let rows = az(
        runner,
        &[
            "support",
            "in-subscription",
            "tickets",
            "list",
            "--query",
            "[?status=='Open'].{name:name, title:title, serviceId:serviceId}",
        ],
    )?;
    let Some(rows) = rows.as_array() else {
        return Err(RepliesError::Config(format!(
            "Azure answered the open ticket listing with something other than a list: {rows}"
        )));
    };
    Ok(rows
        .iter()
        .filter(|r| {
            r.get("serviceId")
                .and_then(Value::as_str)
                .and_then(|id| id.trim_end_matches('/').rsplit('/').next())
                .is_some_and(|service| service.eq_ignore_ascii_case(QUOTA_SERVICE_NAME))
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
        // A ticket is replied to by name; one listed without a name was asked
        // about as ticket "" and could not be answered.
        let Some(name) = ticket.get("name").and_then(Value::as_str) else {
            return Err(RepliesError::Config(format!(
                "Azure listed an open quota ticket without a name: {ticket}"
            )));
        };
        let title = ticket.get("title").and_then(Value::as_str);
        let last = last_communication(runner, name)?;
        out.push(json!({
            "name": name,
            "title": title,
            "region": title.map(region_from_title),
            "last_sender": last.get("sender"),
            "last_sent": last.get("createdDate"),
            "last_subject": last.get("subject"),
            "last_body_snippet": last.get("body_snippet"),
            "awaiting_customer": from_microsoft(&last),
        }));
    }
    Ok(out)
}
