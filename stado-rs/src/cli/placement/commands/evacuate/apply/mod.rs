//! The run half of an evacuation, once every step was checked: the install
//! roots, the profiles that need preparing, the vault, each profile, each
//! role of the host Stado process, the registry authority, then the
//! read-back that the host is emptied, the schedules and the release
//! targets. A step that fails stops the run naming what moved to B before it
//! and what still stands on A.

use serde_json::{json, Value};

use super::scan::listed;
use super::{fixed, holds, steps};
use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::placement::PlacementProfile;

/// What the checked evacuation runs.
pub(super) struct Plan<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub reason: &'a str,
    pub vault_owner_uid: Option<&'a str>,
    pub authority: bool,
    /// The first role carries the authority (`role move --with-authority`).
    pub carried: bool,
    /// The authority was already taken on B before anything was read.
    pub taken_first: bool,
    pub repin_schedules: bool,
    pub document: &'a Value,
    pub portable: &'a [String],
    pub unstaged: &'a [&'a PlacementProfile],
    pub vaulted: &'a [PlacementProfile],
    pub profiles: &'a [PlacementProfile],
    pub roles: &'a [String],
    pub release_drops: &'a [String],
    pub runner: &'a Runner,
}

/// The receipts the run replaces the check's answers with.
pub(super) struct Reports {
    pub portable: Value,
    pub vault: Value,
    pub profiles: Vec<Value>,
    pub roles: Vec<Value>,
    pub authority: Value,
    pub schedules: Vec<String>,
    pub release: Value,
}

/// Run every step of `plan` in order, writing each receipt into `reports`.
pub(super) async fn run(plan: &Plan<'_>, reports: &mut Reports) -> Result<(), CmdError> {
    let Plan {
        from, to, runner, ..
    } = *plan;
    // `done` names what moved before a failure.
    let mut done: Vec<String> = plan
        .taken_first
        .then(|| "the registry authority".to_string())
        .into_iter()
        .collect();
    let names: Vec<String> = plan
        .vaulted
        .iter()
        .chain(plan.profiles.iter())
        .map(|profile| profile.name.clone())
        .chain(plan.roles.iter().cloned())
        .collect();
    let stopped = |error: CmdError, done: &[String]| {
        let left: Vec<String> = names
            .iter()
            .filter(|name| !done.contains(name))
            .cloned()
            .collect();
        error.also(format!(
            "the evacuation of {from} stopped; moved to {to} before it: {}; still on {from}: {}; the same command run again reads what still stands on {from} and moves only that",
            listed(done),
            listed(&left)
        ))
    };
    // Install roots first, then the profiles that need preparing (stores
    // the catalog declares, B standing by), each passing its preflight.
    reports.portable = json!(fixed::portable_roots(plan.portable, to, runner)
        .await
        .map_err(|error| stopped(error, &done))?);
    for profile in plan.unstaged {
        if let Err(error) = steps::prepare(&profile.name, to, plan.reason, runner).await {
            return Err(stopped(error, &done));
        }
    }
    if let (Some(profile), Some(uid)) = (plan.vaulted.first(), plan.vault_owner_uid) {
        match steps::vault(to, uid, true, runner).await {
            Ok(report) => reports.vault = report,
            Err(error) => return Err(stopped(error, &done)),
        }
        done.push(profile.name.clone());
    }
    reports.profiles.clear();
    for profile in plan.profiles {
        match steps::move_profile(&profile.name, to).await {
            Ok(report) => reports.profiles.push(report),
            Err(error) => return Err(stopped(error, &done)),
        }
        done.push(profile.name.clone());
    }
    reports.roles.clear();
    for role in plan.roles {
        let carries = plan.carried && plan.roles.first() == Some(role);
        match fixed::move_role(role, to, carries, plan.reason, runner).await {
            Ok(receipt) => {
                if carries {
                    reports.authority = receipt["authority"].clone();
                }
                reports.roles.push(receipt);
            }
            Err(error) => return Err(stopped(error, &done)),
        }
        done.push(role.clone());
    }
    if plan.authority && !plan.taken_first && !plan.carried {
        match steps::authority(to, true, runner).await {
            Ok(report) => reports.authority = report,
            Err(error) => return Err(stopped(error, &done)),
        }
    }
    holds::ensure_emptied(from, to, plan.authority)
        .await
        .map_err(|error| stopped(error, &done))?;
    if plan.repin_schedules {
        reports.schedules = holds::repin_schedules(plan.document, from, to)
            .await
            .map_err(|error| stopped(error, &done))?;
    }
    let dropped = steps::drop_release_targets(from, plan.release_drops)
        .await
        .map_err(|error| stopped(error, &done))?;
    reports.release = json!(dropped);
    Ok(())
}
