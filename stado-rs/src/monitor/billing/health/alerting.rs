//! What the tick says out loud: the per-section log lines the snapshot
//! justifies, and the fan-out of newly-firing signals to the alert channels.

use serde_json::Value;

use super::{humanize, HealthEvaluation, HEALTH_KEY};
use crate::config;
use crate::monitor::alerts::send_alert;
use crate::monitor::billing::format::{log, py_f64, py_value};
use crate::monitor::billing::BLOB;

pub(in crate::monitor::billing) fn emit_alerts(document: &Value) {
    if let Some(capability) = crate::capabilities::get("billing") {
        for variant in capability.variants {
            let section = &document[variant.id];
            match variant.adapter {
                crate::capabilities::RuntimeAdapter::Billing(
                    crate::capabilities::BillingAdapter::Gcp,
                ) if section
                    .get("credit_depleted")
                    .and_then(Value::as_bool)
                    .unwrap_or(false) =>
                {
                    log(&format!(
                        "BILLING ALERT: GCP latest-month net ${} exceeds ${} — promotion credit exhausted or rate-capped",
                        py_value(section.get("latest_month_net_usd")),
                        py_value(section.get("net_alert_threshold_usd")),
                    ));
                }
                crate::capabilities::RuntimeAdapter::Billing(
                    crate::capabilities::BillingAdapter::Azure,
                ) => {
                    let threshold = config::billing_net_alert_usd();
                    if let Some(balance) = section.get("available_balance").and_then(Value::as_f64)
                    {
                        if balance < threshold {
                            log(&format!(
                                "BILLING ALERT: Azure available credit balance {} below {}",
                                py_f64(balance),
                                py_f64(threshold),
                            ));
                        }
                    }
                    if section.get("overage_risk").and_then(Value::as_bool) == Some(true) {
                        log(
                            "BILLING WARNING: Azure spending limit is off; paid overage can continue after credits",
                        );
                    }
                }
                _ => {}
            }
        }
    }
    let provider_health = document[HEALTH_KEY]
        .get("providers")
        .and_then(Value::as_object);
    if let Some(providers) = provider_health {
        for (provider, health) in providers {
            if health.get("degraded").and_then(Value::as_bool) == Some(true) {
                log(&format!(
                    "BILLING ALERT: {provider} account unhealthy — status {} for {}, last good report {}",
                    py_value(health.get("status")),
                    humanize(health.get("failing_seconds").and_then(Value::as_i64).unwrap_or_default()),
                    py_value(health.get("last_ok")),
                ));
            }
        }
    }
    log(&format!(
        "billing: gcp={} azure={} -> {BLOB}",
        py_value(
            provider_health
                .and_then(|providers| providers.get("gcp"))
                .and_then(|health| health.get("status"))
        ),
        py_value(
            provider_health
                .and_then(|providers| providers.get("azure"))
                .and_then(|health| health.get("status"))
        ),
    ));
}

/// Fan every newly-firing signal out through
/// [`crate::monitor::alerts::send_alert`], which fault-isolates each
/// channel. Recovery is logged, not alerted: an outage that ends does not
/// need to wake anyone.
pub async fn dispatch_signals(evaluation: &HealthEvaluation) {
    for signal in &evaluation.new_signals {
        log(&format!("ALERT {}: {}", signal.key, signal.message));
        send_alert(config::alerts_topic(), &signal.message, &signal.subject).await;
    }
    for key in &evaluation.cleared {
        log(&format!("RECOVERED {key}"));
    }
}
