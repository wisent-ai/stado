//! The first read: what this unit is on, and whether that is a reason to act.

use crate::cli::CmdError;
use crate::deploy::service::{self, ImageIdentity, ImageState, UnitImageObservation};

/// This unit's observation, or the reason there is none.
pub(super) async fn observe(
    target: &crate::targets::ComputeTarget,
    host: &str,
    name: &str,
) -> Result<UnitImageObservation, CmdError> {
    let now = chrono::Utc::now().timestamp();
    service::observe_unit_images(target, Some(host), now)
        .await
        .into_iter()
        .find(|row| row.unit == name)
        .ok_or_else(|| {
            CmdError::click(format!(
                "{host} holds no launchd unit named {name} with a live process. Either no unit \
                 file in launchd's three directories declares that label, or the unit is loaded \
                 and not running — a job that is not running holds no image, so there is nothing \
                 to refresh. `stado registry doctor` lists the units this machine was measured on"
            ))
        })
}

/// The identities to act on, or a refusal.
///
/// A unit that is not stale is refused with the identity that was read, so the
/// refusal is checkable rather than a bare "no".
pub(super) fn actionable(
    row: &UnitImageObservation,
) -> Result<(ImageIdentity, ImageIdentity), CmdError> {
    match &row.state {
        Some(
            ImageState::Unlinked { running, installed }
            | ImageState::Replaced { running, installed },
        ) => Ok((running.clone(), installed.clone())),
        Some(ImageState::Unread { subject, reason }) => Err(CmdError::click(format!(
            "{} was not restarted, because whether it is stale is unknown: {subject} could not be \
             read — {reason}. An unread state is not a reason to act any more than it is a reason \
             to pass",
            row.unit
        ))),
        None => Err(CmdError::click(refusal(row))),
    }
}

/// Why a unit that is not stale is left alone, naming what was found.
fn refusal(row: &UnitImageObservation) -> String {
    let pid = row
        .pid
        .map_or_else(|| "its process".to_string(), |pid| format!("pid {pid}"));
    match (&row.running, &row.installed) {
        (Some(running), Some(installed)) if running.is_same_file(installed) => format!(
            "{} is not stale and was not restarted: {pid} is executing {}, which IS the file its \
             ProgramArguments name at {}. Restarting it would be an outage with nothing to fix",
            row.unit,
            running.describe(),
            installed.path
        ),
        (Some(running), Some(installed)) => format!(
            "{} was not restarted: {pid} is executing {} and its declared file at {} is {}, but \
             that file was written less than {}s ago. A replacement inside that window is an \
             installer mid-flight, not a unit left behind — re-run once it has settled",
            row.unit,
            running.describe(),
            installed.path,
            installed.describe(),
            service::IMAGE_SETTLE_SECONDS
        ),
        _ => format!(
            "{} was not restarted: neither identity was read, so there is no evidence it is stale",
            row.unit
        ),
    }
}
