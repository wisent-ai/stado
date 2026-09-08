//! Live per-accelerator capacity on every configured provider.

use serde_json::Value;

use crate::config;
use crate::doctor::plane::storage::round_trip::STORAGE_REMEDY;
use crate::doctor::{Check, Findings, Status, LOCAL_PROVIDER};
use crate::queue::JobStorage;
use crate::scheduler::quota;

// ---------------------------------------------------------------------------
// 4. Quota
// ---------------------------------------------------------------------------

pub(in crate::doctor) const QUOTA_ID: &str = "quota";
pub(in crate::doctor) const QUOTA_TITLE: &str = "Quota";
pub(in crate::doctor) const QUOTA_REMEDY: &str =
    "`stado quota show` prints the live picture; raise a ceiling with `stado quota request \
     --accel <ACCEL> --new-limit <N>`, and check the reservation overlay at config/quotas.json \
     in the queue store";

/// Live per-accelerator quota through [`quota::load_quotas`] — the same
/// call the dispatcher's admission control makes. Nothing schedulable
/// anywhere is a hard FAIL: no bucket can ever be dispatched, which is
/// exactly what an all-zero Azure subscription looks like from outside.
pub(in crate::doctor) async fn check_quota(store: Option<&JobStorage>, store_error: &str) -> Check {
    let Some(store) = store else {
        return Check::fail(
            QUOTA_ID,
            QUOTA_TITLE,
            format!(
                "quota needs the reservation overlay from the queue store, which could not be \
                 constructed: {store_error}"
            ),
            STORAGE_REMEDY,
        );
    };
    let mut findings = Findings::default();
    let mut any_capacity = false;
    for name in config::wc_providers() {
        if name == LOCAL_PROVIDER {
            // A device-local deployment schedules on the box's own GPU, so
            // it is real capacity even with no cloud quota anywhere.
            findings.note(
                Status::Pass,
                format!("{name}: device-local, admission is by live VRAM not cloud quota"),
            );
            any_capacity = true;
            continue;
        }
        match quota::load_quotas(store, name).await {
            Err(err) => {
                findings.note(Status::Fail, format!("{name}: {err}"));
                findings.remedy(QUOTA_REMEDY);
            }
            Ok(document) => {
                let rows = document.get(name).and_then(Value::as_object);
                let Some(rows) = rows.filter(|rows| !rows.is_empty()) else {
                    findings.note(
                        Status::Fail,
                        format!("{name}: the quota API reported no accelerator at all"),
                    );
                    findings.remedy(QUOTA_REMEDY);
                    continue;
                };
                let mut totals: Vec<String> = Vec::new();
                let mut provider_capacity = false;
                for (accel, row) in rows {
                    let total = row.get("total").and_then(Value::as_i64).unwrap_or_default();
                    let reserved = row
                        .get("reserved")
                        .and_then(Value::as_i64)
                        .unwrap_or_default();
                    provider_capacity |= (total - reserved).is_positive();
                    totals.push(format!("{accel}={total}(-{reserved} reserved)"));
                }
                any_capacity |= provider_capacity;
                let status = if provider_capacity {
                    Status::Pass
                } else {
                    Status::Warn
                };
                findings.note(status, format!("{name}: {}", totals.join(" ")));
            }
        }
    }
    if !any_capacity {
        findings.note(
            Status::Fail,
            "no accelerator has schedulable capacity on any configured provider; every dispatch \
             attempt fails admission and the fleet stays at zero VMs"
                .to_string(),
        );
        findings.remedy(QUOTA_REMEDY);
    }
    findings.into_check(QUOTA_ID, QUOTA_TITLE, QUOTA_REMEDY)
}
