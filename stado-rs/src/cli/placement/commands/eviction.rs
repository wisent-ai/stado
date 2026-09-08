//! `stado placement evict` — end an instance running where the directory
//! places nothing.

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use crate::cli::placement::candidates::ensure_profile_lifecycle_mutable;
use crate::cli::{directory, registry, CmdError};
use crate::deploy::service::{self, SOURCE_REGISTRY};
use crate::deploy::{host_channel, production_runner};
use crate::placement;

/// Stop the process holding a declared service's port on a host the directory
/// does not place it on.
///
/// The guard is the directory itself: eviction is refused on the host that
/// holds the placement, so the command can never take down the real instance.
/// The remote step is the same listener reset launchd already needs when an
/// unmanaged fallback keeps the port.
pub(super) async fn evict(service: &str, host: &str, json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let entry = document
        .get("service_directory")
        .and_then(|block| block.get("services"))
        .and_then(|services| services.get(service))
        .ok_or_else(|| {
            CmdError::click(format!(
                "the directory declares no service named {service:?}"
            ))
        })?;
    if let Some(profile_name) = entry.get("placement_profile").and_then(Value::as_str) {
        let profile = placement::profiles(&document)
            .map_err(CmdError::click)?
            .into_iter()
            .find(|profile| profile.name == profile_name)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "placement profile {profile_name:?} disappeared before eviction"
                ))
            })?;
        ensure_profile_lifecycle_mutable(&profile)?;
    }
    let active = entry
        .get("active_host")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click(format!("{service} has no active_host in the directory")))?;
    if active.starts_with(host) || host.starts_with(active) {
        return Err(CmdError::click(format!(
            "{service} is placed on {active}; evicting its own host would end the real instance. \
             Use `stado service stop {service} --host {host}` to stop a placed service"
        )));
    }
    let port = directory::service_port(entry, active)
        .ok_or_else(|| CmdError::click(format!("the directory declares no port for {service}")))?;
    let target = host_channel::canonical_target(host)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    // The listener reset takes a probe URL and reads its port; the unit id and
    // path are only markers here, because the squatter has no unit on this
    // host - that is what makes it a squatter.
    let squatter = service::launchd_service(
        host,
        &format!("unmanaged.{service}"),
        "",
        SOURCE_REGISTRY,
        &Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
    );
    let report = service::reset_service_listener(
        &target,
        &squatter,
        &format!("http://127.0.0.1:{port}/"),
        &production_runner(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": service,
                "host": host,
                "placed_on": active,
                "port": port,
                "status": report.status,
                "detail": report.detail,
            }))?
        );
    } else {
        println!(
            "{host}\t{service}\tport {port}\t{}\t{}",
            report.status, report.detail
        );
    }
    Ok(())
}
