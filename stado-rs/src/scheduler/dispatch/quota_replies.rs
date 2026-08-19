//! Automated responder for Open Azure quota support tickets.
//!
//! Port of `stado/scheduler/dispatch/quota_replies.py`.
//!
//! Microsoft Capacity CX opens a support ticket for every Azure quota
//! increase and follows up with the same five-question template (Region /
//! Deployment Model / Service Type / Planned VM Families / Planned
//! Compute Usage in Cores). When the customer does not reply within a few
//! days, Microsoft archives the ticket and the quota request is silently
//! dropped. This module scans Open quota tickets in the configured
//! subscription and posts a single canonical reply per ticket so the
//! request progresses without manual triage.
//!
//! Uses Azure Resource Manager directly with the managed-identity or
//! `stado-azure` Skarbiec credential chain. No Azure CLI login or local token
//! cache is a credential source.
//!
//! The reply only fires when:
//!   - ticket.status == "Open"
//!   - the most recent communication is FROM Microsoft (sender domain
//!     contains "@techsupport.microsoft.com" or "@microsoft.com"),
//!     i.e. the customer has not already replied,
//!   - the ticket is a quota-classification (problemClassification
//!     contains "Quota" or "subscription limit").
//!
//! Dry-run prints the (ticket, region, planned body length) and skips
//! the create_communication call.

use std::sync::LazyLock;

use serde_json::{json, Value};

/// Python `_REGION_RE`.
fn region_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\(([^)]+)\)\s*$").expect("static regex compiles"));
    &RE
}

/// Python `_BILLING_DECLINE_RE`.
///
/// Patterns Azure Capacity CX uses when the issue is BILLING (payment
/// history, bank decline, outstanding balance), not a request for
/// customer info. Auto-replying the standard 5-answer template against a
/// billing-decline message is useless — the operator has to fix the
/// payment side before any quota can be granted. Detect and route those
/// to a skip_billing_decline action instead of replying.
fn billing_decline_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::RegexBuilder::new(
            r"insufficient payment history|bank decline|outstanding balance|unpaid invoice|payment issues|pay now to resolve|billing issue",
        )
        .case_insensitive(true)
        .build()
        .expect("static regex compiles")
    });
    &RE
}

fn html_tag_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"<[^>]+>").expect("static regex compiles"));
    &RE
}

fn ws_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\s+").expect("static regex compiles"));
    &RE
}

fn comm_name_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"[^A-Za-z0-9-]").expect("static regex compiles"));
    &RE
}

/// Python `_MS_SENDER`.
const MS_SENDER: [&str; 2] = ["techsupport.microsoft.com", "microsoft.com"];

/// az-replies error. Python raises `subprocess.CalledProcessError` on
/// non-zero exit (so a misconfigured Azure auth surfaces immediately
/// instead of producing empty results that look like 'nothing to do'),
/// `json.JSONDecodeError` on unparseable stdout, and OSError subclasses
/// when az itself cannot be spawned.
#[derive(Debug, thiserror::Error)]
pub enum RepliesError {
    /// Python `subprocess.CalledProcessError` (message matches its str()).
    #[error("Command '{cmd}' returned non-zero exit status {code}.")]
    CalledProcess {
        cmd: String,
        code: i32,
        stderr: String,
    },
    /// Python `FileNotFoundError` / `OSError` spawning az.
    #[error("{0}")]
    Spawn(String),
    /// Python `json.JSONDecodeError`.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl RepliesError {
    /// Python `exc.stderr` for the CLI's Forbidden/permission check.
    pub fn stderr(&self) -> &str {
        match self {
            RepliesError::CalledProcess { stderr, .. } => stderr,
            _ => "",
        }
    }
}

/// Injectable az CLI runner (Python `subprocess.run(["az", *args, "-o",
/// "json"], check=True, capture_output=True, text=True)`). Implementations
/// return raw stdout; non-zero exit maps to
/// [`RepliesError::CalledProcess`].
pub trait AzRunner {
    fn run(&self, args: &[&str]) -> Result<String, RepliesError>;
}

/// Production Azure Support REST runner. The synchronous trait is retained for
/// deterministic fixtures; production bridges into the existing Tokio runtime.
pub struct SystemAzRunner;

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

async fn run_azure_rest(args: &[&str]) -> Result<String, RepliesError> {
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
    let token = crate::azure_token::identity_bearer_token(
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

impl AzRunner for SystemAzRunner {
    fn run(&self, args: &[&str]) -> Result<String, RepliesError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(run_azure_rest(args))
        })
    }
}

