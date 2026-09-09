//! The restart window, and the second read that closes it.

use std::time::Duration;

use crate::deploy::service::{self, UnitImageObservation};

/// How long to wait for launchd to bring the unit back after `kickstart -k`.
///
/// The unit is stopped and started again, so the window has to cover a process
/// exit and a fresh exec. 30 seconds against a measured restart-to-first-work
/// latency of 55 seconds for the janitor — which included a whole cleanup pass
/// — is enough to see the process appear, which is all that is being waited
/// for; the work it then does is not this command's claim.
pub(super) const RESTART_WINDOW: Duration = Duration::from_secs(30);

/// How often to re-read while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Wait for the unit to come back, and read its image again.
///
/// `None` when nothing was executing that unit's argument vector by the end of
/// the window — which is itself a failure, and is reported as one.
///
/// Crate-visible and `async` because the release agent's scheduled revisit
/// pass waits for exactly this and must reach the same answer: a second
/// settle loop would be a second definition of "the unit came back". `async`
/// rather than blocking so that a tick which is waiting thirty seconds for
/// launchd is not a tick holding a runtime worker for thirty seconds.
pub(crate) async fn settle(
    target: &crate::targets::ComputeTarget,
    host: &str,
    name: &str,
    was: Option<u32>,
) -> Option<UnitImageObservation> {
    let deadline = std::time::Instant::now() + RESTART_WINDOW;
    let mut last = None;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        let now = chrono::Utc::now().timestamp();
        let found = service::observe_unit_images(target, Some(host), now)
            .await
            .into_iter()
            .find(|row| row.unit == name);
        if let Some(row) = found {
            // A row whose pid is the one that was just kicked is launchd not
            // having got there yet, not an answer. Anything else — a new pid,
            // or a read that failed — is the state to report.
            let settled = row.pid != was;
            last = Some(row);
            if settled {
                return last;
            }
        }
    }
    last
}
