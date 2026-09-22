//! Exercise actual commands and inspect persisted outcomes, not just their output.
mod support;
use serde_json::{json, Value};
use support::{document, option, Journey};

#[test]
fn catalog_replacement_refuses_stale_versions_and_invalid_money_without_writing() {
    let j = Journey::new();
    let created = j.set(vec![option("mac", 1000.0, 20.0, 200.0)], None);
    assert!(created.status.success());
    let first = document(&created);
    let path = j.store.join("state/fleet/expansion/catalog.json");
    let before = std::fs::read(&path).unwrap();
    let conflict = j.set(Vec::new(), None);
    assert!(!conflict.status.success());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let mut invalid = option("invalid", 1.001, 0.0, 20.0);
    let rejected = j.set(vec![invalid.clone()], first["version"].as_str());
    assert!(!rejected.status.success());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    invalid["upfront_usd"] = Value::Null;
    let replaced = j.set(vec![invalid], first["version"].as_str());
    assert!(replaced.status.success());
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(saved["options"][0]["upfront_usd"].is_null());
    assert!(!j
        .set(Vec::new(), first["version"].as_str())
        .status
        .success());
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(),
        saved
    );
}

#[test]
fn plan_maximizes_gain_not_fastest_payback_and_retains_immutable_history() {
    let j = Journey::new();
    j.demand();
    let saved = j.set(
        vec![
            option("fast", 1000.0, 0.0, 200.0),
            option("larger-gain", 6000.0, 0.0, 600.0),
            option("too-expensive", 11000.0, 0.0, 2000.0),
        ],
        None,
    );
    assert!(saved.status.success());
    let result = j.plan("10000", "24");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report = document(&result);
    // Both positive options cover the same need, so adding their benefits is forbidden.
    assert_eq!(report["portfolio"]["selected_ids"], json!(["larger-gain"]));
    assert_eq!(report["portfolio"]["horizon_net_usd"], 8400.0);
    assert_eq!(report["portfolio"]["payback_months"], 10.0);
    assert_eq!(report["portfolio"]["remaining_budget_usd"], 4000.0);
    assert_eq!(report["portfolio"]["roi_pct"], 140.0);
    assert_eq!(j.persisted(&report), report);
    assert!(j
        .set(Vec::new(), document(&saved)["version"].as_str())
        .status
        .success());
    let shown = j.invoke(
        &[
            "fleet",
            "expansion",
            "show",
            report["plan_id"].as_str().unwrap(),
            "--json",
        ],
        None,
    );
    assert!(shown.status.success());
    assert_eq!(document(&shown), report);
    let history = j.invoke(&["fleet", "expansion", "history", "--json"], None);
    assert_eq!(document(&history)["plans"][0], report);
    assert_eq!(j.persisted(&report), report);
}

#[test]
fn operating_cost_and_delivery_delay_affect_budget_gain_and_payback() {
    let j = Journey::new();
    j.demand();
    let mut delayed = option("delayed", 1000.0, 50.0, 250.0);
    delayed["lead_time_days"] = json!(365);
    assert!(j.set(vec![delayed], None).status.success());
    let refused = j.plan("2199.99", "24");
    assert!(!refused.status.success());
    let rejection = document(&refused);
    assert_eq!(rejection["status"], "no_viable_option");
    assert_eq!(rejection["portfolio"]["selected_ids"], json!([]));
    assert_eq!(j.persisted(&rejection), rejection);
    let accepted = j.plan("2200", "24");
    assert!(accepted.status.success());
    let report = document(&accepted);
    let lead = 365.0 / (365.25 / 12.0);
    let payback = lead + (1000.0 + 50.0 * lead) / 200.0;
    assert!((report["portfolio"]["payback_months"].as_f64().unwrap() - payback).abs() < 0.000001);
    assert_eq!(report["portfolio"]["committed_cost_usd"], 2200.0);
    assert!(
        (report["portfolio"]["horizon_net_usd"].as_f64().unwrap()
            - (250.0 * (24.0 - lead) - 2200.0))
            .abs()
            < 0.000001
    );
    assert_eq!(j.persisted(&report), report);
}

#[test]
fn unknown_expired_and_nonreturning_inputs_do_not_become_recommendations() {
    let j = Journey::new();
    j.demand();
    let absent = j.plan("10000", "24");
    assert!(!absent.status.success());
    assert_eq!(document(&absent)["status"], "insufficient_evidence");
    let mut unknown = option("unknown", 2000.0, 50.0, 250.0);
    unknown["monthly_savings_usd"] = Value::Null;
    let mut expired = option("expired", 1000.0, 0.0, 200.0);
    expired["observed_at"] = json!((chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339());
    expired["valid_until"] = json!((chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339());
    assert!(j
        .set(
            vec![unknown, expired, option("losing", 1000.0, 200.0, 100.0)],
            None
        )
        .status
        .success());
    let result = j.plan("10000", "24");
    assert!(!result.status.success());
    let report = document(&result);
    assert_eq!(report["portfolio"]["selected_ids"], json!([]));
    assert!(report["portfolio"]["payback_months"].is_null());
    assert!(report["candidates"][0]["monthly_net_usd"].is_null());
    assert!(report["candidates"][2]["payback_months"].is_null());
    assert_eq!(j.persisted(&report), report);
    let bad_id = j.invoke(
        &["fleet", "expansion", "show", "../../registry", "--json"],
        None,
    );
    assert!(!bad_id.status.success());
    assert!(String::from_utf8_lossy(&bad_id.stderr).contains("plan id must be a UUID"));
}
