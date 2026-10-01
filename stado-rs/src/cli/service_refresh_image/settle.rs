//! The read after the restart that says whether the unit came back.

use crate::deploy::service::{self, UnitImageObservation};

/// Read the unit's image after `kickstart -k`, which returns once launchd has
/// started the job again.
///
/// `None` when nothing is executing that unit's argument vector — which is
/// itself a failure, and is reported as one. A row whose pid is still the one
/// that was kicked is returned as observed, and the caller reports that the
/// unit was not replaced.
///
/// Crate-visible because the release agent's scheduled revisit pass reads
/// exactly this and must reach the same answer: a second definition of "the
/// unit came back" would drift from this one.
pub(crate) async fn settle(
    target: &crate::targets::ComputeTarget,
    host: &str,
    name: &str,
) -> Option<UnitImageObservation> {
    let now = chrono::Utc::now().timestamp();
    service::observe_unit_images(target, Some(host), now)
        .await
        .into_iter()
        .find(|row| row.unit == name)
}
