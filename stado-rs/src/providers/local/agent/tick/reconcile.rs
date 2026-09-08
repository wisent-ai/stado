//! The declarations this agent re-asserts on its own machine every tick: the
//! registry's VRAM and pinned-only overrides, the board power cap, and the
//! worker's placement policy.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::{Map, Value};

use crate::models::isoformat_utc;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::JobStorage;
use crate::targets::ComputeTarget;

use super::super::capacity::snapshot::{measured_capacity, publish_branch};
use super::super::{reconcile_gpu_power_limit, reconcile_placement_policy, Step, POLL_INTERVAL_S};

const GPU_POWER_RECONCILE_INTERVAL_S: u64 = 300;

pub(super) struct GpuPowerLimitState {
    desired_watts: u32,
    checked_at: Instant,
    checked_at_utc: String,
    ok: bool,
    detail: String,
}

/// How long between two reads of the host's placement policy file.
///
/// The worker re-reads the file itself every 30 seconds
/// (`CACHE_TTL_MS`, `placement-policy.ts`), so a reconcile slower than that
/// only delays when a registry edit takes effect, never how long a wrong file
/// stays in force once corrected. The same 300 the power limit uses: one
/// number for "how often this agent re-asserts a declaration".
const PLACEMENT_RECONCILE_INTERVAL_S: u64 = GPU_POWER_RECONCILE_INTERVAL_S;

/// The last placement-policy reconcile, so the pass is skipped while nothing
/// has changed and the outcome still reaches the capacity diagnostics.
pub(super) struct PlacementPolicyState {
    /// `(enabled, actions)` last written or confirmed — what the worker acts
    /// on, which is the only part a rewrite would change.
    desired: (bool, Vec<String>),
    checked_at: Instant,
    checked_at_utc: String,
    ok: bool,
    detail: String,
}

