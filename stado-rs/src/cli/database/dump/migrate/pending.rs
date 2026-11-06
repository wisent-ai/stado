//! Whether `migrate` restores into the fleet server an earlier run left as
//! `<item>-fleet-pending`, or places a new one.
//!
//! A pending placement is reused only for the host and port it was made on:
//! a run that names neither, or names those, goes on where the earlier one
//! stopped. A run that names another host or port is the operator moving the
//! database somewhere else, typically because the pending host can never
//! start the unit; reusing the pending server there would restore into a
//! server that does not run. So such a run is refused naming the pending
//! item, its host and port, unless `--discard-pending` is given, which
//! retires the pending server's unit on its host and deletes the item before
//! placing anew. The pending server's data directory stays on its host and is
//! named in the answer, as `stado database destroy` leaves a data directory:
//! deleting a database's data is never a side effect.
//!
//! What fails, and what is said:
//! - another host or port without `--discard-pending`: refused (usage) naming
//!   the pending item, where it stands and the flag;
//! - `--discard-pending` with no pending item: refused (usage), nothing to
//!   discard;
//! - a pending item that names no host: refused (config), its server cannot
//!   be found to retire;
//! - the unit's removal on the pending host fails: refused with that cause,
//!   the item kept, so a second run finds the same state;
//! - the item's deletion fails: refused with that cause after the unit was
//!   removed, naming both.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::super::super::supabase::owner_vault::Owner;

/// What a run does with the pending placement it found (or did not).
pub(super) enum Decision {
    /// No pending placement: place a new server.
    Place,
    /// Restore into the server the pending item names.
    Reuse(Value),
    /// The pending server was retired and its item deleted: place a new one.
    /// The value is the step the report keeps.
    Discarded(Value),
}

/// The host and port a pending item records.
fn standing(pending: &str, kept: &Value) -> Result<(String, Option<u64>), CmdError> {
    let host = kept["fields"]["host"]
        .as_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{pending} names no host, so the fleet server it keeps cannot be found to reuse or retire"
            ))
            .stating(FailureCode::Config)
        })?;
    Ok((host.to_string(), kept["fields"]["port"].as_u64()))
}

/// Decide for `name` from the pending item `kept` (`None` when the vault
/// holds none), the `host` and `port` this run names, and `discard`.
pub(super) async fn decide(
    name: &str,
    pending: &str,
    kept: Option<Value>,
    host: Option<&str>,
    port: Option<u16>,
    discard: bool,
    owner: &Owner,
) -> Result<Decision, CmdError> {
    let Some(kept) = kept else {
        if discard {
            return Err(CmdError::usage(format!(
                "--discard-pending was given but the vault holds no {pending}: there is no pending placement to discard"
            )));
        }
        return Ok(Decision::Place);
    };
    let (pending_host, pending_port) = standing(pending, &kept)?;
    let other_host = host.is_some_and(|named| named != pending_host);
    let other_port = port.is_some_and(|named| Some(u64::from(named)) != pending_port);
    if !discard {
        if other_host || other_port {
            let at = match pending_port {
                Some(port) => format!("{pending_host}:{port}"),
                None => pending_host.clone(),
            };
            return Err(CmdError::usage(format!(
                "{pending} keeps the fleet server an earlier run of migrate placed on {at}, and this run names \
                 another place for {name}; a second run restores into that server, so it is refused here. \
                 Add --discard-pending to retire that server's unit and delete {pending} before placing anew \
                 (its data directory stays on {pending_host}), or run without --host and --port to go on there"
            )));
        }
        return Ok(Decision::Reuse(kept));
    }
    let unit = format!("{name}-database");
    let target = crate::cli::canonical_host(&pending_host).await?;
    let declared = crate::deploy::service::declared_services(&target)
        .iter()
        .any(|service| service.matches(&unit));
    if declared {
        crate::cli::service::lifecycle::adopt::removal::remove(&unit, &pending_host, false)
            .await
            .map_err(|error| {
                error.also(format!(
                    "the pending unit {unit} on {pending_host} was not removed, so {pending} is kept"
                ))
            })?;
    }
    let deleted = owner.delete(pending).await.map_err(|error| {
        error.also(format!(
            "the unit {unit} on {pending_host} is {}, but {pending} is still in the vault",
            if declared {
                "removed"
            } else {
                "already absent"
            }
        ))
    })?;
    Ok(Decision::Discarded(json!({
        "item": pending,
        "item_status": deleted,
        "host": pending_host,
        "unit": unit,
        "unit_status": if declared { "removed" } else { "already absent" },
        "data": format!("left in place at ~/.stado/databases/{name}/ on {pending_host}"),
    })))
}
