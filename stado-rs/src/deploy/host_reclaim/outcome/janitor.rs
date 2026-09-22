//! The janitor own account of one pass, in one line an operator can act on.
//!
//! This is the only stage that reads the host declared cleaner policy, and it
//! used to report no items with no detail - so a pass that decided there was no
//! pressure, a pass whose declared roots hold nothing eligible, and a pass that
//! deleted nothing under a report-only policy all looked identical. That cost
//! an afternoon on one host with 29 GiB free against a 24 GiB watermark, a
//! declared model cache of 3.2 GiB beside it, and a receipt that said nothing.

use serde_json::Value;

pub(super) fn janitor_sentence(
    plan: &Value,
    plans: &[crate::deploy::host_state::cleanup::CleanerPlan],
    apply: bool,
) -> String {
    let word = |key: &str| {
        plan.get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let outcome = word("outcome").unwrap_or("no outcome reported");
    let mut sentence = format!("host janitor: {outcome}");
    if let Some(mode) = word("mode") {
        sentence.push_str(&format!(", policy mode {mode}"));
    }
    if plans.is_empty() {
        sentence.push_str(", and it declares no cleaners");
        return sentence;
    }
    let mut cleaners: Vec<String> = plans
        .iter()
        .map(|cleaner| {
            let acted = if apply {
                cleaner.deleted_items
            } else {
                cleaner.eligible_items
            };
            format!(
                "{} scanned {} eligible {} {} {}",
                cleaner.name,
                cleaner.scanned_items,
                cleaner.eligible_items,
                if apply { "deleted" } else { "would delete" },
                acted
            )
        })
        .collect();
    cleaners.sort();
    sentence.push_str(&format!("; {}", cleaners.join("; ")));
    sentence
}
