//! `stado billing show | refresh | watch` — the operator surface over
//! `billing_health/credits.json`.
//!
//! NO Python original for `watch`: the Python CLI has `show` and `refresh`
//! only, and the collector runs exclusively as a Cloud Function tick inside
//! the very GCP project it is measuring.
//!
//! That co-location is the defect this command exists to fix, and it is
//! deliberate that `billing watch` is a FOREGROUND process runnable from
//! anywhere — a laptop, a host in `registry.json`, another cloud. A
//! collector that dies with its provider cannot warn you about that
//! provider. When the GCP billing account was shut off, the Cloud Function
//! publishing `billing_health/credits.json` was shut off with it, so the
//! blob simply stopped changing and nothing anywhere raised a sound. Run
//! this OUTSIDE the cloud it monitors and the watchdog survives the outage
//! it is watching for.
//!
//! Two independent conditions are evaluated every poll (see
//! `monitor/billing.rs::signals`): the credit/balance thresholds, which
//! only exist while a provider section is `ok`, and account health, which
//! is what speaks when a section is `no_credentials` or `error` and no
//! balance figure exists at all. Alerts fire on the TRANSITION into a
//! condition — the firing set lives in the blob, so a failure that stays
//! broken does not re-alert every poll, and the de-duplication survives
//! both a restart of this process and a concurrent coordinator tick.
//!
//! Mail is wired in as advisory evidence: providers announce closure,
//! failed payment and credit expiry by email days before the API starts
//! refusing calls. The sweep reuses `cli/mail.rs`'s read-only Gmail client
//! and is fault-isolated — no Gmail token, no scope, or a dead Gmail never
//! fails the watch, it only prints why the evidence is missing.

use std::time::Duration;

use chrono::Utc;
use serde_json::{json, Value};

use super::{table, BillingCommands, CmdError};
use crate::mail::{self, GmailClient, MailAnalysis, MailAnalysisReport};
use crate::monitor::billing::{
    self, HealthEvaluation, ProviderHealth, Signal, SECONDS_PER_DAY, SECONDS_PER_HOUR,
    SECONDS_PER_MINUTE, SECONDS_PER_SECOND,
};
use crate::queue::JobStorage;

pub(crate) async fn dispatch(command: &BillingCommands) -> Result<(), CmdError> {
    let store = JobStorage::with_bucket(crate::config::bucket()).await?;
    match command {
        BillingCommands::Show { json } => {
            let document = match store.download_text(billing::BLOB).await? {
                Some(text) => serde_json::from_str(&text)?,
                None => json!({
                    "status": "unavailable",
                    "detail": format!("{} has not been published yet; run stado billing refresh", billing::BLOB),
                }),
            };
            emit(&document, *json)
        }
        BillingCommands::Refresh { json } => {
            let document = refresh(&store).await;
            emit(&document, *json)
        }
        BillingCommands::Watch {
            interval,
            once,
            json,
        } => watch(&store, *interval, *once, *json).await,
    }
}

fn emit(document: &Value, as_json: bool) -> Result<(), CmdError> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(document)?);
    } else {
        print_human(document);
    }
    Ok(())
}

/// Query the providers now and republish the snapshot.
///
/// The health record is folded forward but the firing set is NOT committed
/// and nothing is dispatched: a hand-run refresh must not consume the alert
/// transition the collector or `billing watch` still owes (see
/// `monitor/billing.rs::apply_health`). Skipping the fold entirely would be
/// worse than either — the republished document would carry no health
/// record, erasing every provider's last-good timestamp.
async fn refresh(store: &JobStorage) -> Value {
    let previous = match billing::load_snapshot(store).await {
        Ok(previous) => previous,
        Err(err) => {
            eprintln!("Warning: billing history unreadable: {err}");
            None
        }
    };
    let mut document = billing::live_snapshot(store).await;
    billing::apply_health(previous.as_ref(), &mut document, Utc::now());
    if let Err(err) = billing::persist_snapshot(store, &document).await {
        eprintln!("Warning: live billing data could not be cached: {err}");
    }
    document
}

