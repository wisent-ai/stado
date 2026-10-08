//! The long-lived daemon loop: entry resolution, self-survival, self-update
//! and everything the tick is surrounded by once per `interval_seconds`.

use std::time::Duration;

use crate::cli::CmdError;
use crate::config;
use crate::primitives::failure::FailureCode;
use crate::queue::JobStorage;
use crate::targets::{fetch_registry_remote, load_registry_auto, Coordinator};

use super::beside::{Replication, ShapeSweep};
use super::grant::secrets_from_skarbiec;
use super::log;
use super::passes::{resolve_providers, run_tick, CoordinatorError};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Invocation {
    Daemon,
    /// The host worker owns image replacement and waits for its active jobs.
    Hosted,
}

/// Pick the coordinator entry: explicit name or host-placement selector, or
/// the active one, from the configured Stado registry with bundled backup.
async fn resolve_coordinator(target: Option<&str>) -> Result<Coordinator, CmdError> {
    let registry = load_registry_auto().await.map_err(CmdError::from)?;
    if let Some(target) = target {
        return registry
            .lookup_coordinator_selector(target)
            .cloned()
            .ok_or_else(|| {
                CmdError::click(format!(
                    "coordinator selector '{target}' not found in registry"
                ))
                .stating(FailureCode::NotFound)
            });
    }
    let active: Vec<&Coordinator> = registry.coordinators.iter().filter(|c| c.active).collect();
    if active.is_empty() {
        return Err(CmdError::click(
            "no active coordinator in registry. Set active=true on one entry \
             or pass --target NAME explicitly.",
        )
        .stating(FailureCode::Config));
    }
    if active.len() > 1 {
        let names = active
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(CmdError::click(format!(
            "multiple active coordinators ({names}); set active=true on exactly one"
        ))
        .stating(FailureCode::Config));
    }
    Ok(active[0].clone())
}

/// A tick that failed: the store keeps its own class; a scheduler or monitor
/// failure states none, because neither error type carries one.
fn tick_failure(error: CoordinatorError) -> CmdError {
    match error {
        CoordinatorError::Storage(error) => CmdError::from(error),
        other => CmdError::click(other.to_string()),
    }
}

