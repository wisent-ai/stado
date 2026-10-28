//! What the fleet's own agents published about themselves, read for the host
//! a session is about to be placed on.
//!
//! Split out of `jeden/mod.rs`, which had grown past the module line cap; the
//! candidates and the readiness probe stay there.

use serde_json::Value;

use crate::deploy::host_channel;
use crate::targets::ComputeTarget;

/// Every agent's capacity publication. A store that cannot be opened or read
/// is refused: read as no publications, every host looked as if its agent had
/// said nothing, so a host refusing work was chosen and the session queued.
pub(super) async fn live_capacity() -> Result<Vec<Value>, crate::cli::CmdError> {
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(|error| {
            crate::cli::CmdError::click(format!(
                "the job store holding the agents' capacity publications cannot be opened, so no \
             host can be judged ready for a session: {error}"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
    let entries = crate::queue::capacity::read_consumer_capacity(&store)
        .await
        .map_err(|error| {
            crate::cli::CmdError::click(format!(
                "the agents' capacity publications cannot be read, so no host can be judged \
                 ready for a session: {error}"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
    Ok(entries.into_values().collect())
}

/// The publication this target's own agent wrote, when it wrote one.
fn live_entry<'a>(target: &ComputeTarget, capacity: &'a [Value]) -> Option<&'a Value> {
    let hostnames = target
        .hostnames
        .iter()
        .map(|host| crate::targets::normalize_hostname(host))
        .collect::<Vec<_>>();
    capacity
        .iter()
        .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("local"))
        .find(|entry| {
            // A publication that names no consumer belongs to no host.
            let Some(consumer) = entry.get("consumer_id").and_then(Value::as_str) else {
                return false;
            };
            hostnames.iter().any(|host| {
                consumer == format!("local-{host}")
                    || consumer
                        .strip_prefix("local-")
                        .is_some_and(|value| crate::targets::normalize_hostname(value) == *host)
            })
        })
}

/// Why `target` cannot take work right now, in its own agent's words, or
/// nothing when it is accepting.
///
/// A detached session is pinned to the host its placement chose, so a host
/// that publishes `accepting_jobs: false` (disk pressure, say) would hold
/// the session queued until that clears; refusing here moves the placement
/// to a host that can claim it. A host with no capacity broadcast, or one
/// that does not say whether it accepts, is not known to accept: it was
/// placed on as if it did, and the session waited on a host nobody heard.
pub(super) fn admission_refusal(target: &ComputeTarget, capacity: &[Value]) -> Option<String> {
    let Some(live) = live_entry(target, capacity) else {
        return Some(format!(
            "{} publishes no capacity broadcast, so whether it accepts placements is not known",
            target.name
        ));
    };
    match live.get("accepting_jobs").and_then(Value::as_bool) {
        Some(true) => None,
        Some(false) => {
            let reason = match live
                .get("diag")
                .and_then(|diag| diag.get("admission_reason"))
                .and_then(Value::as_str)
                .filter(|reason| !reason.is_empty())
            {
                Some(reason) => reason.to_string(),
                None => "its agent published no reason".to_string(),
            };
            Some(format!(
                "{} is not accepting placements ({reason})",
                target.name
            ))
        }
        None => Some(format!(
            "{}'s capacity broadcast names no accepting_jobs, so whether it accepts \
             placements is not known",
            target.name
        )),
    }
}

/// Where `target` stands in the order hosts are offered a session, from what
/// its own agent published: hosts that say they accept and state their free
/// cores come first, more free cores before fewer; then this host before
/// others. A host that published nothing, or did not state its cores, is
/// still offered last, and its refusal is named by [`admission_refusal`].
/// Sort descending by this key; ties fall to the host's name.
pub(super) fn placement_order(
    target: &ComputeTarget,
    capacity: &[Value],
) -> (bool, Option<i64>, bool) {
    let live = live_entry(target, capacity);
    let accepting = live
        .and_then(|entry| entry.get("accepting_jobs"))
        .and_then(Value::as_bool)
        == Some(true);
    let free_cores = live
        .and_then(|entry| entry.get("available_cpu_cores"))
        .and_then(Value::as_i64);
    (
        accepting && free_cores.is_some(),
        free_cores,
        host_channel::target_is_this_host(target),
    )
}