/// Python `_az`: invoke az returning parsed JSON ([] on empty stdout).
fn az(runner: &dyn AzRunner, args: &[&str]) -> Result<Value, RepliesError> {
    let stdout = runner.run(args)?;
    if stdout.trim().is_empty() {
        return Ok(json!([]));
    }
    Ok(serde_json::from_str(&stdout)?)
}

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

/// Python `_reply_body` — the canned 5-answer template.
pub fn reply_body(subscription: &str, region: &str, contact_email: &str) -> String {
    format!(
        "Hello,\n\nThank you for following up. Please find the requested \
         information below to proceed with the GPU quota increase on \
         subscription {subscription}.\n\nRegion to Enable: {region}\n\
         Deployment Model: ARM\nService Type: Compute VM\n\n\
         Planned VM Families and Cores per family in this region:\n\
         \x20 - Standard_NC24ads_A100_v4 (NCadsA100v4): 192 cores\n\
         \x20 - Standard_ND96asr_A100_v4 (NDasrA100v4): 192 cores\n\
         \x20 - Standard_NC40ads_H100_v5 (NCadsH100v5): 200 cores\n\
         \x20 - Standard_ND96isr_H100_v5 (NDisrH100v5): 200 cores\n\n\
         Use case: wisent-compute is our GPU job orchestrator. It \
         dispatches transient (per-job, on-demand, no Spot) workloads \
         for LLM activation extraction, fine-tuning, and steered \
         inference across multiple cloud providers (GCP + this Azure \
         subscription). We need Azure GPU capacity in {region} to give \
         the autoscaler regional headroom beyond GCP's regional \
         A100/H100 limits, so a burst of queued jobs is not bottlenecked \
         on one cloud's regional ceiling. All VMs are released as soon \
         as the job completes; we do not hold capacity.\n\nPlease \
         proceed with the increase. Happy to provide any additional \
         information.\n\nRegards,\nLukasz Bartoszcze\n{contact_email}"
    )
}

/// Python `_subscription_id`.
fn subscription_id(runner: &dyn AzRunner) -> Result<String, RepliesError> {
    let r = az(runner, &["account", "show", "--query", "id"])?;
    Ok(r.as_str().unwrap_or("").to_string())
}

/// quotaId proves the subscription is sponsored (Sponsored_*).
/// az account show does NOT include subscriptionPolicies by default,
/// so hit management.azure.com via `az rest` directly.
/// Python `_subscription_quota_id`.
fn subscription_quota_id(runner: &dyn AzRunner) -> Result<String, RepliesError> {
    let sub = subscription_id(runner)?;
    if sub.is_empty() {
        return Ok(String::new());
    }
    let r = az(
        runner,
        &[
            "rest",
            "--method",
            "GET",
            "--uri",
            &format!("https://management.azure.com/subscriptions/{sub}?api-version=2022-12-01"),
            "--query",
            "subscriptionPolicies.quotaId",
        ],
    )?;
    Ok(r.as_str().unwrap_or("").to_string())
}

/// Python `_escalation_body` — the sponsored-subscription escalation.
pub fn escalation_body(subscription: &str, quota_id: &str, region: &str, email: &str) -> String {
    format!(
        "Hello,\n\nThe denial reason cited (insufficient payment history / \
         bank decline / outstanding balance) is structurally inapplicable \
         to this subscription:\n\nSubscription ID: {subscription}\n\
         Subscription quotaId: {quota_id}\n\nThis is a credit-funded \
         sponsored Azure subscription (quotaId begins with 'Sponsored_'). \
         It has no invoice/payment history to evaluate: usage is paid \
         from a Microsoft-granted credit balance, not from a customer \
         payment instrument. There is no outstanding balance (credits \
         are consumed in real time) and no prior bank decline (no bank \
         instrument is attached).\n\nPlease escalate this ticket to the \
         capacity team that handles sponsored / credit-funded \
         subscriptions, or to your manager. The quota increase for \
         {region} is needed for wisent-compute's GPU job orchestrator — \
         same use case as the prior message (LLM activation extraction + \
         fine-tuning, on-demand, no Spot, VMs released on job completion).\
         \n\nIf you cannot escalate, please indicate the correct team or \
         process and we will re-route directly.\n\nRegards,\n\
         Lukasz Bartoszcze\n{email}"
    )
}

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

