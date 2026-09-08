//! Free space against the watermark a host declares, and whether the cleaner
//! set it declares can reach what actually fills the disk.

use serde_json::Value;

use super::super::{Finding, DISK_CHECK};
use crate::deploy::{host_disk, Runner};
use crate::targets::ComputeTarget;

/// Each janitor cleaner and the first released `stado` that accepts it in a
/// registry policy.
///
/// Derived from the tags, not guessed: `git tag --contains` on the commit that
/// added each name to `crate::targets`'s allowed list answers `stado-v0.12.0`
/// for `queue_workdirs` (#154) and `stado-v0.13.0` for `backup_twins`.
/// `release_store` first ships in 0.15.26; declaring it against an older host
/// makes that host reject the entire policy instead of merely skipping the
/// unknown cleaner. The four cleaners already in 0.9.5 — `build_caches`,
/// `chromium_clones`, `huggingface_cache`, `weles_recordings` — need no entry,
/// because no host in this fleet runs anything older.
const CLEANERS_BY_VERSION: &[(&str, &str)] = &[
    ("queue_workdirs", "0.12.0"),
    ("backup_twins", "0.13.0"),
    ("release_store", "0.15.26"),
];

/// Whether `installed` is at least `required`, comparing `X.Y.Z` numerically.
///
/// An unreadable or absent version answers false, so an unknown host is treated
/// as unable to take a new cleaner rather than assumed able: the cost of being
/// wrong that way is a note, and the cost of being wrong the other way is a
/// policy that stops every cleaner the host runs.
fn version_at_least(installed: &str, required: &str) -> bool {
    let parse = |value: &str| -> Option<(u64, u64, u64)> {
        let bare = value.trim().trim_start_matches('v');
        let bare = bare.split('-').next().unwrap_or_default();
        let mut parts = bare.split('.').map(|part| part.parse::<u64>().ok());
        Some((parts.next()??, parts.next()??, parts.next()??))
    };
    match (parse(installed), parse(required)) {
        (Some(installed), Some(required)) => installed >= required,
        _ => false,
    }
}

/// Free space against the watermark the registry declares, and a finding when
/// a managed host declares no policy at all.
pub(in crate::fleet_shape) async fn disk_headroom(
    target: &ComputeTarget,
    runner: &Runner,
    out: &mut Vec<Finding>,
    notes: &mut Vec<String>,
) {
    let report = match host_disk::disk_host(&target.name, runner).await {
        Ok(report) => report,
        Err(error) => {
            out.push(Finding {
                check: DISK_CHECK,
                subject: target.name.clone(),
                declared: "the host answers df".to_string(),
                observed: format!("disk read failed: {error}"),
                command: format!("stado space report {}", target.name),
            });
            return;
        }
    };
    let Some(policy) = target.disk_cleanup.as_ref() else {
        out.push(Finding {
            check: DISK_CHECK,
            subject: target.name.clone(),
            declared: "no disk_cleanup policy".to_string(),
            observed: "a managed compute host has no watermark, so nothing on it is ever obliged \
                       to free space"
                .to_string(),
            command: format!(
                "add targets[{}].disk_cleanup to the registry, then stado registry validate and push",
                target.name
            ),
        });
        return;
    };
    let available_kb = report
        .get("usage")
        .and_then(|usage| usage.get("available_kb"))
        .and_then(Value::as_str)
        .and_then(|value| value.trim().parse::<f64>().ok());
    let Some(available_kb) = available_kb else {
        out.push(Finding {
            check: DISK_CHECK,
            subject: target.name.clone(),
            declared: format!("low watermark {} GiB", policy.low_free_gb),
            observed: "df answered without an available column".to_string(),
            command: format!("stado space report {} --json", target.name),
        });
        return;
    };
    let free_gib = host_disk::gib_from_blocks(available_kb);
    if free_gib < policy.low_free_gb as f64 {
        out.push(Finding {
            check: DISK_CHECK,
            subject: target.name.clone(),
            declared: format!(
                "low watermark {} GiB, target {} GiB, mode {}",
                policy.low_free_gb, policy.target_free_gb, policy.mode
            ),
            observed: format!("{free_gib:.1} GiB free, so this host is refusing work"),
            command: format!(
                "stado space reclaim {} --apply --reason <why> and stado host backup-audit {} --reclaim-twins --apply",
                target.name, target.name
            ),
        });
    }
    // A cleaner set that cannot reach what fills the machine is the defect the
    // janitor spent a week not fixing on the always-on mac: it declared
    // huggingface_cache and weles_recordings while cargo build trees and a
    // same-disk replica took the disk down.
    //
    // Gated on the version the host RUNS, and that gate is not a nicety. A
    // registry `disk_cleanup` policy is validated as a whole by whatever binary
    // reads it, so declaring a cleaner an older binary does not recognise stops
    // every cleaner that host already runs. This check said exactly that in its
    // own remedy — "AFTER the host runs a binary that knows it" — and then
    // failed `stado doctor`, which `deploy_stado_rust.sh` runs as a delivery
    // preflight. So the finding blocked the delivery that was the prerequisite
    // for acting on the finding. A check that forbids its own remedy is worse
    // than no check, and weakening it would have been the wrong repair.
    let declared_cleaners: Vec<&str> = policy.cleaners.keys().map(String::as_str).collect();
    let installed = target
        .managed_versions
        .get("stado")
        .map(String::as_str)
        .unwrap_or_default();
    for (expected, since) in CLEANERS_BY_VERSION {
        if declared_cleaners.contains(expected) {
            continue;
        }
        if !version_at_least(installed, since) {
            notes.push(format!(
                "{}: {expected} undeclared and unsupported by installed stado {} (needs {since}) \
                 — deliver first, declare second",
                target.name,
                if installed.is_empty() {
                    "unknown"
                } else {
                    installed
                }
            ));
            continue;
        }
        out.push(Finding {
            check: DISK_CHECK,
            subject: format!("{}:{expected}", target.name),
            declared: format!(
                "cleaners {} on stado {installed}",
                declared_cleaners.join(", ")
            ),
            observed: format!(
                "{expected} is supported by the installed binary and not declared, so the janitor \
                 cannot reclaim what it owns"
            ),
            command: format!(
                "add {expected} to targets[{}].disk_cleanup.cleaners, then stado registry validate and push",
                target.name
            ),
        });
    }
}
