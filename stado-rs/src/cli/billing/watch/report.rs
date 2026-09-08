//! What one poll emits. Under `--json` the whole evaluation is serialised —
//! snapshot, per-provider health, the firing set, the transitions that just
//! fired, what cleared, and the mail evidence — because that document is
//! what a supervising process parses. Otherwise `render` prints tables.

use serde_json::{json, Value};

use super::mail::MailProbe;
use super::render::print_watch;
use crate::cli::CmdError;
use crate::monitor::billing::{HealthEvaluation, ProviderHealth, Signal};

pub(super) fn report(
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
