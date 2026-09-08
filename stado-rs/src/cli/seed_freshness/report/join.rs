//! The join itself: the host's evidence document read into attempts, and the
//! vault sweep and those attempts turned into one report document.

use serde_json::{json, Value};

use crate::cli::seed_freshness::verdict::classify::classify;
use crate::cli::seed_freshness::verdict::inputs::{Attempt, SEED_UNREADABLE};
use crate::cli::seed_freshness::verdict::outcome::Verdict;

/// Parse one host-side evidence document into attempts, keyed by login item.
pub fn attempts_of(evidence: &Value, login_item: &str) -> Vec<Attempt> {
    evidence
        .get("attempts")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter(|row| row.get("login_item").and_then(Value::as_str) == Some(login_item))
                .map(|row| {
                    let flag = |name: &str| row.get(name).and_then(Value::as_bool).unwrap_or(false);
                    Attempt {
                        at: row
                            .get("at")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        at_ms: row.get("at_ms").and_then(Value::as_i64).unwrap_or(0),
                        result: row
                            .get("result")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        code_submitted: flag("code_submitted"),
                        code_rejected: flag("code_rejected"),
                        locked_out: flag("locked_out"),
                        authenticator_unreached: flag("authenticator_unreached"),
                        markers: row
                            .get("markers")
                            .and_then(Value::as_array)
                            .map(|names| {
                                names
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Join the vault sweep and the attempt evidence into one report.
pub fn build_report(target: &str, vault: &Value, evidence: &Value) -> Value {
    let rows = vault
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut findings = Vec::new();
    for row in &rows {
        let item = row
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let seed_state = row
            .get("seed_state")
            .and_then(Value::as_str)
            .unwrap_or(SEED_UNREADABLE);
        let attempts = attempts_of(evidence, &item);
        let verdict = classify(seed_state, &attempts);
        // A row with no seed field and no sign-in history is not evidence of
        // anything an operator has to act on today, and reporting every such
        // row would bury the two that matter.
        if matches!(verdict, Verdict::FieldAbsent) && attempts.is_empty() {
            continue;
        }
        let mut markers: Vec<String> = attempts
            .iter()
            .flat_map(|attempt| attempt.markers.clone())
            .collect();
        markers.sort();
        markers.dedup();
        let repair = verdict.repair(&item);
        let repair = if repair.is_empty() {
            Value::Null
        } else {
            json!(repair)
        };
        findings.push(json!({
            "login_item": item,
            "kind": row.get("kind").cloned().unwrap_or(Value::Null),
            "seed_state": seed_state,
            "verdict": verdict.code(),
            "needs_reenrolment": verdict.needs_reenrolment(),
            "attempts_recorded": attempts.len(),
            "code_submitting_attempts": attempts
                .iter()
                .filter(|attempt| attempt.code_submitted)
                .count(),
            "rejected_since": match &verdict {
                Verdict::RejectedSince { since, .. } => json!(since),
                _ => Value::Null,
            },
            "last_known_good_at": match &verdict {
                Verdict::LastKnownGood { at } => json!(at),
                _ => Value::Null,
            },
            "locked_out": matches!(&verdict, Verdict::RejectedSince { locked_out: true, .. }),
            "markers": markers,
            "repair": repair,
        }));
    }
    findings.sort_by_key(|finding| {
        // Rows needing a repair first: this is read by somebody deciding what
        // to do, not browsing an inventory.
        let urgent = finding
            .get("needs_reenrolment")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        (
            !urgent,
            finding
                .get("login_item")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        )
    });
    json!({
        "schema_version":
            1,
        "target": target,
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "evidence": {
            "journal": evidence.get("journal").cloned().unwrap_or(Value::Null),
            "reauth_runs_seen": evidence.get("reauth_runs_seen").cloned().unwrap_or(Value::Null),
        },
        "login_rows_read": rows.len(),
        "findings": findings,
    })
}
