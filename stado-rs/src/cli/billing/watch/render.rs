//! The human tables for one poll: a status line, per-provider health, the
//! firing signal set with whether this poll actually sent anything, what
//! recovered, and the action-required mail underneath it.

use serde_json::Value;

use super::mail::MailProbe;
use crate::cli::billing::format::text;
use crate::cli::table;
use crate::monitor::billing::{self, HealthEvaluation};

pub(super) fn print_watch(document: &Value, evaluation: &HealthEvaluation, mail: &MailProbe) {
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
