//! Advance only after an actual queued job ends, never after an invented delay.
use super::super::runner::{require, Journey};
use stado::release_pipeline::{DeliveryRunState, PlatformRunState, ReleaseRun, ReleaseRunState};
use std::collections::BTreeSet;

pub(super) fn finish(journey: &mut Journey, initial: &ReleaseRun) -> Result<ReleaseRun, String> {
    let refreshed = journey.cli(&["release", "resume", &initial.run_id, "--json"], true)?;
    let mut run: ReleaseRun =
        serde_json::from_value(refreshed).map_err(|error| error.to_string())?;
    let mut observed = BTreeSet::new();
    loop {
        if run.state == ReleaseRunState::Completed {
            return Ok(run);
        }
        require(
            !run.state.finished(),
            &format!(
                "release {} ended in {:?}: {:?}",
                run.run_id, run.state, run.failure
            ),
        )?;
        let pending = run
            .platforms
            .values()
            .filter(|platform| platform.state == PlatformRunState::Submitted)
            .map(|platform| platform.job_id.clone())
            .chain(
                run.deliveries
                    .values()
                    .filter(|delivery| delivery.state == DeliveryRunState::Submitted)
                    .map(|delivery| delivery.job_id.clone()),
            )
            .collect::<BTreeSet<_>>();
        let mut progressed = false;
        for job in pending {
            require(
                !job.is_empty(),
                "a submitted release leg has no actual job identity",
            )?;
            if !observed.insert(job.clone()) {
                continue;
            }
            let terminal =
                journey.cli(&["machine", "status", &job, "--until", "terminal"], true)?;
            let record = &terminal["result"]["job"];
            require(
                record["terminal"].as_bool() == Some(true),
                &format!("job {job} did not reach terminal: {terminal}"),
            )?;
            require(
                record["error"].is_null()
                    && record["failed_at"].is_null()
                    && record["completed_at"].is_string(),
                &format!("real release job {job} failed: {terminal}"),
            )?;
            progressed = true;
        }
        require(
            progressed,
            &format!(
                "release {} remains {:?} without a new job to observe: {:?}",
                run.run_id, run.state, run.failure
            ),
        )?;
        let refreshed = journey.cli(&["release", "resume", &run.run_id, "--json"], true)?;
        run = serde_json::from_value(refreshed).map_err(|error| error.to_string())?;
    }
}
