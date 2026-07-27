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
//! Uses az CLI subprocess against the box's existing Azure auth instead
//! of adding an Azure support SDK — Azure responses are an operator-side
//! task (the mac-mini coordinator or a workstation has az + Azure auth
//! already; the Cloud Function does not, and should not, hold Azure
//! creds).
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
    CalledProcess { cmd: String, code: i32, stderr: String },
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

/// The real az CLI on PATH.
pub struct SystemAzRunner;

/// Python list repr for the CalledProcessError message.
fn py_list(args: &[&str]) -> String {
    let inner: Vec<String> = args.iter().map(|a| format!("'{a}'")).collect();
    format!("[{}]", inner.join(", "))
}

impl AzRunner for SystemAzRunner {
    fn run(&self, args: &[&str]) -> Result<String, RepliesError> {
        let full: Vec<&str> = std::iter::once("az")
            .chain(args.iter().copied())
            .chain(["-o", "json"])
            .collect();
        let output = std::process::Command::new("az")
            .args(args)
            .args(["-o", "json"])
            .output()
            .map_err(|err| RepliesError::Spawn(err.to_string()))?;
        if !output.status.success() {
            return Err(RepliesError::CalledProcess {
                cmd: py_list(&full),
                code: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
    let Some(comms) = comms.as_object() else { return Ok(json!({})) };
    let body = comms.get("body").and_then(Value::as_str).unwrap_or("");
    let no_html = html_tag_re().replace_all(body, "");
    let snippet: String =
        ws_re().replace_all(&no_html, " ").trim().chars().take(240).collect();
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
    let sender = last.get("sender").and_then(Value::as_str).unwrap_or("").to_lowercase();
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
    let Some(rows) = rows.as_array() else { return Ok(vec![]) };
    Ok(rows
        .iter()
        .filter(|r| {
            let problem =
                r.get("problem").and_then(Value::as_str).unwrap_or("").to_lowercase();
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
        let sender = last.get("sender").and_then(Value::as_str).unwrap_or("").to_lowercase();
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
            &format!(
                "https://management.azure.com/subscriptions/{sub}?api-version=2022-12-01"
            ),
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
    let quota_id = if escalate_billing { subscription_quota_id(runner)? } else { String::new() };
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
        let snippet = ticket.get("last_body_snippet").and_then(Value::as_str).unwrap_or("");
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Scripted az: routes canned responses by argv shape and records
    /// every invocation (assert az CLI construction without spawning az).
    #[derive(Default)]
    struct FakeAz {
        calls: Mutex<Vec<Vec<String>>>,
        tickets: Value,
        comms: std::collections::HashMap<String, Value>,
        subscription: String,
        quota_id: String,
        fail_create_with: Option<(i32, String)>,
    }

    impl FakeAz {
        fn new(tickets: Value, comms: std::collections::HashMap<String, Value>) -> Self {
            FakeAz {
                calls: Mutex::new(vec![]),
                tickets,
                comms,
                subscription: "sub-123".into(),
                quota_id: "Sponsored_2017".into(),
                fail_create_with: None,
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }

        fn creates(&self) -> Vec<Vec<String>> {
            self.calls()
                .into_iter()
                .filter(|c| c.len() > 3 && c[3] == "create")
                .collect()
        }
    }

    impl AzRunner for FakeAz {
        fn run(&self, args: &[&str]) -> Result<String, RepliesError> {
            self.calls.lock().unwrap().push(args.iter().map(|s| s.to_string()).collect());
            match args {
                ["account", "show", "--query", "id"] => {
                    Ok(json!(self.subscription).to_string())
                }
                ["rest", ..] => Ok(json!(self.quota_id).to_string()),
                ["support", "in-subscription", "tickets", "list", ..] => {
                    Ok(self.tickets.to_string())
                }
                ["support", "in-subscription", "communication", "list", _, name, _, _] => {
                    Ok(self.comms.get(*name).cloned().unwrap_or(json!([])).to_string())
                }
                ["support", "in-subscription", "communication", "create", ..] => {
                    if let Some((code, stderr)) = &self.fail_create_with {
                        return Err(RepliesError::CalledProcess {
                            cmd: "['az', ...]".into(),
                            code: *code,
                            stderr: stderr.clone(),
                        });
                    }
                    Ok(String::new())
                }
                other => panic!("unstubbed az invocation: {other:?}"),
            }
        }
    }

    fn ticket(name: &str, title: &str, problem: &str) -> Value {
        json!({"name": name, "title": title, "problem": problem})
    }

    fn comm(sender: &str, body: &str) -> Value {
        json!({
            "sender": sender, "createdDate": "2026-06-01T00:00:00Z",
            "subject": "Your quota request", "body": body,
        })
    }

    #[test]
    fn region_from_title_parses_trailing_parens() {
        assert_eq!(region_from_title("GPU quota increase (eastus)"), "eastus");
        assert_eq!(region_from_title("GPU quota (westus3)  "), "westus3");
        assert_eq!(region_from_title("no region here"), "");
    }

    #[test]
    fn reply_and_escalation_bodies_carry_the_canonical_text() {
        let body = reply_body("sub-1", "eastus", "op@wisent.ai");
        assert!(body.contains("Region to Enable: eastus"));
        assert!(body.contains("subscription sub-1"));
        assert!(body.contains("Standard_NC40ads_H100_v5 (NCadsH100v5): 200 cores"));
        assert!(body.contains("Deployment Model: ARM\nService Type: Compute VM"));
        assert!(body.ends_with("Lukasz Bartoszcze\nop@wisent.ai"));

        let esc = escalation_body("sub-1", "Sponsored_2017", "westus3", "op@wisent.ai");
        assert!(esc.contains("Subscription quotaId: Sponsored_2017"));
        assert!(esc.contains("credit-funded sponsored Azure subscription"));
        assert!(esc.contains("The quota increase for westus3 is needed"));
        assert!(esc.ends_with("Lukasz Bartoszcze\nop@wisent.ai"));
    }

    #[test]
    fn list_open_azure_tickets_filters_and_joins_latest_communication() {
        let tickets = json!([
            ticket("t1", "GPU quota across NC/ND/NV families (eastus)", "Compute quota increase"),
            ticket("t2", "billing question", "Billing issue"),
            ticket("t3", "Limit raise (westus3)", "Subscription limit increase"),
        ]);
        let comms = std::collections::HashMap::from([
            ("t1".to_string(), comm("support@techsupport.microsoft.com", "<p>Please answer</p>")),
            ("t3".to_string(), comm("op@wisent.ai", "my reply")),
        ]);
        let az = FakeAz::new(tickets, comms);
        let rows = list_open_azure_tickets(&az).unwrap();
        // The billing-classification ticket is filtered out.
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0]["name"], json!("t1"));
        assert_eq!(rows[0]["region"], json!("eastus"));
        assert_eq!(rows[0]["awaiting_customer"], json!(true));
        assert_eq!(rows[0]["last_body_snippet"], json!("Please answer"));
        assert_eq!(rows[1]["awaiting_customer"], json!(false));
    }

    #[test]
    fn respond_posts_canonical_reply_only_when_microsoft_waits() {
        let tickets = json!([
            ticket("t1", "GPU quota (eastus)", "Compute quota increase"),
            ticket("t2", "GPU quota (westus3)", "Compute quota increase"),
        ]);
        let comms = std::collections::HashMap::from([
            ("t1".to_string(), comm("cx@microsoft.com", "Please provide the following")),
            ("t2".to_string(), comm("op@wisent.ai", "already answered")),
        ]);
        let az = FakeAz::new(tickets, comms);
        let rows = respond_to_open_quota_tickets(&az, "op@wisent.ai", false, false).unwrap();
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0]["action"], json!("replied"));
        assert_eq!(rows[1]["action"], json!("skip_customer_already_replied"));

        let creates = az.creates();
        assert_eq!(creates.len(), 1, "{creates:?}");
        let args = &creates[0];
        // az support in-subscription communication create ...
        assert_eq!(&args[..4], ["support", "in-subscription", "communication", "create"]);
        let get = |flag: &str| {
            args.iter().position(|a| a == flag).map(|i| args[i + 1].clone()).unwrap()
        };
        assert_eq!(get("--ticket-name"), "t1");
        assert!(get("--communication-name").starts_with("wc-quota-reply-eastus-"));
        assert_eq!(get("--communication-subject"), "RE: GPU quota across NC/ND/NV families (eastus)");
        let body = get("--communication-body");
        assert!(body.contains("Region to Enable: eastus"));
        assert!(body.contains("subscription sub-123"));
        assert!(args.last().unwrap() == "--no-wait");
    }

    #[test]
    fn respond_dry_run_posts_nothing_but_reports_would_and_body_chars() {
        let tickets = json!([ticket("t1", "GPU quota (eastus)", "Compute quota increase")]);
        let comms = std::collections::HashMap::from([(
            "t1".to_string(),
            comm("cx@microsoft.com", "Please provide"),
        )]);
        let az = FakeAz::new(tickets, comms);
        let rows = respond_to_open_quota_tickets(&az, "op@wisent.ai", true, false).unwrap();
        assert_eq!(
            rows,
            vec![json!({
                "name": "t1", "region": "eastus", "ok": true,
                "action": "dry_run", "would": "replied",
                "body_chars": reply_body("sub-123", "eastus", "op@wisent.ai").chars().count(),
            })]
        );
        assert!(az.creates().is_empty());
    }

    #[test]
    fn billing_decline_skips_without_escalate_and_escalates_with() {
        let tickets = json!([ticket("t9", "GPU quota (northeurope)", "Compute quota increase")]);
        let comms = std::collections::HashMap::from([(
            "t9".to_string(),
            comm("cx@microsoft.com", "denied: insufficient payment history on the account"),
        )]);

        // Default: routed to skip_billing_decline, no reply posted.
        let az = FakeAz::new(tickets.clone(), comms.clone());
        let rows = respond_to_open_quota_tickets(&az, "op@wisent.ai", false, false).unwrap();
        assert_eq!(rows[0]["action"], json!("skip_billing_decline"));
        assert!(az.creates().is_empty());

        // escalate_billing: the sponsored-subscription escalation fires
        // (and the quotaId was fetched via az rest).
        let az = FakeAz::new(tickets, comms);
        let rows = respond_to_open_quota_tickets(&az, "op@wisent.ai", false, true).unwrap();
        assert_eq!(rows[0]["action"], json!("escalated"));
        let creates = az.creates();
        assert_eq!(creates.len(), 1);
        let args = &creates[0];
        let get = |flag: &str| {
            args.iter().position(|a| a == flag).map(|i| args[i + 1].clone()).unwrap()
        };
        assert!(get("--communication-name").starts_with("wc-quota-escalate-northeurope-"));
        assert!(get("--communication-subject").contains("sponsored subscription"));
        assert!(get("--communication-body").contains("Sponsored_2017"));
        assert!(az.calls().iter().any(|c| c.first().map(String::as_str) == Some("rest")));
    }

    #[test]
    fn missing_region_and_create_failure_rows() {
        let tickets = json!([
            ticket("t1", "no region in this title", "Compute quota increase"),
            ticket("t2", "GPU quota (eastus)", "Compute quota increase"),
        ]);
        let comms = std::collections::HashMap::from([
            ("t1".to_string(), comm("cx@microsoft.com", "info please")),
            ("t2".to_string(), comm("cx@microsoft.com", "info please")),
        ]);
        let mut az = FakeAz::new(tickets, comms);
        az.fail_create_with = Some((1, "Forbidden: missing permission".repeat(20)));
        let rows = respond_to_open_quota_tickets(&az, "op@wisent.ai", false, false).unwrap();
        assert_eq!(rows[0]["action"], json!("skip_no_region_in_title"));
        assert_eq!(rows[0]["ok"], json!(false));
        assert_eq!(rows[1]["action"], json!("error"));
        assert_eq!(rows[1]["ok"], json!(false));
        // stderr is truncated to 240 chars.
        assert_eq!(rows[1]["error"].as_str().unwrap().chars().count(), 240);
    }
}