/// Apply the registry target's host-level declarations, and refuse admission
/// while the board power cap this host declares is not the one it reports.
#[allow(clippy::too_many_arguments)]
pub(super) async fn registry_declarations(
    store: &JobStorage,
    consumer_id: &str,
    kind: &str,
    initial_gpu: &str,
    registry_target: &Option<ComputeTarget>,
    slots: &[ActiveSlot],
    total_vram_gb: &mut i64,
    pinned_only: &mut bool,
    agent_diag: &mut Map<String, Value>,
    gpu_power_limit_state: &mut Option<GpuPowerLimitState>,
    placement_policy_state: &mut Option<PlacementPolicyState>,
    last_cap: &mut Option<CapacitySnapshot>,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<Step<()>> {
    let mut gpu_power_policy_ok = true;
    if let Some(t) = registry_target.as_ref() {
        if t.is_provider(crate::capabilities::ProviderId::Local) {
            // Env overrides are now owned by systemd (/etc/wisent/wisent-agent.env).
            // Ignore registry env deltas so an external registry push cannot
            // trigger a pip reinstall loop or override local tuning.
            if let Some(t_gpu) = &t.gpu_type {
                if !t_gpu.is_empty() && *t_gpu != initial_gpu && slots.is_empty() {
                    log_fn(&format!(
                        "Registry gpu_type {initial_gpu} -> {t_gpu}; pip_upgrade_and_exec for restart"
                    ));
                    // DEVIATION: no re-exec here — the self-update
                    // path fires on version drift only (see
                    // version_check); a gpu_type change remains an
                    // operator-restart action.
                }
            }
            if let Some(t_vram) = t.vram_gb {
                if t_vram > 0 && t_vram != *total_vram_gb {
                    log_fn(&format!(
                        "Registry vram_gb override {total_vram_gb} -> {t_vram}"
                    ));
                    *total_vram_gb = t_vram;
                }
            }
            *pinned_only = t.pinned_only;
            if *pinned_only {
                agent_diag.insert("pinned_only".into(), Value::from(true));
            }
            if let Some(watts) = t.gpu_power_limit_watts() {
                let reconcile_due = gpu_power_limit_state.as_ref().is_none_or(|state| {
                    state.desired_watts != watts
                        || !state.ok
                        || state.checked_at.elapsed()
                            >= Duration::from_secs(GPU_POWER_RECONCILE_INTERVAL_S)
                });
                if reconcile_due {
                    let checked_at_utc = isoformat_utc(Utc::now());
                    let result = reconcile_gpu_power_limit(watts).await;
                    let (ok, detail) = match result {
                        Ok(detail) => (true, detail),
                        Err(detail) => {
                            log_fn(&format!("GPU power-limit reconciliation failed: {detail}"));
                            (false, detail)
                        }
                    };
                    *gpu_power_limit_state = Some(GpuPowerLimitState {
                        desired_watts: watts,
                        checked_at: Instant::now(),
                        checked_at_utc,
                        ok,
                        detail,
                    });
                }
                if let Some(state) = gpu_power_limit_state {
                    gpu_power_policy_ok = state.ok;
                    agent_diag.insert(
                        "gpu_power_limit_watts".into(),
                        Value::from(state.desired_watts),
                    );
                    agent_diag.insert("gpu_power_limit_ok".into(), Value::from(state.ok));
                    agent_diag.insert(
                        "gpu_power_limit_checked_at".into(),
                        Value::from(state.checked_at_utc.clone()),
                    );
                    agent_diag.insert(
                        "gpu_power_limit_detail".into(),
                        Value::from(state.detail.clone()),
                    );
                }
            } else {
                *gpu_power_limit_state = None;
                for key in [
                    "gpu_power_limit_watts",
                    "gpu_power_limit_ok",
                    "gpu_power_limit_checked_at",
                    "gpu_power_limit_detail",
                ] {
                    agent_diag.remove(key);
                }
            }
            // The registry declares `weles.actions` per host and the
            // worker reads its own `placement-policy.json`. Until this
            // ran, the only thing joining them was an operator typing
            // `stado host publish-placement-policy`, so the two drifted
            // silently: the registry listed an action the file did not
            // and the worker declined every routed row while the
            // declaration said it was allowed. Asserted here for the same
            // reason the power limit is — a declaration nothing enforces
            // is a declaration written for nobody.
            if t.weles.is_some() {
                let reconcile_due = placement_policy_state.as_ref().is_none_or(|state| {
                    !state.ok
                        || state.checked_at.elapsed()
                            >= Duration::from_secs(PLACEMENT_RECONCILE_INTERVAL_S)
                });
                if reconcile_due {
                    let checked_at_utc = isoformat_utc(Utc::now());
                    let (ok, detail, desired) = match reconcile_placement_policy(t).await {
                        Ok((detail, desired)) => (true, detail, desired),
                        Err(detail) => {
                            log_fn(&format!("placement-policy reconciliation failed: {detail}"));
                            (false, detail, (false, Vec::new()))
                        }
                    };
                    *placement_policy_state = Some(PlacementPolicyState {
                        desired,
                        checked_at: Instant::now(),
                        checked_at_utc,
                        ok,
                        detail,
                    });
                }
                if let Some(state) = placement_policy_state {
                    agent_diag.insert(
                        "placement_policy_enabled".into(),
                        Value::from(state.desired.0),
                    );
                    agent_diag.insert(
                        "placement_policy_actions".into(),
                        Value::from(state.desired.1.clone()),
                    );
                    agent_diag.insert("placement_policy_ok".into(), Value::from(state.ok));
                    agent_diag.insert(
                        "placement_policy_checked_at".into(),
                        Value::from(state.checked_at_utc.clone()),
                    );
                    agent_diag.insert(
                        "placement_policy_detail".into(),
                        Value::from(state.detail.clone()),
                    );
                }
            } else {
                // A host that declares no `weles` block has no policy to
                // assert, and the file it may already carry is not this
                // agent's to remove: deleting it would take a worker out
                // on the strength of an absent declaration.
                *placement_policy_state = None;
                for key in [
                    "placement_policy_enabled",
                    "placement_policy_actions",
                    "placement_policy_ok",
                    "placement_policy_checked_at",
                    "placement_policy_detail",
                ] {
                    agent_diag.remove(key);
                }
            }
        }
    }
    if !gpu_power_policy_ok {
        let snapshot = measured_capacity(
            slots,
            false,
            Some("gpu_power_policy_unmet"),
            BTreeMap::new(),
            0,
            *total_vram_gb,
            agent_diag.clone(),
        );
        publish_branch(
            store,
            consumer_id,
            kind,
            "gpu-power-policy-unmet",
            &snapshot,
            log_fn,
        )
        .await?;
        *last_cap = Some(snapshot);
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
        return Ok(Step::Done);
    }
    Ok(Step::Go(()))
}