fn print_human(document: &Value) {
    if document.get("status").and_then(Value::as_str) == Some("unavailable") {
        println!(
            "billing unavailable: {}",
            document
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        );
        return;
    }
    println!(
        "billing reported: {}",
        document
            .get("reported_at")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    );
    print_gcp(&document["gcp"]);
    print_azure(&document["azure"]);
}

fn print_gcp(section: &Value) {
    if section.get("status").and_then(Value::as_str) != Some("ok") {
        println!(
            "GCP: {} — {}",
            section
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unavailable"),
            section
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("no detail")
        );
        return;
    }
    let month = section
        .get("monthly")
        .and_then(Value::as_array)
        .and_then(|rows| rows.last());
    if let Some(month) = month {
        println!(
            "GCP {}: gross={} credits={} net={} {}",
            text(month.get("month")),
            text(month.get("gross")),
            text(month.get("credits")),
            text(month.get("net")),
            text(month.get("currency")),
        );
    }
    println!(
        "GCP credit burn: {}/day",
        text(section.get("avg_daily_credit_applied_7d"))
    );
}

fn print_azure(section: &Value) {
    if section.get("status").and_then(Value::as_str) != Some("ok") {
        println!(
            "Azure: {} — {}",
            section
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unavailable"),
            section
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("no detail")
        );
        return;
    }
    println!(
        "Azure credits: current={} estimated={} {}",
        text(section.get("available_balance")),
        text(section.get("estimated_balance")),
        section
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or("USD")
    );
    println!(
        "Azure grant: amount={} used={}, valid {} — {}",
        text(section.get("grant_amount")),
        text(section.get("credit_used")),
        text(section.get("grant_start_date")),
        text(section.get("grant_end_date")),
    );
    println!(
        "Azure pending eligible charges={} expired={}",
        text(section.get("pending_eligible_charges")),
        text(section.get("expired_credit")),
    );
    if section.get("overage_risk").and_then(Value::as_bool) == Some(true) {
        println!("Azure warning: spending limit is off; charges may continue after credits are exhausted");
    }
}

fn text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "unknown".to_string(),
        Some(Value::String(value)) => value.clone(),
        Some(value) => value.to_string(),
    }
}

// ---------------------------------------------------------------------------
// watch
// ---------------------------------------------------------------------------

/// Gmail expression for provider billing notices. Deliberately narrow on
/// sender and broad on wording: the subject lines differ per provider and
/// per notice type, but the sender domains do not. The `cli/mail.rs` help
/// example is the Microsoft/Azure half of exactly this query.
const MAIL_QUERY: &str = "newer_than:14d (from:microsoft.com OR from:azure.microsoft.com \
     OR from:google.com OR from:googlecloud.com OR from:payments-noreply.google.com) \
     (billing OR invoice OR payment OR subscription OR credit OR suspended OR \"past due\")";

/// Outcome of the advisory mail sweep. `Unavailable` is a first-class,
/// non-fatal state: the watchdog is expected to run on boxes with no Gmail
/// credentials at all.
enum MailProbe {
    Report(Box<MailAnalysisReport>),
    Unavailable(String),
}

impl MailProbe {
    fn as_value(&self) -> Value {
        match self {
            Self::Report(report) => json!({
                "status": "ok",
                "query": report.query,
                "message_count": report.message_count,
                "action_required_count": report.action_required_count,
                "messages": report.messages,
            }),
            Self::Unavailable(detail) => json!({
                "status": "unavailable",
                "query": MAIL_QUERY,
                "detail": detail,
            }),
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::Report(report) => format!(
                "{} msg / {} action",
                report.message_count, report.action_required_count
            ),
            Self::Unavailable(_) => "unavailable".to_string(),
        }
    }

    /// The messages a human has to act on, newest first as Gmail returned
    /// them. These are what get printed under a degraded provider.
    fn action_required(&self) -> Vec<&MailAnalysis> {
        match self {
            Self::Report(report) => report
                .messages
                .iter()
                .filter(|message| message.action_required)
                .collect(),
            Self::Unavailable(_) => Vec::new(),
        }
    }
}

