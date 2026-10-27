//! Announce a refusal once, on the run where it changed.

use std::collections::BTreeMap;

use crate::monitor::alerts;

use super::report::{ServiceReconcileOutcome, ServiceReconcileReport};

pub(in crate::autonomy::service_reconciler) async fn alert_transitions(
    previous: Option<&ServiceReconcileReport>,
    report: &ServiceReconcileReport,
) {
    // The first run has no prior outcomes, so every refusal it finds is new.
    let prior: BTreeMap<String, &ServiceReconcileOutcome> = match previous {
        Some(previous) => previous
            .outcomes
            .iter()
            .map(|outcome| (outcome.key(), outcome))
            .collect(),
        None => BTreeMap::new(),
    };
    for outcome in report
        .outcomes
        .iter()
        .filter(|outcome| outcome.needs_alert())
    {
        let repeated = prior.get(&outcome.key()).is_some_and(|old| {
            old.classification == outcome.classification && old.detail == outcome.detail
        });
        if repeated {
            continue;
        }
        let message = format!(
            "Stado service reconciliation {} on {}: {}",
            outcome.service, outcome.host, outcome.detail
        );
        alerts::send_alert(
            crate::config::alerts_topic(),
            &message,
            "Stado could not reconcile a declared service",
        )
        .await;
    }
}
