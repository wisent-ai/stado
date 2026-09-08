//! Turning one billing document plus its folded health into the set of
//! conditions that are true right now.

use serde_json::Value;

use super::{humanize, ProviderHealth, Signal, HEALTH_GRACE_SECONDS};
use crate::config;
use crate::monitor::billing::format::{py_f64, py_value};

/// Every alert condition true for this document.
///
/// Account health comes FIRST and is computed from the section status
/// alone, never from a balance field. That is the whole point: a section
/// that is not `ok` carries no `credit_depleted` and no `available_balance`
/// — both live inside the success branch — so the three balance conditions
/// below are structurally incapable of firing for a provider whose account
/// or credentials just died. The health signal is what speaks then.
pub(super) fn signals(document: &Value, providers: &[ProviderHealth]) -> Vec<Signal> {
    let mut firing = Vec::new();
    for health in providers.iter().filter(|health| health.degraded) {
        let cause = if health.detail.is_empty() {
            "no detail reported by the provider"
        } else {
            health.detail.as_str()
        };
        firing.push(Signal {
            key: format!("account_health:{}", health.provider),
            subject: format!("stado billing: {} account unhealthy", health.provider),
            message: format!(
                "BILLING ACCOUNT HEALTH: the {} billing section has reported '{}' since {} \
                 ({} and counting), past the {} grace period. Last good report: {}. \
                 Cause: {}. While this persists {} publishes no balance at all, so the \
                 credit-threshold alert CANNOT fire — treat this as the outage warning.",
                health.provider,
                health.status,
                health.failing_since.as_deref().unwrap_or("an unknown time"),
                humanize(health.failing_seconds),
                humanize(HEALTH_GRACE_SECONDS),
                health.last_ok.as_deref().unwrap_or("never"),
                cause,
                health.provider,
            ),
        });
    }

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
                    firing.push(Signal {
                        key: format!("credit_depleted:{}", variant.id),
                        subject: "stado billing: GCP promotion credit exhausted".to_string(),
                        message: format!(
                            "BILLING ALERT: GCP latest-month net ${} exceeds ${} — promotion credit exhausted or rate-capped",
                            py_value(section.get("latest_month_net_usd")),
                            py_value(section.get("net_alert_threshold_usd")),
                        ),
                    });
                }
                crate::capabilities::RuntimeAdapter::Billing(
                    crate::capabilities::BillingAdapter::Azure,
                ) => {
                    let threshold = config::billing_net_alert_usd();
                    if let Some(balance) = section.get("available_balance").and_then(Value::as_f64)
                    {
                        if balance < threshold {
                            firing.push(Signal {
                                key: format!("balance_low:{}", variant.id),
                                subject: "stado billing: Azure credit balance low".to_string(),
                                message: format!(
                                    "BILLING ALERT: Azure available credit balance {} below {}",
                                    py_f64(balance),
                                    py_f64(threshold),
                                ),
                            });
                        }
                    }
                    if section.get("overage_risk").and_then(Value::as_bool) == Some(true) {
                        firing.push(Signal {
                            key: format!("overage_risk:{}", variant.id),
                            subject: "stado billing: Azure spending limit is off".to_string(),
                            message: "BILLING WARNING: Azure spending limit is off; paid overage can continue after credits".to_string(),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    firing
}