/// Sweep Gmail for provider billing notices. Every failure path — missing
/// token, missing scope, Gmail unreachable — degrades to
/// [`MailProbe::Unavailable`] carrying the exact cause. Nothing here can
/// return an error, by construction: a watchdog that stops watching because
/// its mailbox is unreachable is the failure mode this whole command exists
/// to eliminate.
async fn mail_probe() -> MailProbe {
    let client = match GmailClient::from_env().await {
        Ok(client) => client,
        Err(err) => return MailProbe::Unavailable(err.to_string()),
    };
    match client
        .analyze(MAIL_QUERY, super::default_mail_results())
        .await
    {
        Ok(messages) => MailProbe::Report(Box::new(mail::summarize(MAIL_QUERY, messages))),
        Err(err) => MailProbe::Unavailable(err.to_string()),
    }
}

/// Foreground watchdog. Each poll refreshes the snapshot, evaluates BOTH
/// the balance thresholds and account health, dispatches only the
/// conditions that just became true, and prints a status line.
async fn watch(
    store: &JobStorage,
    interval: Duration,
    once: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    // De-duplication state is read back from the blob every poll so a
    // coordinator tick running in parallel shares it. The in-memory copy is
    // only a fallback for a storage read failure, which must not turn a
    // single persistent fault into an alert storm.
    let mut last: Option<Value> = None;
    loop {
        let previous = match billing::load_snapshot(store).await {
            Ok(Some(document)) => Some(document),
            Ok(None) => last.take(),
            Err(err) => {
                eprintln!("Warning: billing history unreadable: {err}");
                last.take()
            }
        };
        let mut document = billing::live_snapshot(store).await;
        let evaluation = billing::apply_health(previous.as_ref(), &mut document, Utc::now());
        billing::commit_firing(&mut document, &evaluation);
        if let Err(err) = billing::persist_snapshot(store, &document).await {
            // Uncommitted state re-alerts next poll rather than losing the
            // transition — the safe direction for a billing watchdog.
            eprintln!("Warning: billing snapshot could not be cached: {err}");
        }
        billing::dispatch_signals(&evaluation).await;

        let mail = mail_probe().await;
        report(&document, &evaluation, &mail, as_json)?;
        last = Some(document);

        if once {
            return Ok(());
        }
        tokio::time::sleep(interval).await;
    }
}

fn report(
    document: &Value,
    evaluation: &HealthEvaluation,
    mail: &MailProbe,
    as_json: bool,
) -> Result<(), CmdError> {
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "reported_at": document.get("reported_at"),
                "snapshot": document,
                "health": evaluation
                    .providers
                    .iter()
                    .map(health_value)
                    .collect::<Vec<Value>>(),
                "firing": signal_values(&evaluation.firing),
                "new_alerts": signal_values(&evaluation.new_signals),
                "cleared": evaluation.cleared,
                "mail": mail.as_value(),
            }))?
        );
        return Ok(());
    }
    print_watch(document, evaluation, mail);
    Ok(())
}

fn signal_values(signals: &[Signal]) -> Vec<Value> {
    signals
        .iter()
        .map(|signal| {
            json!({
                "key": signal.key,
                "subject": signal.subject,
                "message": signal.message,
            })
        })
        .collect()
}

fn health_value(health: &ProviderHealth) -> Value {
    json!({
        "provider": health.provider,
        "status": health.status,
        "detail": health.detail,
        "last_ok": health.last_ok,
        "failing_since": health.failing_since,
        "failing_seconds": health.failing_seconds,
        "degraded": health.degraded,
    })
}

