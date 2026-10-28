//! The schedules pinned to a host, as `placement holds` reads them and as an
//! evacuation (`--repin-schedules`) hands them over and reads them back.

use serde_json::Value;

use super::super::scan::listed;
use crate::cli::CmdError;

/// The schedules pinned to `host`, each with the pin it carries: the host's
/// registry name or a `<kind>-<hostname>` identity that names it (read by
/// the rule the claim path uses, `host_gates::resolves_to`). Schedules pinned
/// to a host keep firing there after its services leave, so they are named;
/// a store that does not answer is said, not read as none.
pub(in crate::cli::placement::commands::evacuate) async fn pinned_schedules(
    document: &Value,
    host: &str,
) -> Result<Vec<(String, String)>, String> {
    let registry = crate::cli::placement::candidates::parse_registry(document)
        .map_err(|error| error.to_string())?;
    let target = crate::cli::placement::candidates::target(&registry, host)
        .map_err(|error| error.to_string())?;
    pins_of(&registry, target, &all_schedules().await?)
}

/// Every schedule the store holds, or why it did not answer.
pub(super) async fn all_schedules() -> Result<Vec<crate::schedules::Schedule>, String> {
    let store = crate::queue::JobStorage::new()
        .await
        .map_err(|error| error.to_string())?;
    crate::schedules::list_schedules(&store)
        .await
        .map_err(|error| error.to_string())
}

/// Of `schedules`, the live ones pinned to `target`, each with its pin.
pub(super) fn pins_of(
    registry: &crate::targets::Registry,
    target: &crate::targets::ComputeTarget,
    schedules: &[crate::schedules::Schedule],
) -> Result<Vec<(String, String)>, String> {
    let mut pinned = Vec::new();
    for schedule in schedules
        .iter()
        .filter(|schedule| !schedule.deleted && !schedule.pinned_host.is_empty())
    {
        let names_host = schedule.pinned_host == target.name
            || crate::deploy::host_gates::resolves_to(registry, target, &schedule.pinned_host)
                .map_err(|error| {
                    format!(
                        "schedule {} is pinned to {}: {error}",
                        schedule.schedule_id, schedule.pinned_host
                    )
                })?;
        if names_host {
            pinned.push((schedule.schedule_id.clone(), schedule.pinned_host.clone()));
        }
    }
    Ok(pinned)
}

/// The schedules an evacuation asked to repin would repin, read before
/// anything moves; a schedule store that does not answer refuses the
/// evacuation with the other refusals. Empty when repinning was not asked.
pub(in crate::cli::placement::commands::evacuate) async fn schedules_to_repin(
    document: &Value,
    from: &str,
    asked: bool,
    refusals: &mut Vec<String>,
) -> Vec<String> {
    if !asked {
        return Vec::new();
    }
    match pinned_schedules(document, from).await {
        Ok(pinned) => pinned.into_iter().map(|(id, _)| id).collect(),
        Err(cause) => {
            refusals.push(format!(
                "--repin-schedules: the schedules pinned to {from} could not be read: {cause}"
            ));
            Vec::new()
        }
    }
}

/// Pin every schedule pinned to `from` to `to` instead, each through the
/// schedule store's compare-and-swap edit, so a concurrent change to a
/// schedule is never overwritten and one pinned elsewhere meanwhile is left
/// as it is. Returns the schedules repinned.
pub(in crate::cli::placement::commands::evacuate) async fn repin_schedules(
    document: &Value,
    from: &str,
    to: &str,
) -> Result<Vec<String>, CmdError> {
    let unread = |cause: String| {
        CmdError::click(format!(
            "the schedules pinned to {from} could not be read, so none was repinned: {cause}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    };
    let pinned = pinned_schedules(document, from).await.map_err(unread)?;
    // The pin a schedule carries is the consumer identity submit resolves a
    // target name to, so B's is written the same way.
    let pin_to = crate::cli::submit::resolve_pinned_host(to).await?;
    let store = crate::queue::JobStorage::new()
        .await
        .map_err(|error| unread(error.to_string()))?;
    let mut repinned = Vec::new();
    for (id, pin) in pinned {
        let edited = crate::schedules::edit_schedule(&store, &id, |schedule| {
            if schedule.pinned_host == pin {
                schedule.pinned_host = pin_to.clone();
            }
        })
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "schedule {id} could not be repinned from {from} to {to} (repinned before it: {}): {error}",
                listed(&repinned)
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        if edited.is_some_and(|schedule| schedule.pinned_host == pin_to) {
            repinned.push(id);
        }
    }
    // Read back: a schedule pinned to A after the pass (created meanwhile,
    // or one whose edit raced) is refused by name rather than left firing.
    let left = pinned_schedules(document, from).await.map_err(unread)?;
    if !left.is_empty() {
        let names: Vec<String> = left
            .into_iter()
            .map(|(id, pin)| format!("{id} ({pin})"))
            .collect();
        return Err(CmdError::click(format!(
            "repinned {} to {to}, but {} still pinned to {from}; run the evacuation again to repin them",
            listed(&repinned),
            names.join(", ")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Ok(repinned)
}
