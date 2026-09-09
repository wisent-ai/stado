//! `stado service refresh-image NAME` — put one unit back on the file it
//! declares, and prove it landed.
//!
//! `registry doctor` grew a `stale-unit-image` row that ends "Restarting the
//! unit is what puts it on the installed file, and nothing does that on its
//! own". That sentence told an operator to perform an action the product did
//! not offer as a checked operation. This is the verb behind it.
//!
//! Three properties, each one measured rather than assumed:
//!
//! - **It refuses a unit that is not stale**, and the refusal names the
//!   identity it found. A command that restarts whatever it is pointed at is a
//!   restart button, and this fleet has already turned a degraded host into a
//!   down one with one of those.
//! - **It re-reads the identity afterwards.** On 2026-09-03 pid 49727 —
//!   `com.wisent.compute.agent.lukasz-macbook` — respawned under `KeepAlive`
//!   straight back onto the same unlinked inode 182274754 it had just left.
//!   launchd re-execs the PATH, and the path was never the problem; a
//!   remediation that reported success on the strength of having issued a
//!   restart would have been a second silence exactly where the first one was.
//!   So a restart that did not change the image is a failure with a non-zero
//!   exit, not a caveat in a success message.
//! - **One unit per invocation.** No `--all`, no glob, no sweep. Three stale
//!   units on a host is three deliberate commands, because the blast radius of
//!   a sweep across a fleet agent, a janitor and a stream writer is the whole
//!   host.
//!
//! The release agent's scheduled revisit pass now reuses this module's
//! [`refresh_outcome`] and the same `observe_unit_images` predicate, so a
//! manual refresh and an automatic one reach their verdict by one route. The
//! two callers keep different blast radii and that is deliberate: this
//! command is one named unit per invocation, typed by an operator who has
//! read the row, and the scheduled caller may touch only the exact launchd
//! labels the registry's top-level `release_unit_image_revisit` block
//! authorises for that host, one of them per tick. Neither widens the other.
//!
//! The predicate is not reimplemented here. `deploy::service::
//! observe_unit_images` is the one pass `registry doctor` reads, so a unit this
//! command calls stale and a unit the doctor reports are the same set by
//! construction rather than by two implementations agreeing today.

mod inspect;
mod outcome;
mod report;
mod settle;

use serde_json::json;

use crate::deploy::service;

use super::{registry, CmdError};

use inspect::{actionable, observe};
use outcome::verdict;
use report::emit;

pub use outcome::{refresh_outcome, RefreshOutcome};
pub(crate) use settle::settle;

/// `stado service refresh-image NAME [--if-needed] [--json]`.
pub async fn refresh_image(name: &str, if_needed: bool, json_output: bool) -> Result<(), CmdError> {
    let registry = registry::read_registry().await?;
    let hostname = crate::providers::vast::system_hostname();
    let local = registry
        .lookup_self(&hostname)
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "no registry target names this machine ({hostname}), and which image a process \
                 is executing is readable only on the machine holding that process; run this \
                 there"
            ))
        })?;
    let host = local.name.clone();

    let before = observe(local, &host, name).await?;
    let (running, installed) = if if_needed {
        match (&before.running, &before.installed) {
            (Some(running), Some(installed)) if running.is_same_file(installed) => {
                if json_output {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "host": host,
                            "unit": name,
                            "status": "already_active",
                            "pid": before.pid,
                            "running": running.describe(),
                            "installed": installed.describe(),
                        }))?
                    );
                } else {
                    println!("{name} already executes {}", running.describe());
                }
                return Ok(());
            }
            (Some(running), Some(installed)) => (running.clone(), installed.clone()),
            _ => actionable(&before)?,
        }
    } else {
        actionable(&before)?
    };

    let service = service::restart_local_unit(local, &before.unit, &before.unit_path, None)
        .await
        .map_err(|reason| {
            CmdError::click(format!("{} was not restarted: {reason}", before.unit))
        })?;

    let after = settle(local, &host, name, before.pid).await;
    emit(&before, after.as_ref(), &service, json_output);
    verdict(&before, after.as_ref(), &running, &installed)
}