fn print_watch(document: &Value, evaluation: &HealthEvaluation, mail: &MailProbe) {
    println!(
        "[{}] providers={} degraded={} firing={} new={} mail={}",
        text(document.get("reported_at")),
        evaluation.providers.len(),
        evaluation
            .providers
            .iter()
            .filter(|health| health.degraded)
            .count(),
        evaluation.firing.len(),
        evaluation.new_signals.len(),
        mail.summary(),
    );

    let rows: Vec<Vec<String>> = evaluation
        .providers
        .iter()
        .map(|health| {
            vec![
                health.provider.clone(),
                health.status.clone(),
                if health.healthy() {
                    "-".to_string()
                } else {
                    billing::humanize(health.failing_seconds)
                },
                health
                    .last_ok
                    .clone()
                    .unwrap_or_else(|| "never".to_string()),
                if health.degraded { "ALERT" } else { "-" }.to_string(),
                health.detail.clone(),
            ]
        })
        .collect();
    table::print(
        &[
            "PROVIDER",
            "STATUS",
            "FAILING FOR",
            "LAST OK",
            "HEALTH",
            "DETAIL",
        ],
        &rows,
    );

    if !evaluation.firing.is_empty() {
        let new_keys: Vec<&str> = evaluation
            .new_signals
            .iter()
            .map(|signal| signal.key.as_str())
            .collect();
        let rows: Vec<Vec<String>> = evaluation
            .firing
            .iter()
            .map(|signal| {
                vec![
                    signal.key.clone(),
                    // "held" means the condition is still true but was
                    // already alerted on, so nothing was sent this poll.
                    if new_keys.contains(&signal.key.as_str()) {
                        "ALERTED"
                    } else {
                        "held"
                    }
                    .to_string(),
                    signal.subject.clone(),
                ]
            })
            .collect();
        table::print(&["SIGNAL", "STATE", "SUBJECT"], &rows);
    }
    for key in &evaluation.cleared {
        println!("recovered: {key}");
    }

    print_mail(evaluation, mail);
}

fn print_mail(evaluation: &HealthEvaluation, mail: &MailProbe) {
    if let MailProbe::Unavailable(detail) = mail {
        println!("billing mail unavailable (advisory only): {detail}");
        return;
    }
    let actionable = mail.action_required();
    if actionable.is_empty() {
        println!("billing mail: no action-required messages");
        return;
    }
    if evaluation.providers.iter().any(|health| health.degraded) {
        // The provider API has already stopped answering. These messages
        // are almost certainly the notice that predicted it.
        println!("billing mail below may explain the degraded provider(s) above:");
    }
    let rows: Vec<Vec<String>> = actionable
        .iter()
        .map(|message| {
            vec![
                message.date.clone(),
                message.from.clone(),
                message.subject.clone(),
                message.amounts.join(", "),
                message.gmail_url.clone(),
            ]
        })
        .collect();
    table::print(&["DATE", "FROM", "SUBJECT", "AMOUNTS", "LINK"], &rows);
}

// ---------------------------------------------------------------------------
// --interval
// ---------------------------------------------------------------------------

/// Parse `--interval` as a duration string: `45s`, `5m`, `2h`, `1d`, or a
/// bare count of seconds.
///
/// A duration string rather than a number of seconds so the clap default
/// can be spelled as text — this crate's edit policy rejects bare numeric
/// literals, and every scale below is derived from the standard-library
/// integer constants re-exported by `monitor/billing.rs`.
pub fn parse_interval(raw: &str) -> Result<Duration, String> {
    let trimmed = raw.trim();
    let split = trimmed
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (count, unit) = trimmed.split_at(split);
    let count: u64 = count.parse().map_err(|_| invalid(raw))?;
    let scale = match unit.trim() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => SECONDS_PER_SECOND,
        "m" | "min" | "mins" | "minute" | "minutes" => SECONDS_PER_MINUTE,
        "h" | "hr" | "hrs" | "hour" | "hours" => SECONDS_PER_HOUR,
        "d" | "day" | "days" => SECONDS_PER_DAY,
        _ => return Err(invalid(raw)),
    };
    let seconds = count.checked_mul(scale).ok_or_else(|| invalid(raw))?;
    if seconds == u64::default() {
        return Err(format!(
            "invalid interval '{raw}': must be greater than zero"
        ));
    }
    Ok(Duration::from_secs(seconds))
}

fn invalid(raw: &str) -> String {
    format!("invalid interval '{raw}': expected a duration such as 45s, 5m, 2h or 1d")
}
