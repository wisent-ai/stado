//! One reconcile pass over every release-controlled product on this host, and
//! the loop that repeats it.

use std::time::Duration;

use super::product::reconcile_product;
use crate::cli::CmdError;
use crate::release_agent::state::document::{
    acquire_product_reconcile_lock, load_state, save_state,
};
use crate::release_agent::state::records::{HostReleaseState, RolloutPhase};
use crate::release_agent::state::status::publish_status;
use crate::release_control::StrategyKind;

/// A host's own release state file could not be locked, read or written:
/// the host's outage, said with the file's own sentence.
fn state_failure(detail: String) -> CmdError {
    CmdError::unreachable(detail)
}

/// One pass. The registry keeps its class; a release-control declaration that
/// does not validate is config; the host's own state files are infra_down. A
/// product whose rollout fails is recorded as `Failed` in its state and is not
/// an error of the pass.
pub async fn reconcile_once(
    target_name: &str,
    product_filter: Option<&str>,
) -> Result<Vec<HostReleaseState>, CmdError> {
    let document = crate::cli::resolver::canonical_document_or_last_good(target_name).await?;
    crate::release_control::validate_registry_contract(&document).map_err(CmdError::declaration)?;
    // No `release_control` is zero rollout products, NOT the end of the tick.
    //
    // The unit-image revisit policy is a top-level registry key and names its
    // own state directory, so it is entirely independent of whether this
    // document declares any blue-green rollout — and the units it exists for
    // are precisely the ones no rollout owns: the Stado release's own janitor
    // and resolver, and a stream writer this catalogue does not carry.
    // Returning here would have made the feature unreachable on exactly the
    // hosts it was built for.
    let control = crate::release_control::control(&document).map_err(CmdError::declaration)?;
    let mut states = Vec::new();
    for (product, policy) in control
        .as_ref()
        .map(|control| control.products.iter())
        .unwrap_or_default()
    {
        if product_filter.is_some_and(|selected| selected != product) {
            continue;
        }
        if policy.strategy.kind != StrategyKind::BlueGreen {
            // A `replace` policy is delivered by the host-release path: the
            // artefact tree is swapped in place, and there is no stable
            // proxy bind or candidate port pair for this reconciler to
            // switch between. The agent must not drive it — only hand the
            // port it served blue-green before to the product's own unit.
            if policy.targets.contains_key(target_name) {
                match crate::release_agent::rollout::processes::handover::hand_over_to_unit(
                    &document,
                    product,
                    policy,
                    target_name,
                )
                .await
                {
                    Ok(Some(detail)) => eprintln!("[release-agent] {detail}"),
                    Ok(None) => {}
                    Err(error) => eprintln!(
                        "[release-agent] {product}: handing its blue-green port to its unit \
                         failed: {error}"
                    ),
                }
            }
            continue;
        }
        let Some(target) = policy.targets.get(target_name) else {
            continue;
        };
        let Some(_reconcile_lock) =
            acquire_product_reconcile_lock(target, product).map_err(state_failure)?
        else {
            continue;
        };
        let control = control.as_ref().ok_or_else(|| {
            CmdError::declaration("release-control product resolved without its document")
        })?;
        let result = reconcile_product(control, product, policy, target_name, target).await;
        let mut state = match result {
            Ok(state) => state,
            Err(reason) => {
                let mut state = load_state(target, product, target_name).map_err(state_failure)?;
                state.phase = RolloutPhase::Failed;
                state.detail = reason;
                save_state(target, &mut state).map_err(state_failure)?;
                state
            }
        };
        if let Err(error) = publish_status(&state).await {
            state.detail = format!("{}; status publish failed: {error}", state.detail);
            save_state(target, &mut state).map_err(state_failure)?;
        }
        states.push(state);
    }
    // What release control left here for a product whose policy no longer
    // names this host. Only on a full pass over a document that has release
    // control at all: a product filter sees one product, not the target set.
    if let (None, Some(control), Ok(home)) =
        (product_filter, control.as_ref(), std::env::var("HOME"))
    {
        let targeted: std::collections::BTreeSet<String> = control
            .products
            .iter()
            .filter(|(_, policy)| policy.targets.contains_key(target_name))
            .map(|(product, _)| product.clone())
            .collect();
        for line in
            crate::release_agent::rollout::processes::handover::retire_untargeted(&home, &targeted)
                .await
        {
            eprintln!("[release-agent] {line}");
        }
    }
    // The revisit pass, after the rollouts and never instead of them.
    //
    // A tick's first duty is the release it was asked to deliver; putting a
    // unit back on a file it already declares is repair work, and a repair
    // that delayed a rollout by up to the settle window every tick would be
    // paying for this feature out of the one this agent exists for.
    //
    // It takes the document THIS tick already resolved. A second registry
    // read would be a behavioural change on a fleet that opted into nothing,
    // and the absent-by-default bound promises exactly that it is not one:
    // `release_unit_image::policy` returns `Ok(None)` whenever the document
    // carries no `release_unit_image_revisit` block, before a process table,
    // a unit file or a disk is read.
    //
    // A failure here is reported and never returned. The revisit pass is not
    // the rollout, and a malformed policy or an unreadable ledger must not
    // become a `Failed` phase on a product whose candidate is serving
    // perfectly well. It must not be silent either: a policy block that will
    // not parse, or a contract that will not resolve, is the reason no unit is
    // being repaired, so it is said here — and `registry doctor` reports the
    // same document through `build-refuses-registry`, because the validator
    // that refuses it is wired into `validate_registry_body`.
    let revisit = match crate::release_unit_image::validate_registry_contract(&document) {
        Ok(()) => match crate::release_unit_image::policy(&document) {
            Ok(Some(policy)) => {
                crate::release_unit_image::revisit_once(
                    &document,
                    &policy,
                    target_name,
                    product_filter,
                )
                .await
            }
            Ok(None) => Ok(None),
            Err(reason) => Err(reason),
        },
        Err(reason) => Err(reason),
    };
    match revisit {
        Ok(Some(report)) => eprintln!("{}", report.line()),
        Ok(None) => {}
        Err(reason) => eprintln!(
            "stado release agent unit-image revisit host={target_name} could not run: {reason}"
        ),
    }
    Ok(states)
}

pub async fn agent(
    target_name: &str,
    product_filter: Option<&str>,
    once: bool,
    interval_seconds: u64,
) -> Result<(), CmdError> {
    loop {
        let states = reconcile_once(target_name, product_filter).await?;
        for state in states {
            eprintln!(
                "stado release agent product={} target={} generation={} phase={:?} detail={}",
                state.product, state.target, state.rollout_generation, state.phase, state.detail
            );
        }
        // Submissions are queued, not waited for: on the control host this is
        // where their builds turn into signed, published, delivered releases.
        match crate::cli::release_submit::finish_ready_runs().await {
            Ok(finished) => {
                for run in finished {
                    eprintln!("stado release agent run={run} finished");
                }
            }
            Err(reason) => eprintln!("stado release agent finish pass failed: {reason}"),
        }
        if once {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(interval_seconds)).await;
    }
}
