//! `stado service ...` — the full service-management layer.
//!
//! NO Python original: the Python CLI stops at `host recover`, and that gap
//! is the point. `stado.wisent.com/docs/missing-commands` items seven through fourteen
//! were written after a wedged `com.wisent.weles-api` sat unmanaged on a
//! mac mini: the unit existed on the host, Stado did not know about it, and
//! there was no command to list it, restart it or adopt it.
//!
//! The engine is [`crate::deploy::service`]; this module is the operator
//! surface over it. Two properties are worth keeping when editing:
//!
//! - `list` answers from the health beacons alone. No ssh, no per-host
//!   round trip, so the fleet-wide question stays answerable when a host
//!   is the thing that is broken. `status` answers the same way, and adds
//!   best-effort host reads — launchd's last exit status and the stderr
//!   tail — only for units whose beacon state is `failed`; those reads
//!   degrade to a note, never to a failed command.
//! - Registry mutations use
//!   `cli/registry.rs::{commit_document, push_document_if}` — the validated
//!   conditional write path — and never hand-edit the document. `retire` and
//!   `remove` also hold the autonomy reconciler's per-unit lease while they
//!   withdraw the declaration and change the host, so a tick with an older
//!   snapshot cannot start the unit during that transaction.

use serde::Deserialize;
use serde_json::{json, Value};
use std::future::Future;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::deploy::service::{
    self, ManagedService, ServiceEnv, ServiceLog, ServiceStatus, UnitDomain, SOURCE_RECOVERY,
    SOURCE_REGISTRY,
};
use crate::deploy::{
    host_channel, host_exec, production_runner, service_env_file, service_file_fetch,
    service_label_print, service_serving, service_spawn_watch, DeployError,
};
use crate::observations;
use crate::queue::JobStorage;
use crate::targets;

use super::{registry, CmdError};
use crate::cli::reporting::table;

pub mod commands;

// `pub(crate)` only so `unit_program`'s return type stays as reachable as the
// function itself: `UnitProgram` is named by nobody outside this module tree,
// so it is not re-exported below, and a type more private than the function
// returning it is what `private_interfaces` refuses.
pub(crate) mod lifecycle;
mod reports;
mod runtime;

pub use commands::{dispatch, ServiceCommands};

pub(crate) use lifecycle::declare::ensure::program::{declared_label, unit_program};
pub(crate) use lifecycle::deploy::catalog::{
    ensure_local_dependency, reconcile_after_config_change,
};
pub(crate) use lifecycle::release::release_pipeline_product;
pub(crate) use lifecycle::release::unit::{host_sudo_password, restart};
pub(crate) use runtime::secrets::service_secret;

// ---------------------------------------------------------------------------
// Shared resolution
// ---------------------------------------------------------------------------

fn click(exc: DeployError) -> CmdError {
    CmdError::click(exc.to_string())
}
async fn resolve_placement(
    host: Option<&str>,
    host_heuristic: Option<&str>,
) -> Result<(crate::targets::ComputeTarget, Option<String>), CmdError> {
    let resolved_host = if let Some(host) = host {
        host.to_string()
    } else if let Some(heuristic) = host_heuristic {
        let registry = targets::load_registry_auto()
            .await
            .map_err(|exc| CmdError::click(exc.to_string()))?;
        registry
            .lookup_host_heuristic(heuristic)
            .map(|target| target.name.clone())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "host heuristic '{heuristic}' matches no local registry target"
                ))
            })?
    } else {
        return Err(CmdError::click(
            "either --host or --host-heuristic is required".to_string(),
        ));
    };
    let target = host_channel::canonical_target(&resolved_host)
        .await
        .map_err(click)?;
    Ok((target, host_heuristic.map(str::to_string)))
}

/// Reuse the host command's provider-neutral beacon store selection.
async fn beacon_store() -> Result<JobStorage, CmdError> {
    super::host::beacon_store().await
}

/// The declared managed set matching NAME, without touching beacons.
///
/// The write-side commands need the declaration — its unit id and its
/// unit-file path — not its state, so they must not pay for a beacon read
/// per host to get it.
pub(crate) async fn declared_matching(
    name: &str,
    host: Option<&str>,
) -> Result<Vec<ManagedService>, CmdError> {
    if let Some(host) = host {
        // Resolve the host first so an unknown or non-local target reports
        // the registry's own precise refusal rather than "no such service".
        host_channel::canonical_target(host).await.map_err(click)?;
    }
    let registry = registry::read_registry().await?;
    let mut found: Vec<ManagedService> = Vec::new();
    for target in registry.local_targets() {
        if host.is_some_and(|host| target.name != host) {
            continue;
        }
        found.extend(
            service::declared_services(target)
                .into_iter()
                .filter(|declared| declared.matches(name)),
        );
    }
    if found.is_empty() {
        return Err(unmanaged(name, host));
    }
    Ok(found)
}

fn unmanaged(name: &str, host: Option<&str>) -> CmdError {
    match host {
        Some(host) => CmdError::click(format!(
            "{name} is not a registry-managed service on {host}"
        )),
        None => CmdError::click(format!("no registry-managed service named {name}")),
    }
}

/// `-` for an empty cell, the spelling `monitor/host_health.rs` already
/// prints for a beacon field it does not have.
fn dash(value: &str) -> String {
    if value.is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

fn print_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Report a partial failure after the per-host results have been printed,
/// so the operator sees which hosts worked as well as which did not.
fn fail_if_any(failures: &[String], action: &str) -> Result<(), CmdError> {
    if failures.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{action} failed on {}",
        failures.join("; ")
    )))
}