/// Coordinator daemon entry point (Python `coordinator.run`). Returns the
/// process exit code; `Err` is a SystemExit-style fatal message.
pub async fn run(target: Option<&str>, invocation: Invocation) -> Result<i32, CmdError> {
    let coord = resolve_coordinator(target).await?;
    if coord.runtime == "gcp_cloud_function" {
        log(&format!(
            "coordinator '{}' runtime=gcp_cloud_function: tick is driven by \
             Cloud Scheduler, this daemon is a no-op. Use --target to point \
             at a runtime=daemon entry instead.",
            coord.name
        ));
        return Ok(0);
    }

    let store = JobStorage::new().await.map_err(CmdError::from)?;
    // The cadence is the registry entry's own declaration; nothing here
    // raises or lowers it.
    let interval = u64::try_from(coord.interval_seconds)
        .ok()
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| {
            CmdError::click(format!(
                "coordinator '{}' declares interval_seconds {}; the tick needs a positive \
                 number of seconds between passes",
                coord.name, coord.interval_seconds
            ))
            .stating(FailureCode::Config)
        })?;
    log(&format!(
        "coordinator '{}' runtime={} interval={interval}s storage={} \
         registry_state_uri_metadata={:?}",
        coord.name,
        coord.runtime,
        store.backend_name(),
        coord.state_uri
    ));

    let secrets = secrets_from_skarbiec().await.map_err(CmdError::from)?;
    let mut replication = Replication::default();
    let mut shape = ShapeSweep::default();
    loop {
        if invocation != Invocation::Hosted
            && !config::stado_api_url().is_empty()
            && !config::stado_release_version().is_empty()
            && !config::stado_release_platform().is_empty()
        {
            let mut update_log = |message: &str| log(message);
            match crate::self_update::self_update(&mut update_log).await {
                Ok(crate::self_update::UpdateOutcome::Updated { from, to }) => {
                    log(&format!(
                        "coordinator self-update installed {from} -> {to}; re-executing"
                    ));
                    let exc = crate::self_update::reexec();
                    log(&format!(
                        "coordinator self-update re-exec failed; continuing old process image: \
                         {exc}"
                    ));
                }
                Ok(crate::self_update::UpdateOutcome::UpToDate { .. }) => {}
                Err(exc) => log(&format!(
                    "coordinator self-update failed; continuing current version: {exc}"
                )),
            }
        }
        // Re-resolve the coordinator entry from the registry each tick. The
        // initial resolve at process start captures the entry once and
        // never re-checks; if an operator pushes a new registry that
        // removes/renames the entry to stop a racing daemon, the running
        // process keeps reaping VMs forever using the cached entry: a stale
        // daemon that is already on the latest published version never sees
        // pip drift fire, and keeps deleting fresh-heartbeat VMs for hours
        // after its registry entry is gone. Re-resolving each tick means a
        // registry change takes effect within one interval_seconds without
        // depending on a new release being published.
        // The canonical registry is read from configured Stado storage and is
        // the only self-survival authority. A registry we could not read is
        // not an authority at all, even when primary reads are failing over.
        if let Some(target) = target {
            let survival = fetch_registry_remote().await;
            // Exit ONLY when a registry we actually READ omits the entry.
            // An unreachable store says nothing about whether the operator
            // revoked us — see `targets::RegistryFetchError`.
            if matches!(&survival, Ok(registry) if registry.lookup_coordinator_selector(target).is_none())
            {
                log(&format!(
                    "coordinator '{target}' not in the canonical registry; exiting. \
                     Operator removed/renamed the entry — daemon stops here so \
                     launchd/supervisor backs off and stale code stops issuing \
                     cloud-resource mutations."
                ));
                return Ok(0);
            }
            if let Err(exc) = survival {
                log(&format!(
                    "canonical registry unreachable ({exc}); SKIPPING the \
                     self-survival check for coordinator '{target}' and \
                     CONTINUING. A storage outage must never mass-terminate \
                     the fleet — the kill switch fires only against a \
                     registry that was actually read."
                ));
            }
        }
        let providers = resolve_providers();
        let n = run_tick(&store, &secrets, &providers, true, &log)
            .await
            .map_err(tick_failure)?;
        log(&format!("tick scheduled={n}"));
        // Publish the served queue namespace so submitters can refuse one
        // this fleet never claims from. An unchanged value, empty namespace
        // or lost compare-and-swap race needs no success log.
        if let Err(exc) =
            crate::targets::record_fleet_queue_namespace(config::wc_stado_storage_namespace()).await
        {
            log(&format!("fleet queue namespace record failed: {exc}"));
        }
        replication.advance();
        // The standing shape checks, so that "is what is declared what is
        // running" is answered without anyone typing a command. Every finding
        // carries its own subject, declaration, observation and fix, because a
        // tick log is the only place some of these will ever be read.
        //
        // Defects of one shape — a declaration nothing compares against
        // reality — get found and fixed by hand one evening at a time, and
        // nothing in the product catches the next one. This is what catches
        // it. The sweep reads every registry host and runs beside the tick:
        // awaited here it held the next tick back for minutes, and a
        // schedule due every minute fired eleven minutes late.
        shape.advance();
        tokio::time::sleep(until_next_tick(&store, interval).await).await;
    }
}

/// How long the loop waits before the next tick: the coordinator's declared
/// interval, or less when an enabled schedule falls due sooner, so a schedule
/// fires at its own time instead of at the next interval boundary. A schedule
/// already due after the tick that should have fired it waits the interval
/// like everything else; its firing error is in that tick's log.
async fn until_next_tick(store: &JobStorage, interval: u64) -> Duration {
    let declared = Duration::from_secs(interval);
    let schedules = match crate::schedules::list_schedules(store).await {
        Ok(schedules) => schedules,
        Err(error) => {
            log(&format!(
                "schedules unreadable, so the next tick waits the declared {interval}s: {error}"
            ));
            return declared;
        }
    };
    let now = chrono::Utc::now();
    let earliest = schedules
        .iter()
        .filter(|schedule| schedule.enabled && !schedule.deleted)
        .filter_map(|schedule| chrono::DateTime::parse_from_rfc3339(&schedule.next_due_at).ok())
        .map(|due| due.with_timezone(&chrono::Utc))
        .filter(|due| *due > now)
        .min();
    match earliest.and_then(|due| (due - now).to_std().ok()) {
        Some(wait) if wait < declared => wait,
        _ => declared,
    }
}
