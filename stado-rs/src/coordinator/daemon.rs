//! The long-lived daemon loop: entry resolution, self-survival, self-update
//! and everything the tick is surrounded by once per `interval_seconds`.

use std::time::Duration;

use crate::config;
use crate::queue::JobStorage;
use crate::targets::{fetch_registry_remote, load_registry_auto, Coordinator};

use super::grant::secrets_from_skarbiec;
use super::log;
use super::passes::{resolve_providers, run_tick};

/// Pick the coordinator entry: explicit name or host-placement selector, or
/// the active one, from the configured Stado registry with bundled backup.
async fn resolve_coordinator(target: Option<&str>) -> Result<Coordinator, String> {
    let registry = load_registry_auto().await.map_err(|exc| exc.to_string())?;
    if let Some(target) = target {
        return registry
            .lookup_coordinator_selector(target)
            .cloned()
            .ok_or_else(|| format!("coordinator selector '{target}' not found in registry"));
    }
    let active: Vec<&Coordinator> = registry.coordinators.iter().filter(|c| c.active).collect();
    if active.is_empty() {
        return Err(
            "no active coordinator in registry. Set active=true on one entry \
             or pass --target NAME explicitly."
                .into(),
        );
    }
    if active.len() > 1 {
        let names = active
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "multiple active coordinators ({names}); set active=true on exactly one"
        ));
    }
    Ok(active[0].clone())
}

/// Coordinator daemon entry point (Python `coordinator.run`). Returns the
/// process exit code; `Err` is a SystemExit-style fatal message.
pub async fn run(target: Option<&str>, once: bool) -> Result<i32, String> {
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

    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let interval = coord.interval_seconds.max(15) as u64;
    log(&format!(
        "coordinator '{}' runtime={} interval={interval}s storage={} \
         registry_state_uri_metadata={:?}",
        coord.name,
        coord.runtime,
        store.backend_name(),
        coord.state_uri
    ));

    let secrets = secrets_from_skarbiec()
        .await
        .map_err(|err| err.to_string())?;
    loop {
        if !config::stado_api_url().is_empty()
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
        // process keeps reaping VMs forever using the cached entry.
        // Confirmed live 2026-05-15: a stale mac mini daemon kept deleting
        // fresh-heartbeat Llama/Qwen3 VMs for 4+ hours after the registry
        // entry was removed because pip drift never fired (the daemon was
        // already on the latest published version). Re-resolving each tick
        // means a registry change takes effect within one interval_seconds
        // without depending on a new release being published.
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
        // Native-build poller: watch registry build recipes for new commits,
        // enqueue build jobs, and record what the finished ones produced.
        // Self-rate-limited to one pass per minute; per-recipe failures are
        // logged inside, never raised.
        //
        // BEFORE `run_tick`, because `run_tick` ends with the by-run reaper.
        // A build job belongs to a `runs/` manifest, so the reaper retires it
        // and deletes both its `completed/` record and the output it uploaded.
        // Reconciling afterwards read a finished build as a job that vanished
        // and recorded it as failed with no artifacts. The consumer of a
        // terminal outcome runs before the pass that cleans it up.
        crate::scheduler::builds::poll_build_recipes(&log).await;
        let providers = resolve_providers();
        let n = run_tick(&store, &secrets, &providers, true, &log)
            .await
            .map_err(|exc| exc.to_string())?;
        log(&format!("tick scheduled={n}"));
        // Record the queue namespace this coordinator serves into the
        // canonical registry so submitters can refuse an ambient namespace
        // the fleet never claims from (the 2026-08-19 silent-stall: a job
        // submitted under the operator's ambient namespace sat unclaimed
        // for hours). Ok(false) — already current, empty namespace, or a
        // lost CAS race — needs no line.
        if let Err(exc) =
            crate::targets::record_fleet_queue_namespace(config::wc_stado_storage_namespace()).await
        {
            log(&format!("fleet queue namespace record failed: {exc}"));
        }
        match crate::queue::copy::replicate_configured_backup().await {
            Ok(Some(report)) if report.is_clean() => log("disaster-recovery replication clean"),
            Ok(Some(report)) => log(&format!(
                "disaster-recovery replication incomplete: {} object(s) failed",
                report.failed()
            )),
            Ok(None) => {}
            Err(exc) => log(&format!("disaster-recovery replication failed: {exc}")),
        }
        // The standing shape checks, on the interval this loop already has, so
        // that "is what is declared what is running" is answered without
        // anyone typing a command. Every finding carries its own subject,
        // declaration, observation and fix, because a tick log is the only
        // place some of these will ever be read.
        //
        // On 2026-08-30 seven defects of one shape — a declaration nothing
        // compared against reality — were found and fixed by hand in one
        // evening, and nothing in the product would have caught the eighth.
        // This is what catches it.
        {
            let runner = crate::deploy::production_runner();
            let mut shape = crate::fleet_shape::sweep(&runner).await;
            if let Some(finding) = crate::fleet_shape::health_disagreement().await {
                shape.measured += 1;
                shape.findings.push(finding);
            }
            log(&shape.summary());
            for finding in &shape.findings {
                log(&finding.line());
            }
            for (host, reason) in &shape.unreachable {
                log(&format!("fleet shape: {host} not measured — {reason}"));
            }
        }
        if once {
            return Ok(0);
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}
