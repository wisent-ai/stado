use std::time::Duration;

use serde_json::{json, Value};

use super::super::ReleasePlan;
use crate::deploy::products::{Install, Readback};
use crate::deploy::{host_channel, service, Runner};
use crate::targets::ComputeTarget;

/// How long a restarted agent is given to publish its stable binds.
pub(super) const STABLE_BIND_BUDGET_SECONDS: u64 = 120;

/// Poll every stable bind this host declares until it listens, and report
/// each one.
///
/// The registry comes through the reader that falls back to this host's
/// last-known-good copy, because the outage this guard exists to catch is one
/// in which the authority cannot be read: a verification that needed the
/// authority would go blind at exactly the moment it matters. A host whose
/// registry cannot be read at all reports no verdicts rather than a false
/// failure — the roll's own steps already carry that.
pub(super) async fn verify_stable_binds(
    target: &ComputeTarget,
    runner: &Runner,
) -> (serde_json::Map<String, Value>, Vec<String>) {
    let mut verdicts = serde_json::Map::new();
    let mut missing = Vec::new();
    let Ok((registry, _)) = crate::targets::fetch_registry_or_last_good().await else {
        return (verdicts, missing);
    };
    let plans = crate::deploy::host_recovery::plan_stable_binds(&registry.to_document(), target);
    if plans.is_empty() {
        return (verdicts, missing);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(STABLE_BIND_BUDGET_SECONDS);
    for plan in plans {
        let port = plan.bind.rsplit(':').next().unwrap_or_default().to_string();
        let listening = loop {
            if stable_bind_listening(target, &port, runner).await {
                break true;
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        };
        verdicts.insert(
            plan.bind.clone(),
            json!({
                "product": plan.product,
                "verdict": if listening { "listening" } else { "absent" },
            }),
        );
        if !listening {
            missing.push(format!("{} ({})", plan.bind, plan.product));
        }
    }
    (verdicts, missing)
}

/// Whether one loopback port has a listener on the target, read with the same
/// `lsof` spelling every other reader of "what is listening" uses.
async fn stable_bind_listening(target: &ComputeTarget, port: &str, runner: &Runner) -> bool {
    let selector = format!("-iTCP:{port}");
    let words = ["/usr/sbin/lsof", "-nP", selector.as_str(), "-sTCP:LISTEN"];
    host_channel::run_program(target, &words, runner)
        .await
        .is_ok_and(|output| output.ok() && !output.stdout.trim().is_empty())
}

/// What a `--dry-run` says it would do, in the order it would do it.
///
/// `code_paths` is what the read-only probe found in the install root of a
/// tree, so the paths this promises to replace and the paths it promises to
/// keep are the paths that are actually there — not a guess made on the
/// control plane about a host nobody looked at.
pub(super) fn planned_steps(
    plan: &ReleasePlan,
    units: &[service::ManagedService],
    code_paths: &[String],
) -> Vec<String> {
    let readback = match &plan.product.readback {
        Readback::Program { .. } => String::new(),
        Readback::JsonFile { path, pointer } => format!(" in {path} {pointer}"),
    };
    let mut steps = vec![
        format!(
            "fetch {} through {}/api/release/object",
            plan.release_uri(),
            plan.release_api
        ),
        format!(
            "verify archive sha256 {} from the release manifest",
            plan.sha256
        ),
        format!(
            "extract {} and verify it declares {}{readback}",
            plan.product.source.member, plan.version
        ),
        format!("stage it at {}", plan.staged_path()),
    ];
    match &plan.product.install {
        Install::Program { .. } => steps.push(format!(
            "re-check the staged version and atomically repoint {}",
            plan.active_path()
        )),
        Install::Tree { root, .. } => {
            steps.push(format!(
                "re-check the staged version and replace the code under {root}, one rename each, \
                 retiring what it replaces: {}",
                if code_paths.is_empty() {
                    "nothing is installed there yet".to_string()
                } else {
                    code_paths.join(", ")
                }
            ));
            steps.push(format!(
                "preserve untouched, never moved and never named as a destination: {}",
                plan.preserved_paths().join(", ")
            ));
        }
    }
    if units.is_empty() {
        steps.push(format!(
            "no restart: the registry declares no units running {}",
            plan.product.name
        ));
    } else {
        steps.extend(
            units
                .iter()
                .map(|declared| format!("restart {}", declared.unit_id())),
        );
    }
    steps
}
