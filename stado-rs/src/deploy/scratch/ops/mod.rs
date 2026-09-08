//! The steps each scratch operation is made of.
//!
//! Split out of the façade so the order of a lease — create, trust, record,
//! enter — is readable in one place, and so the rollback path can reuse the
//! teardown step exactly as `destroy` uses it. A lease that cannot be entered
//! is deleted by the same code that deletes an expired one.
//!
//! This module holds what both halves read: the host's own account of what it
//! holds.

mod open;
mod teardown;

pub use open::{open_account, settle, Entered};
pub use teardown::{destroy_row, reap_rows, row_for, Swept};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::remote::{self, HostLease, HostState};
use crate::deploy::{host_channel, host_users, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Read every lease record a host holds, with each account's real presence.
pub async fn host_leases(
    target: &ComputeTarget,
    home: &str,
    runner: &Runner,
) -> Result<Vec<HostLease>, DeployError> {
    let command = remote::list_command(home);
    let output =
        host_channel::run_program(target, &["/bin/sh", "-c", command.as_str()], runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the host did not answer the lease read",
        )));
    }
    Ok(remote::parse_leases(&output.stdout))
}

/// One lease as a report row: the record's own fields, the account presence and
/// home the host reported, and the arithmetic done here rather than on the far
/// side.
pub fn lease_json(row: &HostLease, now: DateTime<Utc>) -> Value {
    let mut entry = Map::new();
    entry.insert("name".into(), Value::from(row.name.clone()));
    entry.insert(
        "account".into(),
        Value::from(if row.account_present {
            "present"
        } else {
            "absent"
        }),
    );
    entry.insert(
        "home_path".into(),
        row.home_path.clone().map_or(Value::Null, Value::from),
    );
    match &row.lease {
        Some(held) => {
            entry.insert("username".into(), Value::from(held.username.clone()));
            entry.insert("profile".into(), Value::from(held.profile.clone()));
            entry.insert("created_at".into(), Value::from(held.created_at.clone()));
            entry.insert("expires_at".into(), Value::from(held.expires_at.clone()));
            entry.insert(
                "requested_by".into(),
                Value::from(held.requested_by.clone()),
            );
            entry.insert("expired".into(), Value::from(held.expired(now)));
            entry.insert(
                "seconds_remaining".into(),
                held.seconds_remaining(now).map_or(Value::Null, Value::from),
            );
        }
        None => {
            // A record nobody can read is expired by definition: it is a leak,
            // and the reaper is the only thing that removes leaks.
            entry.insert("expired".into(), Value::from(true));
            entry.insert("seconds_remaining".into(), Value::Null);
        }
    }
    if let Some(reason) = &row.unreadable {
        entry.insert("unreadable".into(), Value::from(reason.clone()));
    }
    Value::Object(entry)
}

/// Ask the host what it holds for one name. A probe that printed no verdict is
/// an error, never an optimistic default.
pub async fn probe_state(
    target: &ComputeTarget,
    name: &str,
    record_path: &str,
    runner: &Runner,
) -> Result<HostState, DeployError> {
    let command = remote::state_command(name, record_path);
    let output =
        host_channel::run_program(target, &["/bin/sh", "-c", command.as_str()], runner).await?;
    remote::parse_state(&output.stdout).ok_or_else(|| {
        DeployError(format!(
            "the host printed no state for '{name}': {}",
            host_channel::last_error_line(&output, "no output at all")
        ))
    })
}

/// The ssh destination of a leased account: the parent's declared route, with
/// the leased login in front of it. The route itself is never rewritten, so a
/// lease is reachable exactly where its host is.
pub fn scratch_ssh(parent: &ComputeTarget, username: &str) -> Result<String, DeployError> {
    let destination = parent
        .ssh_connections()
        .next()
        .map(|(_, destination)| destination)
        .ok_or_else(|| {
            DeployError(format!(
                "target '{}' has no registry-managed ssh destination to lease on",
                parent.name
            ))
        })?;
    let route = destination
        .rsplit_once('@')
        .map_or(destination, |(_, host)| host);
    let ssh = format!("{username}@{route}");
    host_users::validate_ssh_target(&ssh)?;
    Ok(ssh)
}
