//! One reconcile invocation: take the host lock, observe, commit the intent,
//! restart one unit, read the identity again, and record what happened.

pub(crate) mod annotate;
pub(crate) mod report;

use serde_json::Value;

use crate::cli::service_refresh_image::{refresh_outcome, settle};
use crate::deploy::service;

use super::ledger::attempt::RevisitAttempt;
use super::ledger::identity::{AttemptOutcome, FileIdentity};
use super::ledger::{load_ledger, save_ledger, RevisitLedger, LOCK_STEM};
use super::plan::{revisit_plan, RevisitPick};
use super::registry_policy::scope::host_scope;
use super::registry_policy::{RevisitPolicy, REVISIT_POLICY_KEY};

use report::RevisitReport;

/// Record one attempt against `unit` and commit the ledger.
fn record(
    state_dir: &str,
    ledger: &mut RevisitLedger,
    pick: &RevisitPick,
    outcome: AttemptOutcome,
    attempted_at: &str,
    service: String,
) -> Result<(), String> {
    ledger.attempts.insert(
        pick.unit.clone(),
        RevisitAttempt {
            was_running: FileIdentity::of(&pick.running),
            declared: FileIdentity::of(&pick.declared),
            outcome: outcome.word().to_string(),
            attempted_at: attempted_at.to_string(),
            service,
        },
    );
    save_ledger(state_dir, ledger)
}

/// Restart at most one authorised stale unit on this host, verify the
/// identity, and record the result.
///
/// The revisit branch in `reconcile_once` receives `Ok(None)` when
/// [`super::registry_policy::policy`] finds no `release_unit_image_revisit`
/// key; it never calls this function or [`host_scope`], so the feature adds no
/// process-table, unit-file, lock or ledger access.
///
/// `document` is the one the caller already resolved; reading the registry
/// again would be a behavioural change on a fleet that opted into nothing.
pub(crate) async fn revisit_once(
    document: &Value,
    policy: &RevisitPolicy,
    target_name: &str,
    product_filter: Option<&str>,
) -> Result<Option<RevisitReport>, String> {
    let Some(scope) = host_scope(policy, target_name)? else {
        return Ok(None);
    };
    let owned = scope.owned(product_filter);
    if owned.is_empty() {
        return Ok(None);
    }
    let registry = crate::targets::load_registry_from_str(&document.to_string())
        .map_err(|error| format!("cannot read the registry targets: {error}"))?;
    let target = registry.lookup(target_name).ok_or_else(|| {
        format!(
            "registry.{REVISIT_POLICY_KEY} authorises units on {target_name}, which names no \
             target"
        )
    })?;
    // Which image a pid executes is answerable only on the machine holding it
    // — the whole reason `observe_unit_images` takes `local_units`. `--target`
    // is an operator-supplied string, so this machine's own target is resolved
    // from its hostname the way `service refresh-image` does. Without it a
    // `--target charless-mac-mini` run on a laptop would read the laptop's
    // process table and kickstart the laptop's units under another host's name.
    let hostname = crate::providers::vast::system_hostname();
    let this_machine = registry
        .lookup_self(&hostname)
        .map_err(|error| format!("cannot resolve this machine in the registry: {error}"))?
        .map(|target| target.name.clone());
    if this_machine.as_deref() != Some(target_name) {
        return Err(format!(
            "registry.{REVISIT_POLICY_KEY} authorises units on {target_name} and this machine \
             ({hostname}) \
             resolves to {}; which image a process is executing is readable only on the machine \
             holding that process, so nothing was read and nothing was restarted",
            this_machine.as_deref().unwrap_or("no registry target")
        ));
    }
    // One lock over observe -> record -> kickstart -> settle -> record.
    //
    // What it prevents is OVERLAP: two reconciles running at the same time
    // would each observe the same stale unit against the same unchanged
    // identity pair and each spend a restart on it, because neither would see
    // the other's ledger write. It is not a rate limit and there is no time
    // window. Sequential invocations are separate ticks and each may act on
    // one unit — a different one, since the unit this tick handled is now
    // either on its declared file or barred by its own record.
    let Some(_lock) = crate::release_agent::acquire_state_lock(&scope.state_dir, LOCK_STEM)? else {
        return Ok(Some(RevisitReport {
            host: target_name.to_string(),
            acted: None,
            skipped: Vec::new(),
            busy: true,
        }));
    };
    let mut ledger = load_ledger(&scope.state_dir, target_name)?;
    let observations =
        service::observe_unit_image_scan(target, Some(target_name), chrono::Utc::now().timestamp())
            .await;
    let plan = revisit_plan(&observations, &owned, &ledger);
    let Some(pick) = plan.pick else {
        // Nothing to restart and nothing an operator has to act on: every
        // authorised unit is on its declared file, or the only skips are
        // settled records already dated in the ledger and annotated on the
        // doctor row. Saying so once per tick, forever, is how a feature that
        // exists to make a condition legible becomes the noise its own signal
        // is lost in — so opting in must not add a permanent log line to a
        // healthy host.
        if plan.skipped.iter().all(|(_, skip)| skip.is_settled()) {
            return Ok(None);
        }
        return Ok(Some(RevisitReport {
            host: target_name.to_string(),
            acted: None,
            skipped: plan.skipped,
            busy: false,
        }));
    };
    let attempted_at = chrono::Utc::now().to_rfc3339();
    // Write the intent BEFORE the side effect, and refuse the side effect if
    // the write fails. A ledger that records an attempt only once its outcome
    // is known loses the attempt to any crash or write failure in the window
    // that follows, and the next tick then kickstarts the same unit again —
    // the hot loop, arriving precisely when the host is already unhealthy.
    // Refusing here costs one deferred repair; the alternative costs an
    // unbounded restart loop.
    record(
        &scope.state_dir,
        &mut ledger,
        &pick,
        AttemptOutcome::Attempting,
        &attempted_at,
        "about to reconcile the observed launchd unit".to_string(),
    )
    .map_err(|error| {
        format!(
            "{} was NOT restarted because the attempt could not be recorded first, and an \
             unrecorded restart is one this host would repeat every tick: {error}",
            pick.unit
        )
    })?;
    let (outcome, service_target) =
        match service::restart_local_unit(target, &pick.unit, &pick.unit_path, None).await {
            Ok(service_target) => {
                let after = settle(target, target_name, &pick.unit, pick.pid).await;
                (
                    AttemptOutcome::Observed(refresh_outcome(&pick.running, after.as_ref())),
                    service_target,
                )
            }
            // Recorded as what it was. A refused restart is an attempt this host
            // has spent — re-issuing the same refused command every tick is the
            // hot loop this module is bounded against — but it observed nothing,
            // so it must not borrow an outcome word that claims a second read.
            Err(reason) => (AttemptOutcome::RestartRefused, format!("refused: {reason}")),
        };
    // Replaces the `Attempting` record in place, on the same identity pair, so
    // the ledger holds one row per unit either way.
    record(
        &scope.state_dir,
        &mut ledger,
        &pick,
        outcome,
        &attempted_at,
        service_target,
    )?;
    Ok(Some(RevisitReport {
        host: target_name.to_string(),
        acted: Some((pick, outcome)),
        skipped: plan.skipped,
        busy: false,
    }))
}
