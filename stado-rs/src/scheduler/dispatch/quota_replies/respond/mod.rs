//! Write side: the two published reply bodies, the subscription
//! identity the escalation cites, and the per-ticket responder that
//! picks between them.

mod subscription;
mod templates;

use serde_json::{json, Value};

use super::patterns::{billing_decline_re, comm_name_re};
use super::runner::{AzRunner, RepliesError};
use super::tickets::list_open_azure_tickets;

use self::subscription::{subscription_id, subscription_quota_id};

/// The two bodies the responder renders. `reply_body` and
/// `escalation_body` were published by the pre-image at
/// `dispatch::quota_replies::` and stay nameable there; this is also the
/// binding the responder below calls them through.
pub use self::templates::{escalation_body, reply_body};

/// Scan Open quota tickets and post a reply per ticket whose last
/// message is from Microsoft. Python `respond_to_open_quota_tickets`.
///
/// Two reply templates:
///   - default (escalate_billing=False): the 5-answer info template.
///     Billing-decline tickets get action=skip_billing_decline (no reply
///     posted; standard template wouldn't help — fix the billing side).
///   - escalate_billing=True: billing-decline tickets get the
///     credit-funded-subscription escalation message instead; other
///     tickets still get the standard info reply.
///
/// Actions: replied / escalated / dry_run / skip_billing_decline /
/// skip_customer_already_replied / skip_no_region_in_title / error.
pub fn respond_to_open_quota_tickets(
    runner: &dyn AzRunner,
    contact_email: &str,
    dry_run: bool,
    escalate_billing: bool,
) -> Result<Vec<Value>, RepliesError> {
    let subscription = subscription_id(runner)?;
    let quota_id = if escalate_billing {
        subscription_quota_id(runner)?
    } else {
        String::new()
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut out = Vec::new();
    for ticket in list_open_azure_tickets(runner)? {
        let name = ticket.get("name").and_then(Value::as_str).unwrap_or("");
        let region = ticket.get("region").and_then(Value::as_str).unwrap_or("");
        if region.is_empty() {
            out.push(json!({
                "name": name, "ok": false,
                "action": "skip_no_region_in_title",
                "title": ticket.get("title").and_then(Value::as_str).unwrap_or(""),
            }));
            continue;
        }
        if ticket.get("awaiting_customer").and_then(Value::as_bool) != Some(true) {
            out.push(json!({
                "name": name, "region": region, "ok": true,
                "action": "skip_customer_already_replied",
            }));
            continue;
        }
        let snippet = ticket
            .get("last_body_snippet")
            .and_then(Value::as_str)
            .unwrap_or("");
        let is_billing = billing_decline_re().is_match(snippet);
        if is_billing && !escalate_billing {
            out.push(json!({
                "name": name, "region": region, "ok": true,
                "action": "skip_billing_decline",
                "last_body_snippet": snippet,
            }));
            continue;
        }
        let (body, subject, action_label, prefix) = if is_billing {
            (
                escalation_body(&subscription, &quota_id, region, contact_email),
                format!(
                    "RE: GPU quota across NC/ND/NV families ({region}) — escalation: sponsored subscription"
                ),
                "escalated",
                "wc-quota-escalate-",
            )
        } else {
            (
                reply_body(&subscription, region, contact_email),
                format!("RE: GPU quota across NC/ND/NV families ({region})"),
                "replied",
                "wc-quota-reply-",
            )
        };
        if dry_run {
            out.push(json!({
                "name": name, "region": region, "ok": true,
                "action": "dry_run", "would": action_label,
                "body_chars": body.chars().count(),
            }));
            continue;
        }
        let comm_name = format!(
            "{prefix}{}",
            comm_name_re().replace_all(&format!("{region}-{ts}"), "-")
        );
        match runner.run(&[
            "support",
            "in-subscription",
            "communication",
            "create",
            "--ticket-name",
            name,
            "--communication-name",
            &comm_name,
            "--communication-subject",
            &subject,
            "--communication-body",
            &body,
            "--no-wait",
        ]) {
            Ok(_) => out.push(json!({
                "name": name, "region": region, "ok": true,
                "action": action_label,
            })),
            Err(err) => out.push(json!({
                "name": name, "region": region, "ok": false,
                "action": "error",
                "error": if err.stderr().is_empty() {
                    err.to_string().chars().take(240).collect::<String>()
                } else {
                    err.stderr().chars().take(240).collect::<String>()
                },
            })),
        }
    }
    Ok(out)
}
